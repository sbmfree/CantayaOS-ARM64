//! Ps — Process and Thread Manager.
//!
//! NT-style:
//!   EPROCESS  — executive process structure
//!   ETHREAD   — executive thread structure
//!   Scheduler — round-robin preemptive (timer-driven)

mod elf;
pub mod process;
pub mod scheduler;
pub mod thread;

use alloc::sync::Arc;
use cantaya_shared::BootInfo;
use process::EProcess;
use thread::EThread;

pub const INITIAL_IMAGE_SOURCE: u64 = elf::INITIAL_IMAGE_SOURCE;
pub const FAT_CHILD_IMAGE_SOURCE: u64 = elf::FAT_CHILD_IMAGE_SOURCE;

/// One-time initialisation: create the idle process + System process.
pub fn init(boot_info: &BootInfo) {
    scheduler::init();
    let init_threads = elf::load_initial_processes(boot_info)
        .expect("failed to load the initial user-mode ELF process");
    for thread in init_threads {
        scheduler::enqueue(thread);
    }
    log::debug!("Ps: Process Manager ready");
}

/// Create a child from the validated boot image retained by the process
/// manager. This deliberately does not expose ELF bytes or parsing to callers.
pub fn create_process_from_source(
    source: u64,
    initial_argument: u64,
) -> Result<(Arc<EProcess>, *mut EThread), elf::LoadError> {
    elf::create_process_from_source(source, initial_argument)
}
