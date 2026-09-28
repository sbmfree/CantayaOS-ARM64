//! Shared terminal output for the kernel prompt and bounded EL0 console writes.

use alloc::sync::{Arc, Weak};
use spin::Mutex;

use crate::executive::ps::process::EProcess;

/// The input pseudo-handle is deliberately separate from the -1 output and
/// current-process pseudo-handles. It is never inserted into a handle table.
pub const INPUT_HANDLE: u64 = u64::MAX - 1;

static INPUT_OWNER: Mutex<Option<Weak<EProcess>>> = Mutex::new(None);

fn with_input_owner<R>(operation: impl FnOnce(&mut Option<Weak<EProcess>>) -> R) -> R {
    // Timer preemption on the single QEMU core must not switch to another
    // thread that spins on this lock while its current holder is suspended.
    let irq_state = crate::executive::ke::spinlock::IrqState::disable();
    let mut owner = INPUT_OWNER.lock();
    let result = operation(&mut owner);
    drop(owner);
    irq_state.restore();
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    OwnedByAnotherProcess,
}

pub fn write(args: core::fmt::Arguments<'_>) {
    crate::hal::uart::write_console(args);
    crate::hal::framebuffer::write_fmt(args);
}

/// Claim the two input devices for this process and take at most `bytes.len()`
/// immediately available bytes. Zero means no data yet; ownership is retained.
pub fn read_for_user(process: &Arc<EProcess>, bytes: &mut [u8]) -> Result<usize, InputError> {
    with_input_owner(|owner| {
        if let Some(active) = owner.as_ref().and_then(Weak::upgrade) {
            if active.exit_status().is_none() && !Arc::ptr_eq(&active, process) {
                return Err(InputError::OwnedByAnotherProcess);
            }
        }
        *owner = Some(Arc::downgrade(process));

        let mut count = 0;
        for slot in bytes {
            let Some(byte) =
                crate::hal::uart::try_read_byte().or_else(crate::drivers::keyboard::try_read)
            else {
                break;
            };
            *slot = byte;
            count += 1;
        }
        Ok(count)
    })
}

/// `NtClose(-2)` relinquishes only the calling process's input claim.
pub fn release_input(process: &Arc<EProcess>) -> bool {
    with_input_owner(|owner| {
        let Some(active) = owner.as_ref().and_then(Weak::upgrade) else {
            *owner = None;
            return false;
        };
        if !Arc::ptr_eq(&active, process) {
            return false;
        }
        *owner = None;
        true
    })
}

/// The kernel prompt may poll only when no live EL0 process owns input.
pub fn try_read_for_shell() -> Option<u8> {
    with_input_owner(|owner| {
        if let Some(active) = owner.as_ref().and_then(Weak::upgrade) {
            if active.exit_status().is_none() {
                return None;
            }
        }
        *owner = None;
        crate::hal::uart::try_read_byte().or_else(crate::drivers::keyboard::try_read)
    })
}
