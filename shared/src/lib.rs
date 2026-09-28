//! cantaya-shared — types exchanged between the UEFI bootloader and the kernel.
//!
//! Both crates compile against this crate with `no_std`.  No heap allocations
//! are used; everything is flat, `#[repr(C)]` data that can be written by the
//! bootloader in physical memory and read by the kernel after paging is on.

#![no_std]

// ─────────────────────────────────────────────────────────────────────────────
// Framebuffer
// ─────────────────────────────────────────────────────────────────────────────

/// Pixel format reported by GOP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    Rgb = 0,
    Bgr = 1,
    Bitmask = 2,
    BltOnly = 3,
}

/// Linear framebuffer descriptor populated by the bootloader from GOP.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct FramebufferInfo {
    /// Physical base address of the framebuffer.
    pub base: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per scanline (stride).
    pub stride: u32,
    /// Bits per pixel.
    pub bpp: u32,
    pub pixel_format: PixelFormat,
}

// ─────────────────────────────────────────────────────────────────────────────
// Memory map
// ─────────────────────────────────────────────────────────────────────────────

/// Memory region type, mirroring UEFI EFI_MEMORY_TYPE values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MemoryType {
    Reserved = 0,
    LoaderCode = 1,
    LoaderData = 2,
    BootServicesCode = 3,
    BootServicesData = 4,
    RuntimeServicesCode = 5,
    RuntimeServicesData = 6,
    /// Free RAM the kernel may use.
    Conventional = 7,
    Unusual = 8,
    AcpiReclaim = 9,
    AcpiNvs = 10,
    MemoryMappedIo = 11,
    MemoryMappedIoPortSpace = 12,
    PalCode = 13,
    PersistentMemory = 14,
    KernelCode = 0x8000_0000,
    KernelData = 0x8000_0001,
    KernelStack = 0x8000_0002,
    Framebuffer = 0x8000_0003,
}

/// A single entry in the boot memory map.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct MemoryDescriptor {
    pub ty: MemoryType,
    /// 4 KiB page-aligned physical start address.
    pub phys_start: u64,
    /// Number of 4 KiB pages in this region.
    pub page_count: u64,
    /// UEFI attribute bitmask (EFI_MEMORY_*).
    pub attributes: u64,
}

/// Fixed-capacity memory map embedded in `BootInfo`.
///
/// 512 descriptors is enough for typical firmware + early kernel mappings.
pub const MAX_MEMORY_DESCRIPTORS: usize = 512;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct MemoryMap {
    pub entries: [MemoryDescriptor; MAX_MEMORY_DESCRIPTORS],
    pub count: usize,
}

impl MemoryMap {
    pub const fn new() -> Self {
        Self {
            entries: [MemoryDescriptor {
                ty: MemoryType::Reserved,
                phys_start: 0,
                page_count: 0,
                attributes: 0,
            }; MAX_MEMORY_DESCRIPTORS],
            count: 0,
        }
    }

    /// Returns the slice of valid entries.
    #[inline]
    pub fn as_slice(&self) -> &[MemoryDescriptor] {
        &self.entries[..self.count]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Boot information block
// ─────────────────────────────────────────────────────────────────────────────

/// Magic value written by the bootloader and verified by the kernel.
pub const BOOT_INFO_MAGIC: u64 = 0xC4_A7_4A_05_00_00_00_01;
/// Run the interactive EL0 console-input contract probe (smoke boot only).
pub const BOOT_FLAG_CONSOLE_INPUT_PROBE: u32 = 1;

/// Top-level structure passed from the bootloader to the kernel via `x0`.
///
/// Must remain `#[repr(C)]` forever — the kernel reads it before any Rust
/// abstractions are available.
#[derive(Debug, Clone, Copy)]
#[repr(C, align(16))]
pub struct BootInfo {
    pub magic: u64,
    pub version: u32,
    pub flags: u32,
    pub framebuffer: FramebufferInfo,
    pub memory_map: MemoryMap,
    /// Physical address of the ACPI 2.0 RSDP, or 0 if not found.
    pub rsdp: u64,
    /// Physical base address at which the kernel ELF was loaded.
    pub kernel_phys_base: u64,
    /// Physical address of the kernel stack top (grows down).
    pub kernel_stack_top: u64,
    /// Physical byte address of the retained initial user-process ELF.
    pub init_elf_phys: u64,
    /// Length in bytes of the retained initial user-process ELF.
    pub init_elf_size: u64,
}
