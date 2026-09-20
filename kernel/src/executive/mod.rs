//! NT Executive — Windows NT-like kernel subsystems.
//!
//! Layering (bottom-up):
//!   Ke  — kernel base (spinlocks, mutexes, DPCs, wait)
//!   Mm  — memory manager
//!   Ob  — object manager
//!   Ps  — process/thread manager + scheduler
//!   Io  — I/O manager
//!   Se  — security reference monitor

pub mod io;
pub mod ke;
pub mod mm;
pub mod ob;
pub mod ps;
pub mod se;
