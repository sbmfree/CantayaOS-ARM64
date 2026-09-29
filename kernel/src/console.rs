//! Shared terminal output for the kernel prompt and bounded EL0 console writes.

use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
};
use spin::Mutex;

use crate::executive::ps::process::EProcess;

/// The input pseudo-handle is deliberately separate from the -1 output and
/// current-process pseudo-handles. It is never inserted into a handle table.
pub const INPUT_HANDLE: u64 = u64::MAX - 1;

struct InputState {
    owner: Option<Weak<EProcess>>,
    generation: u64,
    waiters: VecDeque<usize>,
}

static INPUT_STATE: Mutex<InputState> = Mutex::new(InputState {
    owner: None,
    generation: 0,
    waiters: VecDeque::new(),
});

fn with_input_state<R>(operation: impl FnOnce(&mut InputState) -> R) -> R {
    // Timer preemption on the single QEMU core must not switch to another
    // thread that spins on this lock while its current holder is suspended.
    let irq_state = crate::executive::ke::spinlock::IrqState::disable();
    let mut state = INPUT_STATE.lock();
    let result = operation(&mut state);
    drop(state);
    irq_state.restore();
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    OwnedByAnotherProcess,
}

fn claim(state: &mut InputState, process: &Arc<EProcess>) -> Result<u64, InputError> {
    if let Some(active) = state.owner.as_ref().and_then(Weak::upgrade) {
        if active.exit_status().is_none() {
            if Arc::ptr_eq(&active, process) {
                return Ok(state.generation);
            }
            return Err(InputError::OwnedByAnotherProcess);
        }
    }
    debug_assert!(
        state.waiters.is_empty(),
        "exited console owner retained a waiter"
    );
    state.generation = state
        .generation
        .checked_add(1)
        .expect("console generation exhausted");
    state.owner = Some(Arc::downgrade(process));
    Ok(state.generation)
}

fn input_ready() -> bool {
    let keyboard_ready = crate::drivers::keyboard::has_pending_byte();
    crate::hal::uart::has_input() || keyboard_ready
}

/// Claim input without consuming it, returning the ownership generation used
/// to distinguish a close/reclaim from an uninterrupted wait.
pub fn claim_input(process: &Arc<EProcess>) -> Result<u64, InputError> {
    with_input_state(|state| claim(state, process))
}

pub fn write(args: core::fmt::Arguments<'_>) {
    crate::hal::uart::write_console(args);
    crate::hal::framebuffer::write_fmt(args);
}

/// Claim the two input devices for this process and take at most `bytes.len()`
/// immediately available bytes. Zero means no data yet; ownership is retained.
pub fn read_for_user(process: &Arc<EProcess>, bytes: &mut [u8]) -> Result<usize, InputError> {
    with_input_state(|state| {
        claim(state, process)?;

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
    let waiters = with_input_state(|state| {
        let Some(active) = state.owner.as_ref().and_then(Weak::upgrade) else {
            state.owner = None;
            return None;
        };
        if !Arc::ptr_eq(&active, process) {
            return None;
        }
        state.owner = None;
        state.generation = state
            .generation
            .checked_add(1)
            .expect("console generation exhausted");
        Some(core::mem::take(&mut state.waiters))
    });
    let Some(waiters) = waiters else {
        return false;
    };
    for waiter in waiters {
        crate::executive::ps::scheduler::wake_thread(waiter as *mut _);
    }
    true
}

/// Observe console readiness or register one scheduler-owned raw waiter.
/// The caller masks IRQs until its thread is marked waiting and switched out.
pub fn observe_or_register_waiter(
    process: &Arc<EProcess>,
    generation: u64,
    waiter: usize,
) -> Option<i32> {
    with_input_state(|state| {
        let still_owned = state.generation == generation
            && state
                .owner
                .as_ref()
                .and_then(Weak::upgrade)
                .is_some_and(|active| {
                    active.exit_status().is_none() && Arc::ptr_eq(&active, process)
                });
        if !still_owned {
            return Some(0xC000_0008u32 as i32); // STATUS_INVALID_HANDLE after close
        }
        if input_ready() {
            return Some(0);
        }
        assert!(
            !state.waiters.contains(&waiter),
            "duplicate console wait registration"
        );
        state.waiters.push_back(waiter);
        None
    })
}

pub fn cancel_waiter(waiter: usize) {
    with_input_state(|state| state.waiters.retain(|entry| *entry != waiter));
}

/// An input IRQ wakes one waiter of the live owner.
/// Device bytes remain queued until that thread performs `NtReadFile`.
pub fn wake_ready_waiter() {
    let irq_state = crate::executive::ke::spinlock::IrqState::disable();
    let waiter = with_input_state(|state| {
        if state.waiters.is_empty() {
            return None;
        }
        let owner_live = state
            .owner
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|process| process.exit_status().is_none());
        if owner_live && input_ready() {
            state.waiters.pop_front()
        } else {
            None
        }
    });
    if let Some(waiter) = waiter {
        crate::executive::ps::scheduler::wake_thread(waiter as *mut _);
    }
    irq_state.restore();
}

/// Only the current live input owner may redraw the terminal from EL0.
pub fn owns_input(process: &Arc<EProcess>) -> bool {
    with_input_state(|state| {
        state
            .owner
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|active| active.exit_status().is_none() && Arc::ptr_eq(&active, process))
    })
}

pub fn clear() {
    crate::hal::uart::write_console(format_args!("\x1b[2J\x1b[H"));
    crate::hal::framebuffer::show_shell_screen();
}

/// The kernel prompt may poll only when no live EL0 process owns input.
pub fn try_read_for_shell() -> Option<u8> {
    with_input_state(|state| {
        if let Some(active) = state.owner.as_ref().and_then(Weak::upgrade) {
            if active.exit_status().is_none() {
                return None;
            }
        }
        if state.owner.is_some() {
            debug_assert!(
                state.waiters.is_empty(),
                "exited console owner retained a waiter"
            );
            state.owner = None;
            state.generation = state
                .generation
                .checked_add(1)
                .expect("console generation exhausted");
        }
        crate::hal::uart::try_read_byte().or_else(crate::drivers::keyboard::try_read)
    })
}
