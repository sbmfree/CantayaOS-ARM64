//! Syscall layer — SVC #0 dispatch.
//!
//! AArch64 calling convention for CantayaOS syscalls (mirrors Linux AArch64):
//!   x8  = syscall number
//!   x0–x5 = arguments (up to 6)
//!   x0  = return value
//!
//! The SVC handler in `boot.s` calls `syscall_dispatch(number, saved_regs)`.

pub mod nt;

use nt::NtSyscallNumber;

/// Raw saved register file passed from the SVC exception handler.
/// The layout matches the `SAVE_CONTEXT` macro in `boot.s`.
#[repr(C)]
pub struct SavedRegs {
    pub x: [u64; 30], // x0–x29
    pub lr: u64,      // x30
    pub elr: u64,     // ELR_EL1 (return address to user)
    pub spsr: u64,    // SPSR_EL1
}

/// Called by the SVC exception handler in `boot.s`.
///
/// `syscall_num` is the value of x8 from the saved register frame.
/// `regs` points to the full saved register context on the kernel stack.
#[no_mangle]
pub extern "C" fn syscall_dispatch(syscall_num: u64, regs: *mut SavedRegs) {
    let regs = unsafe { &mut *regs };
    let number = NtSyscallNumber::from_u64(syscall_num);
    // Console reads, waits, and writes are hot terminal paths; tracing each
    // one would flood the serial console and perturb input pacing.
    if !matches!(
        number,
        NtSyscallNumber::NtReadFile
            | NtSyscallNumber::NtWriteFile
            | NtSyscallNumber::NtClearConsole
            | NtSyscallNumber::NtQueryDisplay
            | NtSyscallNumber::NtPresentDisplay
            | NtSyscallNumber::NtReadDesktopEvent
            | NtSyscallNumber::NtReadDesktopOutput
            | NtSyscallNumber::NtCreateWindow
            | NtSyscallNumber::NtPresentWindow
            | NtSyscallNumber::NtReadWindowEvent
            | NtSyscallNumber::NtWaitWindowEvent
            | NtSyscallNumber::NtCloseWindow
            | NtSyscallNumber::NtEnumerateWindows
            | NtSyscallNumber::NtCopyWindow
            | NtSyscallNumber::NtSendWindowEvent
            | NtSyscallNumber::NtResizeWindow
            | NtSyscallNumber::NtQueryWindow
            | NtSyscallNumber::NtAcknowledgeWindows
            | NtSyscallNumber::NtWaitForConsoleInput
    ) && !(number == NtSyscallNumber::NtQuerySystemInfo && crate::desktop::is_active())
    {
        log::trace!("SVC: {:?} (num={:#x})", number, syscall_num);
    }

    let result: u64 = match number {
        NtSyscallNumber::NtWriteFile => nt::sys_write_file(regs),
        NtSyscallNumber::NtReadFile => nt::sys_read_file(regs),
        NtSyscallNumber::NtWaitForConsoleInput => nt::sys_wait_for_console_input(regs),
        NtSyscallNumber::NtClearConsole => nt::sys_clear_console(regs),
        NtSyscallNumber::NtCreateProcess => nt::sys_create_process(regs),
        NtSyscallNumber::NtCreateThread => nt::sys_create_thread(regs),
        NtSyscallNumber::NtWaitForSingleObject => nt::sys_wait_for_single_object(regs),
        NtSyscallNumber::NtAllocateVirtual => nt::sys_allocate_virtual(regs),
        NtSyscallNumber::NtFreeVirtual => nt::sys_free_virtual(regs),
        NtSyscallNumber::NtCreateFile => nt::sys_create_file(regs),
        NtSyscallNumber::NtQueryRootDirectory => nt::sys_query_root_directory(regs),
        NtSyscallNumber::NtQueryDirectory => nt::sys_query_directory(regs),
        NtSyscallNumber::NtClose => nt::sys_close(regs),
        NtSyscallNumber::NtQuerySystemInfo => nt::sys_query_system_info(regs),
        NtSyscallNumber::NtTerminateProcess => nt::sys_terminate_process(regs),
        NtSyscallNumber::NtTerminateThread => nt::sys_terminate_thread(regs),
        NtSyscallNumber::NtQueryDisplay => crate::desktop::sys_query(regs),
        NtSyscallNumber::NtPresentDisplay => crate::desktop::sys_present(regs),
        NtSyscallNumber::NtReadDesktopEvent => crate::desktop::sys_read_event(regs),
        NtSyscallNumber::NtReadDesktopOutput => crate::desktop::sys_read_output(regs),
        NtSyscallNumber::NtCreateWindow => crate::windows::sys_create(regs),
        NtSyscallNumber::NtPresentWindow => crate::windows::sys_present(regs),
        NtSyscallNumber::NtReadWindowEvent => crate::windows::sys_read_event(regs),
        NtSyscallNumber::NtWaitWindowEvent => crate::windows::sys_wait_event(regs),
        NtSyscallNumber::NtCloseWindow => crate::windows::sys_close(regs),
        NtSyscallNumber::NtEnumerateWindows => crate::windows::sys_enumerate(regs),
        NtSyscallNumber::NtCopyWindow => crate::windows::sys_copy(regs),
        NtSyscallNumber::NtSendWindowEvent => crate::windows::sys_send_event(regs),
        NtSyscallNumber::NtResizeWindow => crate::windows::sys_resize(regs),
        NtSyscallNumber::NtQueryWindow => crate::windows::sys_query(regs),
        NtSyscallNumber::NtAcknowledgeWindows => crate::windows::sys_acknowledge(regs),
        NtSyscallNumber::Unknown => {
            log::warn!("Unknown syscall {:#x}", syscall_num);
            0xC000_0001u64 // STATUS_UNSUCCESSFUL
        }
    };

    // Store return value back into x0 in the saved register frame
    regs.x[0] = result;
}
