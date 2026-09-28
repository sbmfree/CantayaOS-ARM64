//! NT-style syscall numbers and stub implementations.
//!
//! Numbers are chosen to mirror Windows NT's AArch64 syscall table where
//! documented, enabling future binary-compatibility work.
//!
//! Status codes use NTSTATUS convention:
//!   0x0000_0000 = STATUS_SUCCESS
//!   0xC000_0001 = STATUS_UNSUCCESSFUL
//!   0xC000_0002 = STATUS_NOT_IMPLEMENTED
//!   0x0000_0103 = STATUS_PENDING

use super::SavedRegs;

const STATUS_ACCESS_VIOLATION: u64 = 0xC000_0005;
const STATUS_INVALID_HANDLE: u64 = 0xC000_0008;
const STATUS_INVALID_PARAMETER: u64 = 0xC000_000D;
const STATUS_ACCESS_DENIED: u64 = 0xC000_0022;
const STATUS_TIMEOUT: u64 = 0x0000_0102;
const STATUS_INVALID_IMAGE_FORMAT: u64 = 0xC000_007B;
const STATUS_NO_MEMORY: u64 = 0xC000_0017;
const MAX_WRITE_LENGTH: usize = 1024;
const CONSOLE_OUTPUT_HANDLE: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtSyscallNumber {
    NtCreateProcess = 0x004C,
    NtCreateThread = 0x004E,
    NtCreateFile = 0x0055,
    NtWaitForSingleObject = 0x0004,
    NtClose = 0x000F,
    NtReadFile = 0x0006,
    NtWriteFile = 0x0008,
    NtAllocateVirtual = 0x0015,
    NtFreeVirtual = 0x001B,
    NtQuerySystemInfo = 0x0036,
    NtTerminateProcess = 0x0029,
    NtTerminateThread = 0x0030,
    Unknown,
}

impl NtSyscallNumber {
    pub fn from_u64(v: u64) -> Self {
        match v {
            0x004C => Self::NtCreateProcess,
            0x004E => Self::NtCreateThread,
            0x0055 => Self::NtCreateFile,
            0x0004 => Self::NtWaitForSingleObject,
            0x000F => Self::NtClose,
            0x0006 => Self::NtReadFile,
            0x0008 => Self::NtWriteFile,
            0x0015 => Self::NtAllocateVirtual,
            0x001B => Self::NtFreeVirtual,
            0x0036 => Self::NtQuerySystemInfo,
            0x0029 => Self::NtTerminateProcess,
            0x0030 => Self::NtTerminateThread,
            _ => Self::Unknown,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stub implementations
//
// Each function receives the full saved register context.  Arguments are in
// regs.x[0]..regs.x[5] at the time of the SVC.
// ─────────────────────────────────────────────────────────────────────────────

pub fn sys_write_file(regs: &mut SavedRegs) -> u64 {
    // x0 = fixed console pseudo-handle (-1), x1 = text ptr, x2 = byte length.
    // This is not a generic file write and does not accept typed handles.
    if regs.x[0] != CONSOLE_OUTPUT_HANDLE {
        return STATUS_INVALID_HANDLE;
    }
    let user_buffer = regs.x[1];
    let Ok(len) = usize::try_from(regs.x[2]) else {
        return STATUS_INVALID_PARAMETER;
    };
    if user_buffer == 0 || !(1..=MAX_WRITE_LENGTH).contains(&len) {
        return STATUS_INVALID_PARAMETER;
    }

    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return STATUS_ACCESS_VIOLATION;
    };
    let mut copied = [0u8; MAX_WRITE_LENGTH];
    let copy_result = process.with_user_address_space(|address_space| {
        address_space.copy_from_user(user_buffer, &mut copied[..len])
    });
    match copy_result {
        Some(Ok(())) => {
            // The framebuffer currently has an ASCII glyph path and understands
            // only these terminal controls. Reject bytes the two sinks would
            // render differently before emitting any part of the write.
            if !copied[..len]
                .iter()
                .all(|&byte| matches!(byte, b'\x07' | b'\x08' | b'\r' | b'\n' | b' '..=b'~'))
            {
                return STATUS_INVALID_PARAMETER;
            }
            let Ok(text) = core::str::from_utf8(&copied[..len]) else {
                return STATUS_INVALID_PARAMETER;
            };
            crate::console::write(format_args!("{text}"));
            log::info!(
                "NtWriteFile copied {} byte(s) from validated EL0 memory to console",
                len
            );
            0
        }
        Some(Err(_)) | None => STATUS_ACCESS_VIOLATION,
    }
}

pub fn sys_read_file(_regs: &mut SavedRegs) -> u64 {
    0xC000_0002 // STATUS_NOT_IMPLEMENTED
}

pub fn sys_create_process(regs: &mut SavedRegs) -> u64 {
    // x0 = *process_handle, x1 = image source selector, x2 = child startup
    // argument. Sources are fixed kernel-owned images: the retained boot init
    // ELF or bounded CHILD.ELF from the retained FAT boot volume.
    if regs.x[0] == 0
        || !matches!(
            regs.x[1],
            crate::executive::ps::INITIAL_IMAGE_SOURCE
                | crate::executive::ps::FAT_CHILD_IMAGE_SOURCE
        )
    {
        return STATUS_INVALID_PARAMETER;
    }
    let source_name = match regs.x[1] {
        crate::executive::ps::INITIAL_IMAGE_SOURCE => "cached init ELF",
        crate::executive::ps::FAT_CHILD_IMAGE_SOURCE => "FAT CHILD.ELF",
        _ => unreachable!(),
    };

    let Some(parent) = crate::executive::ps::scheduler::current_process() else {
        return STATUS_ACCESS_VIOLATION;
    };
    let output_valid = parent.with_user_address_space(|address_space| {
        address_space.validate_user_writable_range(regs.x[0], core::mem::size_of::<u64>())
    });
    if !matches!(output_valid, Some(Ok(()))) {
        return STATUS_ACCESS_VIOLATION;
    }

    let Ok((child, thread)) =
        crate::executive::ps::create_process_from_source(regs.x[1], regs.x[2])
    else {
        return STATUS_INVALID_IMAGE_FORMAT;
    };
    let pid = child.pid.0;
    let Some(handle) = parent.insert_process_handle(child) else {
        crate::executive::ps::scheduler::discard_unstarted(thread, STATUS_NO_MEMORY as i32);
        return STATUS_NO_MEMORY;
    };
    let output = handle.to_le_bytes();
    let copied_output = parent
        .with_user_address_space(|address_space| address_space.copy_to_user(regs.x[0], &output));
    if !matches!(copied_output, Some(Ok(()))) {
        assert!(parent.close_handle(handle));
        crate::executive::ps::scheduler::discard_unstarted(thread, STATUS_ACCESS_VIOLATION as i32);
        return STATUS_ACCESS_VIOLATION;
    }

    crate::executive::ps::scheduler::enqueue(thread);
    log::info!(
        "NtCreateProcess created pid={} source={} handle={:#x}",
        pid,
        source_name,
        handle,
    );
    0
}

pub fn sys_create_thread(regs: &mut SavedRegs) -> u64 {
    // x0 = *thread_handle, x1 = user entry, x2 = user stack top.
    if regs.x[0] == 0 || regs.x[1] == 0 || regs.x[2] == 0 {
        return STATUS_INVALID_PARAMETER;
    }

    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return STATUS_ACCESS_VIOLATION;
    };
    let validation: Option<Result<(), crate::arch::mmu::PageMapError>> = process
        .with_user_address_space(|address_space| {
            address_space.validate_user_writable_range(regs.x[0], core::mem::size_of::<u64>())?;
            address_space.validate_user_instruction_pointer(regs.x[1])?;
            address_space.validate_user_stack_pointer(regs.x[2])
        });
    if !matches!(validation, Some(Ok(()))) {
        return STATUS_ACCESS_VIOLATION;
    }

    let thread =
        crate::executive::ps::thread::EThread::new_user(process.clone(), regs.x[1], regs.x[2], 4);
    let object = unsafe { alloc::sync::Arc::clone(&(*thread).object) };
    let tid = object.tid.0;
    let Some(handle) = process.insert_thread_handle(object) else {
        crate::executive::ps::scheduler::discard_unstarted(thread, STATUS_NO_MEMORY as i32);
        return STATUS_NO_MEMORY;
    };
    let output = handle.to_le_bytes();
    let copied_output = process
        .with_user_address_space(|address_space| address_space.copy_to_user(regs.x[0], &output));
    if !matches!(copied_output, Some(Ok(()))) {
        assert!(process.close_handle(handle));
        crate::executive::ps::scheduler::discard_unstarted(thread, STATUS_ACCESS_VIOLATION as i32);
        return STATUS_ACCESS_VIOLATION;
    }

    crate::executive::ps::scheduler::enqueue(thread);
    log::info!(
        "NtCreateThread created a validated EL0 thread tid={} handle={:#x}",
        tid,
        handle,
    );
    0
}

pub fn sys_allocate_virtual(regs: &mut SavedRegs) -> u64 {
    // x0 = process handle (-1 = current), x1 = *base_address, x2 = size
    let Some(size) = usize::try_from(regs.x[2]).ok().filter(|&size| size != 0) else {
        return STATUS_INVALID_PARAMETER;
    };
    if regs.x[0] != u64::MAX || regs.x[1] == 0 {
        return STATUS_INVALID_PARAMETER;
    }

    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return STATUS_ACCESS_VIOLATION;
    };
    let mut requested_base = [0u8; core::mem::size_of::<u64>()];
    let copied_input = process.with_user_address_space(|address_space| {
        address_space.copy_from_user(regs.x[1], &mut requested_base)
    });
    if !matches!(copied_input, Some(Ok(()))) {
        return STATUS_ACCESS_VIOLATION;
    }
    let requested_base = u64::from_le_bytes(requested_base);

    let allocated = process.with_user_address_space(|address_space| {
        if requested_base == 0 {
            address_space.allocate_user_region(size)
        } else {
            address_space.allocate_user_region_at(requested_base, size)
        }
    });
    let Some(Ok(base)) = allocated else {
        return if requested_base == 0 {
            STATUS_NO_MEMORY
        } else {
            STATUS_INVALID_PARAMETER
        };
    };

    let output = base.to_le_bytes();
    let copied_output = process
        .with_user_address_space(|address_space| address_space.copy_to_user(regs.x[1], &output));
    if matches!(copied_output, Some(Ok(()))) {
        let placement = if requested_base == 0 {
            "automatic"
        } else {
            "fixed"
        };
        log::info!(
            "NtAllocateVirtual mapped {} byte(s) at {:#x} ({})",
            size,
            base,
            placement,
        );
        return 0;
    }

    // `copy_to_user` prevalidates its full range. If it nevertheless fails,
    // do not retain an allocation the caller never received.
    let _ =
        process.with_user_address_space(|address_space| address_space.free_user_region(base, size));
    STATUS_ACCESS_VIOLATION
}

pub fn sys_free_virtual(regs: &mut SavedRegs) -> u64 {
    // x0 = process handle (-1 = current), x1 = base address, x2 = size.
    if regs.x[0] != u64::MAX {
        return 0xC000_0002; // STATUS_NOT_IMPLEMENTED
    }
    let virt = regs.x[1];
    let Some(size) = usize::try_from(regs.x[2]).ok().filter(|&size| size != 0) else {
        return STATUS_INVALID_PARAMETER;
    };
    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return STATUS_ACCESS_VIOLATION;
    };
    match process
        .with_user_address_space(|address_space| address_space.free_user_region(virt, size))
    {
        Some(Ok(())) => {
            log::info!("NtFreeVirtual released {} byte(s) at {:#x}", size, virt);
            0
        }
        Some(Err(_)) | None => STATUS_INVALID_PARAMETER,
    }
}

pub fn sys_create_file(_regs: &mut SavedRegs) -> u64 {
    0xC000_0002
}

pub fn sys_close(regs: &mut SavedRegs) -> u64 {
    let handle = regs.x[0];
    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return 0xC000_0008; // STATUS_INVALID_HANDLE
    };

    if process.close_handle(handle) {
        0 // STATUS_SUCCESS
    } else {
        STATUS_INVALID_HANDLE
    }
}

pub fn sys_wait_for_single_object(regs: &mut SavedRegs) -> u64 {
    // x0 = handle, x1 = alertable, x2 = optional pointer to a relative u64
    // scheduler-tick timeout, x3 = optional i32 completion-status output.
    if regs.x[1] != 0 {
        return STATUS_INVALID_PARAMETER;
    }

    let Some(process) = crate::executive::ps::scheduler::current_process() else {
        return STATUS_ACCESS_VIOLATION;
    };
    let timeout_ticks = if regs.x[2] == 0 {
        None
    } else {
        let mut timeout = [0u8; core::mem::size_of::<u64>()];
        let copied_timeout = process.with_user_address_space(|address_space| {
            address_space.copy_from_user(regs.x[2], &mut timeout)
        });
        if !matches!(copied_timeout, Some(Ok(()))) {
            return STATUS_ACCESS_VIOLATION;
        }
        let ticks = u64::from_le_bytes(timeout);
        if ticks == 0 || ticks > crate::executive::ps::scheduler::MAX_FINITE_WAIT_TICKS {
            return STATUS_INVALID_PARAMETER;
        }
        Some(ticks)
    };
    if regs.x[3] != 0 {
        let output_valid = process.with_user_address_space(|address_space| {
            address_space.validate_user_writable_range(regs.x[3], core::mem::size_of::<i32>())
        });
        if !matches!(output_valid, Some(Ok(()))) {
            return STATUS_ACCESS_VIOLATION;
        }
    }

    match crate::executive::ps::scheduler::wait_for_handle_with_timeout(regs.x[0], timeout_ticks) {
        Ok(crate::executive::ps::scheduler::WaitOutcome::Signaled(status)) => {
            if regs.x[3] != 0 {
                let output = status.to_le_bytes();
                let copied_output = process.with_user_address_space(|address_space| {
                    address_space.copy_to_user(regs.x[3], &output)
                });
                if !matches!(copied_output, Some(Ok(()))) {
                    return STATUS_ACCESS_VIOLATION;
                }
            }
            log::info!(
                "NtWaitForSingleObject observed exit status={:#x}",
                status as u32
            );
            0
        }
        Ok(crate::executive::ps::scheduler::WaitOutcome::TimedOut) => STATUS_TIMEOUT,
        Err(crate::executive::ps::scheduler::WaitError::InvalidHandle) => STATUS_INVALID_HANDLE,
        Err(crate::executive::ps::scheduler::WaitError::AccessDenied) => STATUS_ACCESS_DENIED,
        Err(_) => STATUS_INVALID_PARAMETER,
    }
}

pub fn sys_query_system_info(regs: &mut SavedRegs) -> u64 {
    let class = regs.x[0];
    match class {
        0 => {
            // SystemBasicInformation — return free page count as stub
            if regs.x[1] == 0 {
                return STATUS_INVALID_PARAMETER;
            }
            let Some(process) = crate::executive::ps::scheduler::current_process() else {
                return STATUS_ACCESS_VIOLATION;
            };
            let free_pages = crate::executive::mm::phys::free_pages() as u64;
            let output = free_pages.to_le_bytes();
            match process.with_user_address_space(|address_space| {
                address_space.copy_to_user(regs.x[1], &output)
            }) {
                Some(Ok(())) => {
                    log::info!("NtQuerySystemInfo copied validated EL0 output");
                    0
                }
                Some(Err(_)) | None => STATUS_ACCESS_VIOLATION,
            }
        }
        _ => 0xC000_0002,
    }
}

pub fn sys_terminate_process(regs: &mut SavedRegs) -> u64 {
    // The current-process pseudo-handle (-1) terminates the calling process,
    // including any queued siblings. A process handle terminates every
    // non-current target thread and signals its process completion.
    if regs.x[0] == u64::MAX {
        crate::executive::ps::scheduler::terminate_current_process(regs.x[1] as i32)
    }

    let Some(owner) = crate::executive::ps::scheduler::current_process() else {
        return STATUS_INVALID_HANDLE;
    };
    let target = match owner.handle_table.lock().lookup_with_access(
        regs.x[0],
        crate::executive::ob::handle::HANDLE_ACCESS_TERMINATE,
    ) {
        Ok(crate::executive::ob::handle::HandleObject::Process(target)) => target,
        Ok(_) | Err(crate::executive::ob::handle::HandleLookupError::Invalid) => {
            return STATUS_INVALID_HANDLE;
        }
        Err(crate::executive::ob::handle::HandleLookupError::AccessDenied) => {
            return STATUS_ACCESS_DENIED;
        }
    };

    match crate::executive::ps::scheduler::terminate_process(target, regs.x[1] as i32) {
        Ok(()) => 0,
        Err(crate::executive::ps::scheduler::TerminateProcessError::Exited) => 0,
        Err(_) => STATUS_INVALID_PARAMETER,
    }
}

pub fn sys_terminate_thread(regs: &mut SavedRegs) -> u64 {
    // x0 = parent-owned typed thread handle, x1 = final i32 status. The
    // current thread is deliberately rejected; its existing -1 process-pseudo
    // handle path remains the only self-termination contract.
    let Some(owner) = crate::executive::ps::scheduler::current_process() else {
        return STATUS_INVALID_HANDLE;
    };
    let target = match owner.handle_table.lock().lookup_with_access(
        regs.x[0],
        crate::executive::ob::handle::HANDLE_ACCESS_TERMINATE,
    ) {
        Ok(crate::executive::ob::handle::HandleObject::Thread(target)) => target,
        Ok(_) | Err(crate::executive::ob::handle::HandleLookupError::Invalid) => {
            return STATUS_INVALID_HANDLE;
        }
        Err(crate::executive::ob::handle::HandleLookupError::AccessDenied) => {
            return STATUS_ACCESS_DENIED;
        }
    };

    match crate::executive::ps::scheduler::terminate_thread(target, regs.x[1] as i32) {
        Ok(()) | Err(crate::executive::ps::scheduler::TerminateThreadError::Exited) => 0,
        Err(crate::executive::ps::scheduler::TerminateThreadError::SelfTarget) => {
            STATUS_INVALID_PARAMETER
        }
        Err(_) => STATUS_INVALID_HANDLE,
    }
}
