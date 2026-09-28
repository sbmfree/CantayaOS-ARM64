//! CantayaOS UEFI Bootloader — entry point.
//!
//! Execution flow:
//!   1. UEFI firmware loads this PE32+ image and calls `efi_main`.
//!   2. We initialise the UEFI logger so early messages appear on the console.
//!   3. GOP framebuffer is located and its descriptor saved.
//!   4. `kernel.elf` is loaded from `\EFI\CantayaOS\kernel.elf` on the ESP.
//!   5. `init.elf` is retained in loader-data memory for the first EL0 process.
//!   6. A `BootInfo` block is allocated in loader-data memory.
//!   7. We call `exit_boot_services`, switch to the kernel's stack, and jump.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use alloc::vec::Vec;

use uefi::{
    boot::{AllocateType, MemoryType as UefiMemTy, PAGE_SIZE},
    prelude::*,
    proto::media::file::{Directory, File, FileAttribute, FileMode},
    proto::media::fs::SimpleFileSystem,
};

// Panic handler for the UEFI environment.
// Provided by uefi crate `panic_handler` feature.

// UEFI requires a global allocator.  Use `uefi`'s built-in pool allocator.
// Declared here; enabled by the `alloc` feature of the `uefi` crate.

use cantaya_shared::{BootInfo, BOOT_INFO_MAGIC};

mod elf;
mod framebuffer;
mod handoff;
mod memory;

/// UEFI entry point.  The `#[entry]` macro generates the PE32+ entry wrapper.
#[entry]
fn efi_main() -> Status {
    uefi::helpers::init().expect("Failed to init UEFI helpers");

    log::info!("CantayaOS UEFI Bootloader v{}", env!("CARGO_PKG_VERSION"));
    log::info!("(c) Oliwier Wieczorek — AArch64");

    // ── 1. Framebuffer ───────────────────────────────────────────────────────
    // GOP may not be available in headless mode — continue without it.
    let fb_info_opt = framebuffer::init();
    let fb_info = match fb_info_opt {
        Some(fb) => {
            log::info!(
                "Framebuffer: {}x{} stride={}",
                fb.width,
                fb.height,
                fb.stride
            );
            fb
        }
        None => {
            log::warn!("No GOP framebuffer — running headless (serial only)");
            cantaya_shared::FramebufferInfo {
                base: 0,
                width: 0,
                height: 0,
                stride: 0,
                bpp: 0,
                pixel_format: cantaya_shared::PixelFormat::Bgr,
            }
        }
    };

    // ── 2. Load kernel ELF ───────────────────────────────────────────────────
    let kernel_bytes = load_kernel_elf().expect("Failed to load kernel ELF");
    let (entry_point, kernel_phys_base) =
        elf::load_elf(&kernel_bytes).expect("Failed to map kernel ELF");
    log::info!(
        "Kernel entry @ {:#x}  base @ {:#x}",
        entry_point,
        kernel_phys_base
    );

    let init_elf = load_init_elf().expect("Failed to load init ELF");
    let init_elf_phys = init_elf.as_ptr() as u64;
    let init_elf_size = init_elf.len() as u64;
    log::info!("Init ELF @ {:#x} ({} bytes)", init_elf_phys, init_elf_size);

    // ── 3. Allocate kernel stack ─────────────────────────────────────────────
    const STACK_PAGES: usize = 16; // 64 KiB
    let stack_phys =
        uefi::boot::allocate_pages(AllocateType::AnyPages, UefiMemTy::LOADER_DATA, STACK_PAGES)
            .expect("Failed to allocate kernel stack");
    let kernel_stack_top = stack_phys.as_ptr() as u64 + (STACK_PAGES as u64 * PAGE_SIZE as u64);

    // ── 4. Collect memory map & exit boot services ───────────────────────────
    // Sizing the memory map buffer.
    let _mem_map_size_hint = 64 * 256; // 256 entries worst-case
                                       // We pass a zeroed placeholder map until exit_boot_services provides the real one.
    let mut boot_info = BootInfo {
        magic: BOOT_INFO_MAGIC,
        version: 3,
        _reserved: 0,
        framebuffer: fb_info,
        memory_map: cantaya_shared::MemoryMap::new(),
        rsdp: 0,
        kernel_phys_base,
        kernel_stack_top,
        init_elf_phys,
        init_elf_size,
    };

    // The kernel copies PT_LOAD segments into conventional RAM after the
    // handoff, so retain the UEFI pool allocation until then.
    core::mem::forget(init_elf);

    // Exit boot services — after this point no UEFI calls are valid.
    let mem_map = unsafe { uefi::boot::exit_boot_services(None) };
    boot_info.memory_map = memory::convert_map(&mem_map);

    // ── 5. Jump to kernel (no return) ────────────────────────────────────────
    unsafe { handoff::jump_to_kernel(entry_point, kernel_stack_top, &mut boot_info) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Open `\EFI\CantayaOS\kernel.elf` and read it into a Vec.
fn load_kernel_elf() -> Option<Vec<u8>> {
    load_os_file(cstr16!("kernel.elf"))
}

/// Open an ELF from `\EFI\CantayaOS` and retain its exact byte contents.
fn load_os_file(file_name: &uefi::CStr16) -> Option<Vec<u8>> {
    let sfs_handle = uefi::boot::get_handle_for_protocol::<SimpleFileSystem>().ok()?;
    let mut sfs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(sfs_handle).ok()?;
    let mut root: Directory = sfs.open_volume().ok()?;

    // Navigate: EFI -> CantayaOS -> requested ELF.
    let mut efi_dir = root
        .open(cstr16!("EFI"), FileMode::Read, FileAttribute::DIRECTORY)
        .ok()?
        .into_directory()?;
    let mut os_dir = efi_dir
        .open(
            cstr16!("CantayaOS"),
            FileMode::Read,
            FileAttribute::DIRECTORY,
        )
        .ok()?
        .into_directory()?;
    let file_handle = os_dir
        .open(file_name, FileMode::Read, FileAttribute::empty())
        .ok()?;
    let mut file = file_handle.into_regular_file()?;

    // Query file size via get_boxed_info
    let file_info = file
        .get_boxed_info::<uefi::proto::media::file::FileInfo>()
        .ok()?;
    let file_size = file_info.file_size() as usize;

    let mut buf = alloc::vec![0u8; file_size];
    file.read(&mut buf).ok()?;

    Some(buf)
}

/// Open the initial user-mode ELF from the ESP.
fn load_init_elf() -> Option<Vec<u8>> {
    load_os_file(cstr16!("init.elf"))
}
