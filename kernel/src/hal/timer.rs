//! ARM Generic Timer (EL1 Physical) driver.
//!
//! Generates a periodic interrupt (default: 10 ms = 100 Hz) used by the
//! scheduler for preemptive round-robin timeslicing.
//!
//! PPI 30 is the non-secure EL1 physical timer in GICv2 on QEMU virt.

use aarch64_cpu::registers::*;
use tock_registers::interfaces::{Readable, Writeable};

/// GIC IRQ number for EL1 Physical Timer on QEMU virt.
const TIMER_IRQ: usize = 30;

/// Timer frequency (Hz) — read from CNTFRQ_EL0.
static mut TIMER_FREQ: u64 = 0;

/// Tick interval in Hz (100 Hz = 10 ms slices).
const TICK_HZ: u64 = 100;

/// Initialise the ARM Generic Timer and register it with the GIC.
pub fn init() {
    unsafe {
        TIMER_FREQ = CNTFRQ_EL0.get();
        let interval = TIMER_FREQ / TICK_HZ;

        // Load the compare value
        CNTP_TVAL_EL0.set(interval);
        // Enable and unmask the EL1 physical timer
        CNTP_CTL_EL0.write(CNTP_CTL_EL0::ENABLE::SET + CNTP_CTL_EL0::IMASK::CLEAR);
    }

    crate::hal::gic::register_handler(TIMER_IRQ, on_tick);
    log::debug!("ARM Generic Timer at {} Hz, IRQ {}", TICK_HZ, TIMER_IRQ);
}

/// Reload the timer compare register after each tick.
fn reload() {
    let interval = unsafe { TIMER_FREQ / TICK_HZ };
    CNTP_TVAL_EL0.set(interval);
}

/// Timer tick handler — called from GIC IRQ path.
fn on_tick() {
    reload();
    crate::executive::ke::dpc::flush_queue();
    crate::executive::ps::scheduler::on_timer_tick();
}
