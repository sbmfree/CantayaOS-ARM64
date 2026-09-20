//! CantayaOS kernel — main entry point.
//!
//! `kernel_main` is called by the assembly stub in `boot.s` with:
//!   x0 = physical pointer to `BootInfo`.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use cantaya_shared::{BootInfo, BOOT_INFO_MAGIC};

mod arch;
mod drivers;
mod executive;
mod hal;
mod syscall;

// ─────────────────────────────────────────────────────────────────────────────
// Global allocator — provided by mm::heap after init
// ─────────────────────────────────────────────────────────────────────────────

use linked_list_allocator::Heap;
use spin::Mutex as SpinMutex;

struct LockedKernelHeap(SpinMutex<Heap>);

#[global_allocator]
static ALLOCATOR: LockedKernelHeap = LockedKernelHeap(SpinMutex::new(Heap::empty()));

unsafe impl core::alloc::GlobalAlloc for LockedKernelHeap {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        self.0
            .lock()
            .allocate_first_fit(layout)
            .map(|nn| nn.as_ptr())
            .unwrap_or(core::ptr::null_mut())
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        if let Some(nn) = core::ptr::NonNull::new(ptr) {
            unsafe { self.0.lock().deallocate(nn, layout) };
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Panic handler
// ─────────────────────────────────────────────────────────────────────────────

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Try to print via UART; if UART not yet init'd this is a no-op.
    log::error!("KERNEL PANIC: {}", info);
    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kernel entry
// ─────────────────────────────────────────────────────────────────────────────

/// Called by `boot.s` — x0 = &BootInfo.
#[no_mangle]
pub extern "C" fn kernel_main(boot_info_ptr: *const BootInfo) -> ! {
    // ── Validate BootInfo ────────────────────────────────────────────────
    // SAFETY: pointer comes from the bootloader; we validate the magic.
    let boot_info = unsafe {
        assert!(!boot_info_ptr.is_null(), "BootInfo pointer is null");
        &*boot_info_ptr
    };
    assert_eq!(
        boot_info.magic, BOOT_INFO_MAGIC,
        "BootInfo magic mismatch — bootloader/kernel version skew"
    );

    // ── AArch64 hardware init (UART + MMU + exceptions) ──────────────────
    // arch::init also installs the UART logger before changing translation.
    arch::init(boot_info);

    // Keep this bootstrap entry low. The remainder executes through TTBR1,
    // leaving TTBR0 ready for its future process-address-space role.
    unsafe { arch::enter_higher_half(kernel_main_higher_half, boot_info_ptr) }
}

/// Main kernel initialization after relocating execution to the TTBR1 alias.
#[no_mangle]
pub extern "C" fn kernel_main_higher_half(boot_info_ptr: *const BootInfo) -> ! {
    let boot_info = unsafe {
        assert!(
            !boot_info_ptr.is_null(),
            "BootInfo pointer is null after TTBR1 branch"
        );
        &*boot_info_ptr
    };
    let high_pc = arch::activate_higher_half_execution();

    log::info!(
        "CantayaOS v{} (CantayaTech) [AArch64] high_pc={:#x}",
        env!("CARGO_PKG_VERSION"),
        high_pc,
    );

    // ── Memory manager ───────────────────────────────────────────────────
    executive::mm::init(boot_info);
    log::info!("Memory manager ready");
    verify_kernel_virtual_mapping();
    verify_user_address_space();
    verify_user_stack_guard();
    verify_fault_decoder();

    // ── HAL peripherals ──────────────────────────────────────────────────
    hal::gic::init();
    hal::timer::init();
    log::info!("GIC + timer initialised");

    // ── Framebuffer console ──────────────────────────────────────────────
    if hal::framebuffer::init(&boot_info.framebuffer) {
        log::info!("Framebuffer console active");
    } else {
        log::warn!("No usable framebuffer — continuing on UART console");
    }

    // ── NT Executive subsystems ──────────────────────────────────────────
    executive::ob::init();
    drivers::virtio_blk::init();
    executive::io::init();
    executive::ps::init(boot_info);
    log::info!("NT Executive initialised");
    // Keyboard: use VirtIO (-device virtio-keyboard-pci) — PL050 not on QEMU virt

    // ── Start scheduler ──────────────────────────────────────────────────
    // Keep IRQs masked until `idle_loop` performs its first context switch.
    // A timer interrupt before `CURRENT` is set would switch away from the
    // GIC handler before it can acknowledge the interrupt.
    log::info!("Starting scheduler");

    // The scheduler takes over from here; the initial thread context restores
    // its own IRQ state once `CURRENT` has been established.
    executive::ps::scheduler::idle_loop()
}

/// Exercise the live high-half map/unmap path before drivers and processes use
/// it.  This makes a page-table regression fail immediately at boot.
fn verify_kernel_virtual_mapping() {
    const PROBE_VALUE: u64 = 0x4341_4E54_4159_414F;

    let virt = executive::mm::virt::mm_allocate_kernel_virtual(core::mem::size_of::<u64>())
        .expect("failed to allocate high-half kernel virtual probe page");
    unsafe {
        core::ptr::write_volatile(virt as *mut u64, PROBE_VALUE);
        assert_eq!(
            core::ptr::read_volatile(virt as *const u64),
            PROBE_VALUE,
            "high-half kernel virtual mapping probe mismatch",
        );
    }
    assert!(
        executive::mm::virt::mm_free_kernel_virtual(virt, core::mem::size_of::<u64>()),
        "failed to release high-half kernel virtual probe page",
    );
    log::info!("Mm high-half map/unmap probe passed at {:#x}", virt);
}

/// Verify an inactive process TTBR0 root without disturbing the live kernel
/// mapping. This exercises user permissions, table ownership, translation, and
/// cleanup before EL0 execution is introduced.
fn verify_user_address_space() {
    const USER_PROBE_VA: u64 = 0x0000_0001_0040_0000;
    let free_before = executive::mm::phys::free_pages();
    let process = executive::ps::process::EProcess::new_user_process(USER_PROBE_VA);
    let data_page = executive::mm::phys::alloc_page();

    process
        .with_user_address_space(|address_space| {
            address_space
                .map_user_page(
                    USER_PROBE_VA,
                    data_page,
                    arch::mmu::PagePermissions::WRITE | arch::mmu::PagePermissions::EXECUTE,
                )
                .expect("failed to map user address-space probe page");
            assert_eq!(
                address_space.translate(USER_PROBE_VA + 0x128),
                Some(data_page + 0x128),
                "user address-space translation probe mismatch",
            );
            assert_eq!(
                address_space
                    .unmap_user_page(USER_PROBE_VA)
                    .expect("failed to unmap user address-space probe page"),
                data_page,
            );
            assert!(
                address_space.translate(USER_PROBE_VA).is_none(),
                "unmapped user probe page still translates",
            );
            assert!(
                address_space.translate(0x0900_0000).is_none(),
                "user TTBR0 root inherited the UART identity mapping",
            );
        })
        .expect("user process has no address space");

    executive::mm::phys::free_page(data_page);
    let root = process.page_table_base;
    drop(process);
    assert_eq!(
        executive::mm::phys::free_pages(),
        free_before,
        "user address-space table pages leaked",
    );
    log::info!("Mm user address-space probe passed: TTBR0 root={:#x}", root);
}

/// Verify that future EL0 stacks have mapped pages above a reliable guard page
/// and that their physical backing is reclaimed with the process root.
fn verify_user_stack_guard() {
    const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
    const STACK_PAGES: usize = 4;

    let free_before = executive::mm::phys::free_pages();
    let process = executive::ps::process::EProcess::new_user_process(0x400000);
    process
        .with_user_address_space(|address_space| {
            let stack = address_space
                .map_user_stack(USER_STACK_TOP, STACK_PAGES)
                .expect("failed to map user stack probe");
            assert!(
                address_space.translate(stack.guard_page).is_none(),
                "user stack guard page unexpectedly maps",
            );
            assert!(
                address_space.is_stack_guard_page(stack.guard_page),
                "user stack guard page is not registered for fault handling",
            );
            assert!(
                address_space.translate(stack.mapped_base).is_some(),
                "lowest user stack page is not mapped",
            );
            assert!(
                address_space.translate(stack.top - 1).is_some(),
                "top user stack page is not mapped",
            );
            address_space
                .unmap_user_stack(stack)
                .expect("failed to unmap user stack probe");
            assert!(
                address_space.translate(stack.mapped_base).is_none(),
                "unmapped user stack still translates",
            );
            assert!(
                !address_space.is_stack_guard_page(stack.guard_page),
                "released user stack guard remains registered",
            );
        })
        .expect("user process has no address space for stack probe");

    drop(process);
    assert_eq!(
        executive::mm::phys::free_pages(),
        free_before,
        "user stack or table pages leaked",
    );
    log::info!("Mm user stack guard probe passed: {} page(s)", STACK_PAGES);
}

/// Exercise the ESR decoder with a level-3 EL0 write-permission fault.
fn verify_fault_decoder() {
    const EL0_DATA_ABORT_PERMISSION_WRITE: u64 = (0x24 << 26) | (1 << 6) | 0x0F;
    let fault = arch::exceptions::decode_fault(
        arch::exceptions::ExceptionSource::El0,
        EL0_DATA_ABORT_PERMISSION_WRITE,
        0x400000,
        0x401000,
    );
    assert_eq!(
        fault.class,
        arch::exceptions::FaultClass::DataAbort,
        "fault decoder did not classify EL0 data abort",
    );
    assert_eq!(
        fault.status,
        arch::exceptions::AbortStatus::Permission(3),
        "fault decoder did not classify level-3 permission fault",
    );
    assert!(fault.is_write, "fault decoder lost data-abort write bit");
    log::info!("AArch64 fault decoder probe passed");
}
