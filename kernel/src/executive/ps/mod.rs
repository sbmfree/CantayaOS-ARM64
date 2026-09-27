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
use spin::Mutex;
use thread::EThread;

pub const INITIAL_IMAGE_SOURCE: u64 = elf::INITIAL_IMAGE_SOURCE;
pub const FAT_CHILD_IMAGE_SOURCE: u64 = elf::FAT_CHILD_IMAGE_SOURCE;

static INITIAL_PROCESSES: Mutex<Option<[Arc<EProcess>; 2]>> = Mutex::new(None);

/// One-time initialisation: create the idle process + System process.
pub fn init(boot_info: &BootInfo) {
    scheduler::init();
    let [(first_process, first_thread), (second_process, second_thread)] =
        elf::load_initial_processes(boot_info)
            .expect("failed to load the initial user-mode ELF process");
    *INITIAL_PROCESSES.lock() = Some([first_process, second_process]);
    scheduler::enqueue(first_thread);
    scheduler::enqueue(second_thread);
    log::debug!("Ps: Process Manager ready");
}

/// Report when the boot validation workloads have exited, releasing the
/// process references retained only to sequence the terminal startup.
pub fn initial_processes_complete() -> bool {
    let completed = {
        let mut initial = INITIAL_PROCESSES.lock();
        let Some(processes) = initial.as_ref() else {
            return true;
        };
        if processes
            .iter()
            .all(|process| process.exit_status().is_some())
        {
            initial.take()
        } else {
            None
        }
    };
    completed.is_some()
}

/// Create a child from the validated boot image retained by the process
/// manager. This deliberately does not expose ELF bytes or parsing to callers.
pub fn create_process_from_source(
    source: u64,
    initial_argument: u64,
) -> Result<(Arc<EProcess>, *mut EThread), elf::LoadError> {
    elf::create_process_from_source(source, initial_argument)
}
