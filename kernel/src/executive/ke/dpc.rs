//! `KeDpc` — Deferred Procedure Calls.
//!
//! A DPC is a kernel callback enqueued from an ISR and executed at the end
//! of IRQ processing, at a lower IRQL than the ISR itself.  This avoids
//! doing lengthy work inside the interrupt handler.
//!
//! The DPC queue is flushed by `hal::timer::on_tick` after the tick IRQ.

use super::spinlock::KeSpinLock;
use alloc::collections::VecDeque;

pub type DpcRoutine = fn(context: usize);

pub struct KeDpc {
    pub routine: DpcRoutine,
    pub context: usize,
}

static DPC_LOCK: KeSpinLock = KeSpinLock::new();
static DPC_QUEUE: spin::Mutex<VecDeque<KeDpc>> = spin::Mutex::new(VecDeque::new());

/// Enqueue a DPC to be run after the current IRQ returns.
pub fn ke_insert_queue_dpc(routine: DpcRoutine, context: usize) {
    DPC_QUEUE.lock().push_back(KeDpc { routine, context });
}

/// Drain and execute all pending DPCs.  Called from the timer IRQ path.
pub fn flush_queue() {
    loop {
        let dpc = DPC_QUEUE.lock().pop_front();
        match dpc {
            Some(d) => (d.routine)(d.context),
            None => break,
        }
    }
}
