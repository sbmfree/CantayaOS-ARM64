//! Final handoff: switch to kernel stack and jump to kernel entry.
//!
//! # Safety
//!
//! This function never returns.  It switches the stack pointer to
//! `kernel_stack_top` and calls the kernel entry point with a pointer to
//! `boot_info` in `x0`.  The caller must guarantee that the memory map has
//! already been retrieved and boot services have exited.

use cantaya_shared::BootInfo;

/// Jump to the kernel.  Never returns.
///
/// AArch64 calling convention:
///   x0  — first argument (pointer to BootInfo)
///   sp  — new kernel stack pointer
///   pc  — kernel entry point
#[unsafe(naked)]
pub unsafe extern "C" fn jump_to_kernel(
    entry: u64,
    kernel_stack_top: u64,
    boot_info: *mut BootInfo,
) -> ! {
    // AArch64 calling convention on entry to this function:
    //   x0 = entry (kernel entry point VA)
    //   x1 = kernel_stack_top
    //   x2 = boot_info pointer
    //
    // We need to call kernel_main(boot_info), so:
    //   1. Switch SP to kernel stack
    //   2. Put boot_info in x0 (first argument)
    //   3. Branch to entry
    unsafe {
        core::arch::naked_asm!(
            "msr  SPSel, #1", // select the EL1 stack before replacing SP
            "mov  sp, x1",    // switch to kernel stack (entry still in x0)
            "mov  x1, x0",    // save entry address to x1
            "mov  x0, x2",    // x0 = boot_info (kernel_main's first arg)
            "br   x1",        // jump to kernel entry point
        );
    }
}
