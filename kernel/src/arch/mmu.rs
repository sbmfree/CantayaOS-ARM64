//! AArch64 MMU bootstrap for the QEMU `virt` platform.
//!
//! The first MMU milestone deliberately keeps execution at the kernel linker
//! address (`0x4000_0000`) through TTBR0_EL1.  It also installs a valid TTBR1
//! high-half alias for the kernel image, which is the bridge to the later
//! higher-half linker and per-process address-space milestones.

use aarch64_cpu::{asm::barrier, registers::*};
use alloc::vec::Vec;
use cantaya_shared::{BootInfo, MemoryType};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use tock_registers::interfaces::{ReadWriteable, Readable, Writeable};

const PAGE_SIZE: u64 = 4096;
const GIB: u64 = 1024 * 1024 * 1024;
const ENTRIES_PER_TABLE: usize = 512;
const IDENTITY_LIMIT: u64 = ENTRIES_PER_TABLE as u64 * GIB;
const DIRECT_MAP_PHYS_LIMIT: u64 = 16 * GIB;

/// Lower linker address for the bootstrapped kernel image.
pub const KERNEL_LINK_BASE: u64 = 0x4000_0000;
/// Canonical location for the CantayaOS higher-half kernel mapping.
pub const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_8000_0000_0000;
/// Start of the dynamically mapped kernel virtual-memory region.
pub const KERNEL_DYNAMIC_BASE: u64 = KERNEL_HIGHER_HALF_BASE + 0x0000_0001_0000_0000;
/// Start of the permanent high-half physical-memory mapping.
pub const KERNEL_DIRECT_MAP_BASE: u64 = KERNEL_HIGHER_HALF_BASE + 0x0000_0040_0000_0000;
pub const KERNEL_DIRECT_MAP_LIMIT: u64 = KERNEL_DIRECT_MAP_BASE + DIRECT_MAP_PHYS_LIMIT;
pub const KERNEL_DYNAMIC_LIMIT: u64 = KERNEL_DIRECT_MAP_BASE;
/// User-mode addresses occupy the lower half of the 48-bit virtual space.
pub const USER_ADDRESS_LIMIT: u64 = 1 << 48;
/// First address used for automatically placed user virtual-memory regions.
const USER_DYNAMIC_BASE: u64 = 0x0000_0002_0000_0000;
const TTBR1_L0_INDEX: usize = ((KERNEL_HIGHER_HALF_BASE >> 39) & 0x1FF) as usize;
const TTBR1_L1_INDEX: usize = ((KERNEL_HIGHER_HALF_BASE >> 30) & 0x1FF) as usize;
const DIRECT_MAP_L1_INDEX: usize = ((KERNEL_DIRECT_MAP_BASE >> 30) & 0x1FF) as usize;

// Page-table descriptor bits.
const PTE_VALID: u64 = 1 << 0;
const PTE_TABLE: u64 = 1 << 1;
const PTE_AF: u64 = 1 << 10;
const PTE_SH_INNER: u64 = 0b11 << 8;
const PTE_AP_KERNEL_RW: u64 = 0b00 << 6;
const PTE_AP_USER_RW: u64 = 0b01 << 6;
const PTE_AP_KERNEL_RO: u64 = 0b10 << 6;
const PTE_AP_USER_RO: u64 = 0b11 << 6;
const PTE_AP_MASK: u64 = 0b11 << 6;
const PTE_ATTR_NORMAL: u64 = 0 << 2;
const PTE_ATTR_DEVICE: u64 = 1 << 2;
const PTE_UXN: u64 = 1 << 54;
const PTE_PXN: u64 = 1 << 53;
const PTE_ADDRESS_MASK: u64 = 0x0000_00FF_FFFF_F000;

#[repr(C, align(4096))]
struct PageTable([u64; ENTRIES_PER_TABLE]);

impl PageTable {
    const fn zero() -> Self {
        Self([0; ENTRIES_PER_TABLE])
    }
}

// A 48-bit TTBR0 VA uses L0 -> L1 -> L2 -> L3.  The bootstrap requires only
// L0[0] -> L1 because QEMU RAM and the configured devices live below 512 GiB.
static mut TTBR0_L0: PageTable = PageTable::zero();
static mut TTBR0_L1: PageTable = PageTable::zero();

// The high-half alias needs a separate L0/L1 hierarchy.  One L1 block maps
// the first GiB of the loaded kernel image at KERNEL_HIGHER_HALF_BASE.
static mut TTBR1_L0: PageTable = PageTable::zero();
static mut TTBR1_L1: PageTable = PageTable::zero();

/// Serialises updates to the live TTBR1 kernel page table hierarchy.
static PAGE_TABLE_LOCK: Mutex<()> = Mutex::new(());

/// Firmware may already have translation enabled before CantayaOS takes
/// control. This becomes true only after CantayaOS has installed its own
/// TTBR1 direct map.
static KERNEL_DIRECT_MAP_READY: AtomicBool = AtomicBool::new(false);

bitflags::bitflags! {
    /// Permissions for a stage-1 page mapping.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct PagePermissions: u8 {
        const WRITE = 1 << 0;
        const EXECUTE = 1 << 1;
        const USER = 1 << 2;
        const DEVICE = 1 << 3;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageMapError {
    UnalignedAddress,
    OutsideKernelWindow,
    OutsideUserWindow,
    InvalidRange,
    ExistingBlockMapping,
    AlreadyMapped,
    NotMapped,
    InvalidTableHierarchy,
}

/// A mapped EL0 stack with one deliberately unmapped guard page below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserStack {
    /// Unmapped page below the stack. A downward stack overflow faults here.
    pub guard_page: u64,
    /// Lowest mapped stack page.
    pub mapped_base: u64,
    /// First address above the stack allocation; initialize SP_EL0 with this.
    pub top: u64,
    pub page_count: usize,
}

#[derive(Clone, Copy)]
struct OwnedUserPage {
    virt: u64,
    phys: u64,
}

#[derive(Clone, Copy)]
struct OwnedUserRegion {
    base: u64,
    page_count: usize,
}

/// Independently-owned TTBR0 page-table hierarchy for one EL0 process.
pub struct UserAddressSpace {
    root_phys: u64,
    table_pages: Vec<u64>,
    guard_pages: Vec<u64>,
    owned_pages: Vec<OwnedUserPage>,
    regions: Vec<OwnedUserRegion>,
}

impl UserAddressSpace {
    /// Allocate a private four-level TTBR0 root.
    ///
    /// User mappings are installed explicitly. The root also retains a
    /// supervisor-only low alias for the currently low-linked kernel image:
    /// exception and logging code still contain absolute linker addresses
    /// while executing through TTBR1. It grants no EL0 access and excludes
    /// the low MMIO window.
    pub fn new() -> Self {
        let root_phys = crate::executive::mm::phys::alloc_page();
        let kernel_l1_phys = crate::executive::mm::phys::alloc_page();
        unsafe { zero_page_table(root_phys) };
        unsafe { zero_page_table(kernel_l1_phys) };
        unsafe {
            let root = page_table_at(root_phys);
            (*root).0[0] = (kernel_l1_phys & PTE_ADDRESS_MASK) | PTE_VALID | PTE_TABLE;

            let kernel_l1 = page_table_at(kernel_l1_phys);
            (*kernel_l1).0[(KERNEL_LINK_BASE >> 30) as usize] =
                KERNEL_LINK_BASE | PTE_VALID | PTE_AF | PTE_SH_INNER | PTE_AP_KERNEL_RW | PTE_UXN;
        }
        Self {
            root_phys,
            table_pages: alloc::vec![root_phys, kernel_l1_phys],
            guard_pages: Vec::new(),
            owned_pages: Vec::new(),
            regions: Vec::new(),
        }
    }

    /// Physical address suitable for TTBR0_EL1.
    pub fn root_phys(&self) -> u64 {
        self.root_phys
    }

    /// Whether a page was deliberately reserved as a downward-stack guard.
    pub fn is_stack_guard_page(&self, address: u64) -> bool {
        let page = address & !(PAGE_SIZE - 1);
        self.guard_pages.contains(&page)
    }

    /// Map a user page. User accessibility is enforced regardless of the
    /// caller-provided permissions.
    pub fn map_user_page(
        &mut self,
        virt: u64,
        phys: u64,
        permissions: PagePermissions,
    ) -> Result<(), PageMapError> {
        if virt == 0 || virt >= USER_ADDRESS_LIMIT {
            return Err(PageMapError::OutsideUserWindow);
        }
        if virt % PAGE_SIZE != 0 || phys % PAGE_SIZE != 0 {
            return Err(PageMapError::UnalignedAddress);
        }

        let l0 = page_table_at(self.root_phys);
        let l0_index = ((virt >> 39) & 0x1FF) as usize;
        let l1 = self.ensure_next_table(l0, l0_index)?;
        let l1_index = ((virt >> 30) & 0x1FF) as usize;
        let l2 = self.ensure_next_table(l1, l1_index)?;
        let l2_index = ((virt >> 21) & 0x1FF) as usize;
        let l3 = self.ensure_next_table(l2, l2_index)?;
        let l3_index = ((virt >> 12) & 0x1FF) as usize;

        if unsafe { (*l3).0[l3_index] } & PTE_VALID != 0 {
            return Err(PageMapError::AlreadyMapped);
        }

        unsafe {
            (*l3).0[l3_index] =
                (phys & PTE_ADDRESS_MASK) | leaf_attributes(permissions | PagePermissions::USER);
        }
        Ok(())
    }

    /// Allocate, map, and retain one user page until it is explicitly unmapped
    /// or the address space is destroyed.
    pub fn allocate_user_page(
        &mut self,
        virt: u64,
        permissions: PagePermissions,
    ) -> Result<u64, PageMapError> {
        let phys = crate::executive::mm::phys::alloc_page();
        unsafe {
            core::ptr::write_bytes(phys_to_direct_map(phys) as *mut u8, 0, PAGE_SIZE as usize)
        };
        if let Err(error) = self.map_user_page(virt, phys, permissions) {
            crate::executive::mm::phys::free_page(phys);
            return Err(error);
        }
        self.owned_pages.push(OwnedUserPage { virt, phys });
        Ok(phys)
    }

    /// Allocate zeroed, read/write EL0 memory at the first process-local free
    /// range. Released regions are eligible for reuse.
    pub fn allocate_user_region(&mut self, size: usize) -> Result<u64, PageMapError> {
        let page_count = user_page_count(size)?;
        let base = self.find_free_user_region(page_count)?;
        self.allocate_user_region_pages(base, page_count)
    }

    /// Allocate zeroed, read/write EL0 memory at an exact requested base.
    ///
    /// Fixed regions are limited to the dynamic window so callers cannot
    /// claim image, stack, or bootstrap mappings even when an address is not
    /// currently present in the process page tables.
    pub fn allocate_user_region_at(&mut self, base: u64, size: usize) -> Result<u64, PageMapError> {
        let page_count = user_page_count(size)?;
        self.validate_dynamic_region(base, page_count)?;
        self.allocate_user_region_pages(base, page_count)
    }

    fn allocate_user_region_pages(
        &mut self,
        base: u64,
        page_count: usize,
    ) -> Result<u64, PageMapError> {
        self.validate_dynamic_region(base, page_count)?;

        for index in 0..page_count {
            let virt = base + index as u64 * PAGE_SIZE;
            if let Err(error) = self.allocate_user_page(virt, PagePermissions::WRITE) {
                self.release_user_pages(base, index);
                return Err(error);
            }
        }

        self.regions.push(OwnedUserRegion { base, page_count });
        Ok(base)
    }

    fn find_free_user_region(&self, page_count: usize) -> Result<u64, PageMapError> {
        let mapped_size = (page_count as u64)
            .checked_mul(PAGE_SIZE)
            .ok_or(PageMapError::InvalidRange)?;
        let mut candidate = USER_DYNAMIC_BASE;

        loop {
            let end = candidate
                .checked_add(mapped_size)
                .ok_or(PageMapError::InvalidRange)?;
            if end > USER_ADDRESS_LIMIT {
                return Err(PageMapError::OutsideUserWindow);
            }

            let mut next_candidate = None;
            for region in &self.regions {
                let region_end = region
                    .base
                    .checked_add(region.page_count as u64 * PAGE_SIZE)
                    .ok_or(PageMapError::InvalidRange)?;
                if candidate < region_end && region.base < end {
                    next_candidate = Some(
                        next_candidate.map_or(region_end, |previous: u64| previous.max(region_end)),
                    );
                }
            }
            for page in &self.owned_pages {
                let page_end = page.virt + PAGE_SIZE;
                if candidate < page_end && page.virt < end {
                    next_candidate = Some(
                        next_candidate.map_or(page_end, |previous: u64| previous.max(page_end)),
                    );
                }
            }

            match next_candidate {
                Some(next) => candidate = next,
                None => return Ok(candidate),
            }
        }
    }

    fn validate_dynamic_region(&self, base: u64, page_count: usize) -> Result<(), PageMapError> {
        if base < USER_DYNAMIC_BASE || base % PAGE_SIZE != 0 {
            return Err(PageMapError::InvalidRange);
        }
        let mapped_size = (page_count as u64)
            .checked_mul(PAGE_SIZE)
            .ok_or(PageMapError::InvalidRange)?;
        let end = base
            .checked_add(mapped_size)
            .ok_or(PageMapError::InvalidRange)?;
        if end > USER_ADDRESS_LIMIT {
            return Err(PageMapError::OutsideUserWindow);
        }
        if self.regions.iter().any(|region| {
            base < region.base + region.page_count as u64 * PAGE_SIZE && region.base < end
        }) || self
            .owned_pages
            .iter()
            .any(|page| base <= page.virt && page.virt < end)
        {
            return Err(PageMapError::AlreadyMapped);
        }
        Ok(())
    }

    /// Release a complete process-owned user-memory region.
    ///
    /// The caller must supply exactly the base and size returned by
    /// [`allocate_user_region`].
    pub fn free_user_region(&mut self, base: u64, size: usize) -> Result<(), PageMapError> {
        let page_count = user_page_count(size)?;
        let Some(region_index) = self
            .regions
            .iter()
            .position(|region| region.base == base && region.page_count == page_count)
        else {
            return Err(PageMapError::NotMapped);
        };

        self.release_user_pages(base, page_count);
        self.regions.swap_remove(region_index);
        Ok(())
    }

    /// Remove a user page mapping and return its physical backing page.
    /// Empty intermediate tables remain owned for reuse until this address
    /// space is dropped.
    pub fn unmap_user_page(&mut self, virt: u64) -> Result<u64, PageMapError> {
        if virt == 0 || virt >= USER_ADDRESS_LIMIT {
            return Err(PageMapError::OutsideUserWindow);
        }
        if virt % PAGE_SIZE != 0 {
            return Err(PageMapError::UnalignedAddress);
        }

        let l0 = page_table_at(self.root_phys);
        let l0_index = ((virt >> 39) & 0x1FF) as usize;
        let l1 = unsafe { existing_next_table((*l0).0[l0_index]) }?;
        let l1_index = ((virt >> 30) & 0x1FF) as usize;
        let l2 = unsafe { existing_next_table((*l1).0[l1_index]) }?;
        let l2_index = ((virt >> 21) & 0x1FF) as usize;
        let l3 = unsafe { existing_next_table((*l2).0[l2_index]) }?;
        let l3_index = ((virt >> 12) & 0x1FF) as usize;
        let entry = unsafe { (*l3).0[l3_index] };
        if entry & PTE_VALID == 0 {
            return Err(PageMapError::NotMapped);
        }

        unsafe { (*l3).0[l3_index] = 0 };
        let phys = entry & PTE_ADDRESS_MASK;
        if let Some(index) = self
            .owned_pages
            .iter()
            .position(|page| page.virt == virt && page.phys == phys)
        {
            self.owned_pages.swap_remove(index);
        }
        Ok(phys)
    }

    /// Allocate and map a downward-growing EL0 stack.
    ///
    /// The page immediately below `mapped_base` is intentionally absent from
    /// TTBR0. A later EL0 fault handler can identify that address as a clean
    /// stack-overflow fault instead of silently growing into adjacent memory.
    pub fn map_user_stack(
        &mut self,
        stack_top: u64,
        page_count: usize,
    ) -> Result<UserStack, PageMapError> {
        if page_count == 0 || stack_top % PAGE_SIZE != 0 || stack_top > USER_ADDRESS_LIMIT {
            return Err(PageMapError::InvalidRange);
        }

        let mapped_size = (page_count as u64)
            .checked_mul(PAGE_SIZE)
            .ok_or(PageMapError::InvalidRange)?;
        let reservation_size = mapped_size
            .checked_add(PAGE_SIZE)
            .ok_or(PageMapError::InvalidRange)?;
        let guard_page = stack_top
            .checked_sub(reservation_size)
            .ok_or(PageMapError::InvalidRange)?;
        if guard_page == 0 || guard_page >= USER_ADDRESS_LIMIT {
            return Err(PageMapError::OutsideUserWindow);
        }
        let mapped_base = guard_page + PAGE_SIZE;

        let stack = UserStack {
            guard_page,
            mapped_base,
            top: stack_top,
            page_count,
        };

        for index in 0..page_count {
            let virt = mapped_base + index as u64 * PAGE_SIZE;
            if let Err(error) = self.allocate_user_page(virt, PagePermissions::WRITE) {
                self.release_mapped_stack_pages(stack, index);
                return Err(error);
            }
        }

        self.guard_pages.push(guard_page);
        Ok(stack)
    }

    /// Unmap and free every stack page. The guard page stays unmapped.
    pub fn unmap_user_stack(&mut self, stack: UserStack) -> Result<(), PageMapError> {
        if stack.page_count == 0
            || stack.mapped_base != stack.guard_page + PAGE_SIZE
            || stack.top != stack.mapped_base + stack.page_count as u64 * PAGE_SIZE
        {
            return Err(PageMapError::InvalidRange);
        }

        let Some(guard_index) = self
            .guard_pages
            .iter()
            .position(|&guard_page| guard_page == stack.guard_page)
        else {
            return Err(PageMapError::InvalidRange);
        };

        self.release_mapped_stack_pages(stack, stack.page_count);
        self.guard_pages.swap_remove(guard_index);
        Ok(())
    }

    /// Return the mapped physical address, including the supplied page offset.
    pub fn translate(&self, virt: u64) -> Option<u64> {
        let entry = self.page_entry(virt)?;
        Some((entry & PTE_ADDRESS_MASK) | (virt & (PAGE_SIZE - 1)))
    }

    /// Copy bytes from mappings which are explicitly accessible to EL0.
    ///
    /// The copy uses the direct physical map rather than dereferencing an
    /// untrusted user virtual address in EL1.
    pub fn copy_from_user(&self, source: u64, destination: &mut [u8]) -> Result<(), PageMapError> {
        if destination.is_empty() {
            return Ok(());
        }
        self.validate_user_range(source, destination.len(), false)?;

        let mut copied = 0usize;
        while copied < destination.len() {
            let user_address = source + copied as u64;
            let phys = self
                .translate_user(user_address)
                .ok_or(PageMapError::NotMapped)?;
            let page_remaining = (PAGE_SIZE - (user_address & (PAGE_SIZE - 1))) as usize;
            let copy_len = core::cmp::min(destination.len() - copied, page_remaining);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_to_direct_map(phys) as *const u8,
                    destination.as_mut_ptr().add(copied),
                    copy_len,
                );
            }
            copied += copy_len;
        }
        Ok(())
    }

    /// Copy bytes to explicitly writable EL0 mappings.
    ///
    /// The entire destination range is validated before the first write, so
    /// an invalid later page cannot leave an externally visible partial copy.
    pub fn copy_to_user(&self, destination: u64, source: &[u8]) -> Result<(), PageMapError> {
        if source.is_empty() {
            return Ok(());
        }
        self.validate_user_range(destination, source.len(), true)?;

        let mut copied = 0usize;
        while copied < source.len() {
            let user_address = destination + copied as u64;
            let phys = self
                .translate_user_writable(user_address)
                .ok_or(PageMapError::NotMapped)?;
            let page_remaining = (PAGE_SIZE - (user_address & (PAGE_SIZE - 1))) as usize;
            let copy_len = core::cmp::min(source.len() - copied, page_remaining);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source.as_ptr().add(copied),
                    phys_to_direct_map(phys) as *mut u8,
                    copy_len,
                );
            }
            copied += copy_len;
        }
        Ok(())
    }

    /// Validate a writable EL0 range without copying data to it.
    pub fn validate_user_writable_range(
        &self,
        address: u64,
        length: usize,
    ) -> Result<(), PageMapError> {
        self.validate_user_range(address, length, true)
    }

    /// Validate an EL0 instruction pointer against an executable user mapping.
    pub fn validate_user_instruction_pointer(&self, address: u64) -> Result<(), PageMapError> {
        if address == 0 || address >= USER_ADDRESS_LIMIT {
            return Err(PageMapError::OutsideUserWindow);
        }
        let entry = self.page_entry(address).ok_or(PageMapError::NotMapped)?;
        let access = entry & PTE_AP_MASK;
        if (access != PTE_AP_USER_RW && access != PTE_AP_USER_RO) || entry & PTE_UXN != 0 {
            return Err(PageMapError::NotMapped);
        }
        Ok(())
    }

    /// Validate a 16-byte-aligned EL0 stack top with writable backing below it.
    pub fn validate_user_stack_pointer(&self, stack_top: u64) -> Result<(), PageMapError> {
        if stack_top == 0 || stack_top % 16 != 0 {
            return Err(PageMapError::InvalidRange);
        }
        let stack_byte = stack_top
            .checked_sub(1)
            .ok_or(PageMapError::OutsideUserWindow)?;
        if stack_byte >= USER_ADDRESS_LIMIT || self.translate_user_writable(stack_byte).is_none() {
            return Err(PageMapError::NotMapped);
        }
        Ok(())
    }

    fn validate_user_range(
        &self,
        address: u64,
        length: usize,
        requires_write: bool,
    ) -> Result<(), PageMapError> {
        let end = address
            .checked_add(length as u64)
            .ok_or(PageMapError::OutsideUserWindow)?;
        if address == 0 || end > USER_ADDRESS_LIMIT {
            return Err(PageMapError::OutsideUserWindow);
        }

        let mut page = address & !(PAGE_SIZE - 1);
        while page < end {
            let translated = if requires_write {
                self.translate_user_writable(page)
            } else {
                self.translate_user(page)
            };
            if translated.is_none() {
                return Err(PageMapError::NotMapped);
            }
            page = page
                .checked_add(PAGE_SIZE)
                .ok_or(PageMapError::OutsideUserWindow)?;
        }
        Ok(())
    }

    fn page_entry(&self, virt: u64) -> Option<u64> {
        if virt >= USER_ADDRESS_LIMIT {
            return None;
        }

        let l0 = page_table_at(self.root_phys) as *const PageTable;
        let l0_index = ((virt >> 39) & 0x1FF) as usize;
        let l1 = unsafe { existing_next_table((*l0).0[l0_index]).ok()? };
        let l1_index = ((virt >> 30) & 0x1FF) as usize;
        let l2 = unsafe { existing_next_table((*l1).0[l1_index]).ok()? };
        let l2_index = ((virt >> 21) & 0x1FF) as usize;
        let l3 = unsafe { existing_next_table((*l2).0[l2_index]).ok()? };
        let l3_index = ((virt >> 12) & 0x1FF) as usize;
        let entry = unsafe { (*l3).0[l3_index] };
        (entry & PTE_VALID != 0).then_some(entry)
    }

    fn translate_user(&self, virt: u64) -> Option<u64> {
        let entry = self.page_entry(virt)?;
        let access = entry & PTE_AP_MASK;
        if access != PTE_AP_USER_RW && access != PTE_AP_USER_RO {
            return None;
        }
        Some((entry & PTE_ADDRESS_MASK) | (virt & (PAGE_SIZE - 1)))
    }

    fn translate_user_writable(&self, virt: u64) -> Option<u64> {
        let entry = self.page_entry(virt)?;
        if entry & PTE_AP_MASK != PTE_AP_USER_RW {
            return None;
        }
        Some((entry & PTE_ADDRESS_MASK) | (virt & (PAGE_SIZE - 1)))
    }

    fn ensure_next_table(
        &mut self,
        parent: *mut PageTable,
        index: usize,
    ) -> Result<*mut PageTable, PageMapError> {
        let entry = unsafe { (*parent).0[index] };
        if entry & PTE_VALID != 0 {
            if entry & PTE_TABLE == 0 {
                return Err(PageMapError::ExistingBlockMapping);
            }
            return Ok(page_table_at(entry & PTE_ADDRESS_MASK));
        }

        let table_phys = crate::executive::mm::phys::alloc_page();
        unsafe {
            zero_page_table(table_phys);
            (*parent).0[index] = (table_phys & PTE_ADDRESS_MASK) | PTE_VALID | PTE_TABLE;
        }
        self.table_pages.push(table_phys);
        Ok(page_table_at(table_phys))
    }

    fn release_mapped_stack_pages(&mut self, stack: UserStack, page_count: usize) {
        self.release_user_pages(stack.mapped_base, page_count);
    }

    fn release_user_pages(&mut self, base: u64, page_count: usize) {
        for index in 0..page_count {
            let virt = base + index as u64 * PAGE_SIZE;
            if let Ok(phys) = self.unmap_user_page(virt) {
                crate::executive::mm::phys::free_page(phys);
            }
        }
    }
}

fn user_page_count(size: usize) -> Result<usize, PageMapError> {
    if size == 0 {
        return Err(PageMapError::InvalidRange);
    }
    let rounded_size = (size as u64)
        .checked_add(PAGE_SIZE - 1)
        .ok_or(PageMapError::InvalidRange)?
        & !(PAGE_SIZE - 1);
    usize::try_from(rounded_size / PAGE_SIZE).map_err(|_| PageMapError::InvalidRange)
}

impl Drop for UserAddressSpace {
    fn drop(&mut self) {
        for page in self.owned_pages.drain(..) {
            crate::executive::mm::phys::free_page(page.phys);
        }
        for &page in self.table_pages.iter().rev() {
            crate::executive::mm::phys::free_page(page);
        }
    }
}

/// Result of building the identity map, used for early boot diagnostics.
#[derive(Clone, Copy)]
pub struct BootstrapMappings {
    pub normal_blocks: usize,
    pub device_blocks: usize,
    pub kernel_high_alias: u64,
}

/// Builds and installs the initial translation regime.
///
/// The page tables are static kernel data, so this must run before enabling
/// process-local address spaces.  All currently reachable kernel data, UEFI
/// loader data, framebuffer memory, and QEMU RAM retain their identity mapping.
///
/// # Safety
///
/// The caller must invoke this once on the bootstrap CPU while interrupts are
/// masked.  The kernel must still execute at its physical linker address.
pub unsafe fn init(boot_info: &BootInfo) -> BootstrapMappings {
    let l0 = core::ptr::addr_of_mut!(TTBR0_L0);
    let l1 = core::ptr::addr_of_mut!(TTBR0_L1);
    let high_l0 = core::ptr::addr_of_mut!(TTBR1_L0);
    let high_l1 = core::ptr::addr_of_mut!(TTBR1_L1);

    unsafe {
        core::ptr::write(l0, PageTable::zero());
        core::ptr::write(l1, PageTable::zero());
        core::ptr::write(high_l0, PageTable::zero());
        core::ptr::write(high_l1, PageTable::zero());

        // L0 index 0 covers the first 512 GiB of the lower VA range.
        (*l0).0[0] = table_address(l1) | PTE_VALID | PTE_TABLE;

        // QEMU `virt` places the GIC and PL011 UART below 1 GiB.  Treat the
        // entire low window as Device-nGnRE; no CantayaOS executable data is
        // placed there.
        write_l1_block(
            l1,
            0,
            0,
            PTE_ATTR_DEVICE | PTE_AP_KERNEL_RW | PTE_AF | PTE_UXN | PTE_PXN,
        );

        let mut normal_blocks = 0;
        for descriptor in boot_info.memory_map.as_slice() {
            if is_bootstrap_ram(descriptor.ty) {
                normal_blocks += map_normal_range(
                    l1,
                    descriptor.phys_start,
                    descriptor.page_count.saturating_mul(PAGE_SIZE),
                );
            }
        }

        // These inputs are mandatory even if firmware emits an unusual map.
        normal_blocks += map_normal_range(l1, boot_info.kernel_phys_base, GIB);
        normal_blocks += map_normal_range(
            l1,
            boot_info.kernel_stack_top.saturating_sub(16 * PAGE_SIZE),
            16 * PAGE_SIZE,
        );
        if boot_info.framebuffer.base != 0 {
            normal_blocks += map_normal_range(l1, boot_info.framebuffer.base, GIB);
        }

        // Construct a valid high-half alias without moving execution there yet.
        // The linker still emits the kernel at 0x4000_0000, so both the lower
        // identity map and this alias remain available during the transition.
        (*high_l0).0[TTBR1_L0_INDEX] = table_address(high_l1) | PTE_VALID | PTE_TABLE;
        write_l1_block(
            high_l1,
            TTBR1_L1_INDEX,
            boot_info.kernel_phys_base,
            PTE_ATTR_NORMAL | PTE_SH_INNER | PTE_AP_KERNEL_RW | PTE_AF,
        );

        // The direct map is the permanent high-half access path for physical
        // memory, MMIO, and EL1 stacks after TTBR0 becomes process-local.
        // Its first GiB covers QEMU's peripheral window as Device-nGnRE; RAM
        // blocks retain Normal write-back attributes.
        write_l1_block(
            high_l1,
            DIRECT_MAP_L1_INDEX,
            0,
            PTE_ATTR_DEVICE | PTE_AP_KERNEL_RW | PTE_AF | PTE_UXN | PTE_PXN,
        );
        for descriptor in boot_info.memory_map.as_slice() {
            if is_bootstrap_ram(descriptor.ty) {
                map_direct_normal_range(
                    high_l1,
                    descriptor.phys_start,
                    descriptor.page_count.saturating_mul(PAGE_SIZE),
                );
            }
        }
        map_direct_normal_range(high_l1, boot_info.kernel_phys_base, GIB);
        map_direct_normal_range(
            high_l1,
            boot_info.kernel_stack_top.saturating_sub(16 * PAGE_SIZE),
            16 * PAGE_SIZE,
        );
        if boot_info.framebuffer.base != 0 {
            map_direct_normal_range(high_l1, boot_info.framebuffer.base, GIB);
        }

        // MAIR index 0: normal write-back, read/write allocate.
        // MAIR index 1: device-nGnRE for MMIO.
        MAIR_EL1.write(
            MAIR_EL1::Attr0_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
                + MAIR_EL1::Attr0_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc
                + MAIR_EL1::Attr1_Device::nonGathering_nonReordering_EarlyWriteAck,
        );

        // A Cortex-A57 supports a 40-bit physical address range.  TTBR1 maps
        // the kernel high-half alias while TTBR0 continues to serve the live
        // lower identity mapping and future user address spaces.
        TCR_EL1.write(
            TCR_EL1::IPS::Bits_40
                + TCR_EL1::TG0::KiB_4
                + TCR_EL1::TG1::KiB_4
                + TCR_EL1::T0SZ.val(16)
                + TCR_EL1::T1SZ.val(16)
                + TCR_EL1::EPD0::EnableTTBR0Walks
                + TCR_EL1::EPD1::EnableTTBR1Walks
                + TCR_EL1::SH0::Inner
                + TCR_EL1::SH1::Inner
                + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
                + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
                + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
                + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable,
        );
        TTBR0_EL1.set(table_address(l0));
        TTBR1_EL1.set(table_address(high_l0));

        barrier::dsb(barrier::ISHST);
        core::arch::asm!("tlbi vmalle1");
        barrier::dsb(barrier::ISH);
        barrier::isb(barrier::SY);

        SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);
        barrier::isb(barrier::SY);
        KERNEL_DIRECT_MAP_READY.store(true, Ordering::Release);

        BootstrapMappings {
            normal_blocks,
            device_blocks: 1,
            kernel_high_alias: KERNEL_HIGHER_HALF_BASE,
        }
    }
}

/// True once the stage-1 EL1 MMU is active.
pub fn is_enabled() -> bool {
    SCTLR_EL1.get() & 1 != 0
}

/// True only after CantayaOS has installed the TTBR1 direct physical map.
pub fn is_kernel_direct_map_ready() -> bool {
    KERNEL_DIRECT_MAP_READY.load(Ordering::Acquire)
}

/// Convert a physical address in the direct-map window into its TTBR1 alias.
///
/// The caller must use this only after [`init`] has enabled translation.
pub fn phys_to_direct_map(phys: u64) -> u64 {
    assert!(
        phys < DIRECT_MAP_PHYS_LIMIT,
        "physical address {:#x} is outside the direct-map window",
        phys,
    );
    KERNEL_DIRECT_MAP_BASE + phys
}

/// Returns whether `address` is in the permanent high-half direct-map window.
pub fn is_direct_map_address(address: u64) -> bool {
    (KERNEL_DIRECT_MAP_BASE..KERNEL_DIRECT_MAP_LIMIT).contains(&address)
}

#[inline]
fn page_table_at(phys: u64) -> *mut PageTable {
    phys_to_direct_map(phys) as *mut PageTable
}

/// Install a physical four-level root as TTBR0 and invalidate stale entries.
///
/// Callers must mask interrupts or otherwise prevent another context switch
/// while the translation base changes.
pub unsafe fn activate_ttbr0(root_phys: u64) {
    assert_eq!(root_phys % PAGE_SIZE, 0, "TTBR0 root must be page aligned");
    TTBR0_EL1.set(root_phys);
    barrier::dsb(barrier::ISHST);
    unsafe { core::arch::asm!("tlbi vmalle1") };
    barrier::dsb(barrier::ISH);
    barrier::isb(barrier::SY);
}

/// Restore the bootstrap identity root used by kernel-mode threads.
pub unsafe fn activate_bootstrap_ttbr0() {
    let root = core::ptr::addr_of!(TTBR0_L0) as u64;
    let root_phys = if is_higher_half_address(root) {
        KERNEL_LINK_BASE + (root - KERNEL_HIGHER_HALF_BASE)
    } else {
        root
    };
    unsafe { activate_ttbr0(root_phys) };
}

/// Translate a linked kernel address into its active TTBR1 alias.
///
/// The bootstrap maps one GiB of the image, which keeps this conversion
/// explicit and prevents unrelated lower addresses becoming branch targets.
pub fn to_higher_half_address(address: u64) -> Option<u64> {
    if !(KERNEL_LINK_BASE..KERNEL_LINK_BASE + GIB).contains(&address) {
        return None;
    }
    Some(KERNEL_HIGHER_HALF_BASE + (address - KERNEL_LINK_BASE))
}

/// Returns true when an address falls in the bootstrapped TTBR1 kernel alias.
pub fn is_higher_half_address(address: u64) -> bool {
    (KERNEL_HIGHER_HALF_BASE..KERNEL_HIGHER_HALF_BASE + GIB).contains(&address)
}

/// Move VBAR_EL1 to the higher-half exception-vector alias.
///
/// The lower vectors remain mapped through the bootstrap path until this call,
/// preserving a diagnosable fallback during the relocation transition.
pub unsafe fn relocate_exception_vectors_to_higher_half() -> u64 {
    let lower_vectors = VBAR_EL1.get();
    let higher_vectors = to_higher_half_address(lower_vectors)
        .expect("exception vectors are not inside the linked kernel image");
    VBAR_EL1.set(higher_vectors);
    barrier::isb(barrier::SY);
    higher_vectors
}

/// Map one 4 KiB page into the dynamic higher-half kernel region.
///
/// `allocate_table` supplies zeroable physical pages when an intermediate L2
/// or L3 page table is required.
///
/// # Safety
///
/// The MMU must already be active, `phys` must refer to allocated RAM, and no
/// caller may concurrently access the virtual address until this returns.
pub unsafe fn map_kernel_page<F>(
    virt: u64,
    phys: u64,
    permissions: PagePermissions,
    mut allocate_table: F,
) -> Result<(), PageMapError>
where
    F: FnMut() -> u64,
{
    if virt % PAGE_SIZE != 0 || phys % PAGE_SIZE != 0 {
        return Err(PageMapError::UnalignedAddress);
    }
    if !(KERNEL_DYNAMIC_BASE..KERNEL_DYNAMIC_LIMIT).contains(&virt) {
        return Err(PageMapError::OutsideKernelWindow);
    }

    let _guard = PAGE_TABLE_LOCK.lock();
    let high_l0 = core::ptr::addr_of_mut!(TTBR1_L0);

    let l0_entry = unsafe { (*high_l0).0[TTBR1_L0_INDEX] };
    if l0_entry & PTE_VALID == 0 || l0_entry & PTE_TABLE == 0 {
        return Err(PageMapError::InvalidTableHierarchy);
    }
    let l1 = page_table_at(l0_entry & PTE_ADDRESS_MASK);
    let l1_index = ((virt >> 30) & 0x1FF) as usize;
    let l2 = unsafe { ensure_next_table(l1, l1_index, &mut allocate_table) }?;
    let l2_index = ((virt >> 21) & 0x1FF) as usize;
    let l3 = unsafe { ensure_next_table(l2, l2_index, &mut allocate_table) }?;
    let l3_index = ((virt >> 12) & 0x1FF) as usize;

    if unsafe { (*l3).0[l3_index] } & PTE_VALID != 0 {
        return Err(PageMapError::AlreadyMapped);
    }

    unsafe {
        (*l3).0[l3_index] = (phys & PTE_ADDRESS_MASK) | leaf_attributes(permissions);
        flush_virtual_address(virt);
    }
    Ok(())
}

/// Remove one dynamic higher-half kernel mapping and return its physical page.
/// Empty intermediate page tables intentionally remain allocated for reuse.
///
/// # Safety
///
/// The caller must ensure no CPU is still using the virtual address.
pub unsafe fn unmap_kernel_page(virt: u64) -> Result<u64, PageMapError> {
    if virt % PAGE_SIZE != 0 {
        return Err(PageMapError::UnalignedAddress);
    }
    if !(KERNEL_DYNAMIC_BASE..KERNEL_DYNAMIC_LIMIT).contains(&virt) {
        return Err(PageMapError::OutsideKernelWindow);
    }

    let _guard = PAGE_TABLE_LOCK.lock();
    let high_l0 = core::ptr::addr_of_mut!(TTBR1_L0);
    let l0_entry = unsafe { (*high_l0).0[TTBR1_L0_INDEX] };
    let l1 = unsafe { existing_next_table(l0_entry) }?;
    let l1_index = ((virt >> 30) & 0x1FF) as usize;
    let l2 = unsafe { existing_next_table((*l1).0[l1_index]) }?;
    let l2_index = ((virt >> 21) & 0x1FF) as usize;
    let l3 = unsafe { existing_next_table((*l2).0[l2_index]) }?;
    let l3_index = ((virt >> 12) & 0x1FF) as usize;
    let entry = unsafe { (*l3).0[l3_index] };
    if entry & PTE_VALID == 0 {
        return Err(PageMapError::NotMapped);
    }

    unsafe {
        (*l3).0[l3_index] = 0;
        flush_virtual_address(virt);
    }
    Ok(entry & PTE_ADDRESS_MASK)
}

unsafe fn table_address(table: *const PageTable) -> u64 {
    table as u64
}

/// Zero one physical page-table page through the permanent TTBR1 direct map.
unsafe fn zero_page_table(phys: u64) {
    unsafe { core::ptr::write_bytes(phys_to_direct_map(phys) as *mut u8, 0, PAGE_SIZE as usize) };
}

fn leaf_attributes(permissions: PagePermissions) -> u64 {
    let mut attributes = PTE_VALID | PTE_TABLE | PTE_AF;
    let device = permissions.contains(PagePermissions::DEVICE);
    let user = permissions.contains(PagePermissions::USER);
    let execute = permissions.contains(PagePermissions::EXECUTE);

    attributes |= if device {
        PTE_ATTR_DEVICE
    } else {
        PTE_ATTR_NORMAL | PTE_SH_INNER
    };

    attributes |= match (
        permissions.contains(PagePermissions::USER),
        permissions.contains(PagePermissions::WRITE),
    ) {
        (false, true) => PTE_AP_KERNEL_RW,
        (false, false) => PTE_AP_KERNEL_RO,
        (true, true) => PTE_AP_USER_RW,
        (true, false) => PTE_AP_USER_RO,
    };

    if device {
        attributes |= PTE_UXN | PTE_PXN;
    } else if user {
        // EL0 code is never executable at EL1.
        attributes |= PTE_PXN;
        if !execute {
            attributes |= PTE_UXN;
        }
    } else {
        // Kernel mappings are never executable at EL0.
        attributes |= PTE_UXN;
        if !execute {
            attributes |= PTE_PXN;
        }
    }
    attributes
}

unsafe fn ensure_next_table<F>(
    parent: *mut PageTable,
    index: usize,
    allocate_table: &mut F,
) -> Result<*mut PageTable, PageMapError>
where
    F: FnMut() -> u64,
{
    let entry = unsafe { (*parent).0[index] };
    if entry & PTE_VALID != 0 {
        if entry & PTE_TABLE == 0 {
            return Err(PageMapError::ExistingBlockMapping);
        }
        return Ok(page_table_at(entry & PTE_ADDRESS_MASK));
    }

    let phys = allocate_table();
    if phys % PAGE_SIZE != 0 {
        return Err(PageMapError::UnalignedAddress);
    }
    let table = page_table_at(phys);
    unsafe {
        core::ptr::write_bytes(table.cast::<u8>(), 0, PAGE_SIZE as usize);
        (*parent).0[index] = (phys & PTE_ADDRESS_MASK) | PTE_VALID | PTE_TABLE;
    }
    Ok(table)
}

unsafe fn existing_next_table(entry: u64) -> Result<*mut PageTable, PageMapError> {
    if entry & PTE_VALID == 0 {
        return Err(PageMapError::NotMapped);
    }
    if entry & PTE_TABLE == 0 {
        return Err(PageMapError::ExistingBlockMapping);
    }
    Ok(page_table_at(entry & PTE_ADDRESS_MASK))
}

unsafe fn flush_virtual_address(virt: u64) {
    barrier::dsb(barrier::ISHST);
    unsafe { core::arch::asm!("tlbi vae1, {}", in(reg) virt >> 12) };
    barrier::dsb(barrier::ISH);
    barrier::isb(barrier::SY);
}

unsafe fn write_l1_block(table: *mut PageTable, index: usize, phys: u64, attributes: u64) {
    unsafe {
        (*table).0[index] = (phys & !(GIB - 1)) | PTE_VALID | attributes;
    }
}

unsafe fn map_normal_range(table: *mut PageTable, phys_start: u64, size: u64) -> usize {
    if size == 0 || phys_start >= IDENTITY_LIMIT {
        return 0;
    }

    let first = phys_start / GIB;
    let end = phys_start.saturating_add(size.saturating_sub(1));
    let last = (end / GIB).min(ENTRIES_PER_TABLE as u64 - 1);
    let mut mapped = 0;

    for block in first..=last {
        // Preserve the low MMIO window as Device memory.
        if block == 0 {
            continue;
        }
        unsafe {
            write_l1_block(
                table,
                block as usize,
                block * GIB,
                PTE_ATTR_NORMAL | PTE_SH_INNER | PTE_AP_KERNEL_RW | PTE_AF,
            );
        }
        mapped += 1;
    }

    mapped
}

unsafe fn map_direct_normal_range(table: *mut PageTable, phys_start: u64, size: u64) {
    if size == 0 || phys_start >= DIRECT_MAP_PHYS_LIMIT {
        return;
    }

    let first = phys_start / GIB;
    let end = phys_start.saturating_add(size.saturating_sub(1));
    let last = (end / GIB).min(DIRECT_MAP_PHYS_LIMIT / GIB - 1);
    for block in first..=last {
        // The direct-map low block mirrors QEMU MMIO as Device memory.
        if block == 0 {
            continue;
        }
        unsafe {
            write_l1_block(
                table,
                DIRECT_MAP_L1_INDEX + block as usize,
                block * GIB,
                PTE_ATTR_NORMAL | PTE_SH_INNER | PTE_AP_KERNEL_RW | PTE_AF,
            );
        }
    }
}

fn is_bootstrap_ram(ty: MemoryType) -> bool {
    matches!(
        ty,
        MemoryType::LoaderCode
            | MemoryType::LoaderData
            | MemoryType::BootServicesCode
            | MemoryType::BootServicesData
            | MemoryType::RuntimeServicesCode
            | MemoryType::RuntimeServicesData
            | MemoryType::Conventional
            | MemoryType::AcpiReclaim
            | MemoryType::AcpiNvs
            | MemoryType::PersistentMemory
            | MemoryType::KernelCode
            | MemoryType::KernelData
            | MemoryType::KernelStack
            | MemoryType::Framebuffer
    )
}
