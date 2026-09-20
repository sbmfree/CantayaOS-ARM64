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
    log::trace!("SVC: {:?} (num={:#x})", number, syscall_num);

    let result: u64 = match number {
        NtSyscallNumber::NtWriteFile => nt::sys_write_file(regs),
        NtSyscallNumber::NtReadFile => nt::sys_read_file(regs),
        NtSyscallNumber::NtCreateProcess => nt::sys_create_process(regs),
        NtSyscallNumber::NtCreateThread => nt::sys_create_thread(regs),
        NtSyscallNumber::NtWaitForSingleObject => nt::sys_wait_for_single_object(regs),
        NtSyscallNumber::NtAllocateVirtual => nt::sys_allocate_virtual(regs),
        NtSyscallNumber::NtFreeVirtual => nt::sys_free_virtual(regs),
        NtSyscallNumber::NtCreateFile => nt::sys_create_file(regs),
        NtSyscallNumber::NtClose => nt::sys_close(regs),
        NtSyscallNumber::NtQuerySystemInfo => nt::sys_query_system_info(regs),
        NtSyscallNumber::NtTerminateProcess => nt::sys_terminate_process(regs),
        NtSyscallNumber::NtTerminateThread => nt::sys_terminate_thread(regs),
        NtSyscallNumber::Unknown => {
            log::warn!("Unknown syscall {:#x}", syscall_num);
            0xC000_0001u64 // STATUS_UNSUCCESSFUL
        }
    };

    // Store return value back into x0 in the saved register frame
    regs.x[0] = result;
}
