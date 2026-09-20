//! `arch` — AArch64-specific initialisation.

mod boot_asm;
pub mod exceptions;
pub mod mmu;

use crate::hal;
use cantaya_shared::BootInfo;

/// Perform all AArch64 hardware initialisation before the executive starts.
pub fn init(boot_info: &BootInfo) {
    // UART and the logger must be available before enabling translation so a
    // fault can be diagnosed from the serial console.
    unsafe { hal::uart::init() };
    hal::uart::init_logger();
    log::info!("AArch64 arch init");

    let mappings = unsafe { mmu::init(boot_info) };
    log::info!(
        "MMU enabled: {} RAM block(s), {} device block(s), kernel alias={:#x}",
        mappings.normal_blocks,
        mappings.device_blocks,
        mappings.kernel_high_alias,
    );
    assert!(mmu::is_enabled(), "MMU enable bit was not retained");
}

/// Branch into a function through the TTBR1 higher-half kernel alias.
///
/// # Safety
///
/// `entry` must be part of the linked kernel image, while `boot_info` must
/// remain readable through the transitional TTBR0 identity mapping.
pub unsafe fn enter_higher_half(
    entry: extern "C" fn(*const BootInfo) -> !,
    boot_info: *const BootInfo,
) -> ! {
    let target = mmu::to_higher_half_address(entry as usize as u64)
        .expect("higher-half target is outside the linked kernel image");

    unsafe {
        core::arch::asm!(
            "br {target}",
            target = in(reg) target,
            in("x0") boot_info,
            options(noreturn),
        );
    }
}

/// Assert execution through TTBR1 and move exception vectors to that alias.
pub fn activate_higher_half_execution() -> u64 {
    let pc: u64;
    unsafe {
        core::arch::asm!(
            "adr {pc}, .",
            pc = out(reg) pc,
            options(nomem, nostack),
        );
    }
    assert!(
        mmu::is_higher_half_address(pc),
        "kernel did not branch through TTBR1: PC={:#x}",
        pc,
    );

    let vectors = unsafe { mmu::relocate_exception_vectors_to_higher_half() };
    log::info!(
        "Kernel executing through TTBR1: PC={:#x}, VBAR={:#x}",
        pc,
        vectors,
    );
    pc
}
