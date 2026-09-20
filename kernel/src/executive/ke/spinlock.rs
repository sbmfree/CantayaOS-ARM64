//! `KeSpinLock` — IRQ-disabling spinlock.
//!
//! Disables IRQs on the local core before acquiring so that ISR code cannot
//! deadlock by re-entering the same lock.  Uses AArch64 `LDAXR`/`STLXR`
//! (load-acquire exclusive / store-release exclusive) for atomicity.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

/// Saved IRQ state (DAIF I-bit value before we disabled interrupts).
#[derive(Debug)]
pub struct IrqState(u64);

impl IrqState {
    /// Disable IRQ and return the previous DAIF state.
    pub fn disable() -> Self {
        let daif: u64;
        unsafe {
            core::arch::asm!(
                "mrs {daif}, DAIF",
                "msr DAIFSet, #2",   // set I-bit → disable IRQ
                daif = out(reg) daif,
            );
        }
        IrqState(daif)
    }

    /// Restore IRQ state from saved DAIF value.
    pub fn restore(self) {
        unsafe {
            core::arch::asm!(
                "msr DAIF, {daif}",
                daif = in(reg) self.0,
            );
        }
    }
}

/// A simple test-and-set spinlock.
pub struct KeSpinLock {
    locked: AtomicBool,
}

impl KeSpinLock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    /// Acquire the lock, disabling IRQs.  Returns the saved IRQ state.
    pub fn acquire(&self) -> IrqState {
        let state = IrqState::disable();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        state
    }

    /// Release the lock and restore IRQ state.
    pub fn release(&self, state: IrqState) {
        self.locked.store(false, Ordering::Release);
        state.restore();
    }
}

/// RAII guard for `KeSpinLock`.
pub struct SpinLockGuard<'a> {
    lock: &'a KeSpinLock,
    state: Option<IrqState>,
}

impl<'a> SpinLockGuard<'a> {
    pub fn new(lock: &'a KeSpinLock) -> Self {
        let state = lock.acquire();
        SpinLockGuard {
            lock,
            state: Some(state),
        }
    }
}

impl Drop for SpinLockGuard<'_> {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            self.lock.release(state);
        }
    }
}

impl KeSpinLock {
    pub fn lock(&self) -> SpinLockGuard<'_> {
        SpinLockGuard::new(self)
    }
}
