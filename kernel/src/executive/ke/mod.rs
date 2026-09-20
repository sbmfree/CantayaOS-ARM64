//! Ke — Kernel Executive primitives.
//!
//! NT naming conventions:
//!   Ke  prefix = kernel-mode, low-level synchronisation + scheduling support
//!
//! Modules:
//!   spinlock — `KeSpinLock` (disable-IRQ + LDAXR/STLXR CAS)
//!   mutex    — `KeMutex`   (sleeping mutex, backed by wait queue)
//!   dpc      — `KeDpc`     (deferred procedure calls, run at end of IRQ)

pub mod dpc;
pub mod mutex;
pub mod spinlock;
