//! EPROCESS — Executive Process structure.

use crate::arch::mmu::UserAddressSpace;
use crate::executive::ob::handle::{
    Handle, HandleAccess, HandleObject, HandleTable, HANDLE_ACCESS_TERMINATE, HANDLE_ACCESS_WAIT,
};
use crate::executive::ob::types::{ObjectHeader, OB_TYPE_PROCESS};
use crate::executive::ps::thread::ThreadObject;
use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use spin::Mutex;

/// Process unique ID counter.
static NEXT_PID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId(pub u64);

struct ProcessCompletion {
    active_threads: usize,
    exit_status: Option<i32>,
    termination_requested: bool,
    waiters: VecDeque<usize>,
}

pub struct EProcess {
    /// Object-manager identity retained by typed process handles.
    pub object_header: ObjectHeader,
    pub pid: ProcessId,
    pub handle_table: Mutex<HandleTable>,
    /// Physical page base of this process's page tables (TTBR0).
    pub page_table_base: u64,
    /// Present for user processes; kernel-only processes share bootstrap TTBR0.
    pub address_space: Mutex<Option<UserAddressSpace>>,
    /// Virtual base where the main image is loaded (0 for kernel processes).
    pub image_base: u64,
    completion: Mutex<ProcessCompletion>,
}

impl EProcess {
    pub fn new_kernel_process() -> Arc<Self> {
        Arc::new(EProcess {
            object_header: ObjectHeader::new(&OB_TYPE_PROCESS, core::mem::size_of::<Self>()),
            pid: ProcessId(NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::SeqCst)),
            handle_table: Mutex::new(HandleTable::new()),
            page_table_base: 0, // shares kernel page tables
            address_space: Mutex::new(None),
            image_base: 0,
            completion: Mutex::new(ProcessCompletion {
                active_threads: 0,
                exit_status: None,
                termination_requested: false,
                waiters: VecDeque::new(),
            }),
        })
    }

    /// Create a process with an independently-owned, inactive TTBR0 root.
    /// The scheduler activates this root whenever one of its user threads runs.
    pub fn new_user_process(image_base: u64) -> Arc<Self> {
        let address_space = UserAddressSpace::new();
        let page_table_base = address_space.root_phys();
        Arc::new(EProcess {
            object_header: ObjectHeader::new(&OB_TYPE_PROCESS, core::mem::size_of::<Self>()),
            pid: ProcessId(NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::SeqCst)),
            handle_table: Mutex::new(HandleTable::new()),
            page_table_base,
            address_space: Mutex::new(Some(address_space)),
            image_base,
            completion: Mutex::new(ProcessCompletion {
                active_threads: 0,
                exit_status: None,
                termination_requested: false,
                waiters: VecDeque::new(),
            }),
        })
    }

    /// Mutably access the user address space, if this is a user process.
    pub fn with_user_address_space<R>(
        &self,
        operation: impl FnOnce(&mut UserAddressSpace) -> R,
    ) -> Option<R> {
        let mut address_space = self.address_space.lock();
        address_space.as_mut().map(operation)
    }

    /// Close a handle owned by this process.
    pub fn close_handle(&self, handle: Handle) -> bool {
        self.handle_table.lock().close(handle)
    }

    pub fn insert_process_handle(&self, target: Arc<EProcess>) -> Option<Handle> {
        self.handle_table.lock().insert(
            HandleObject::Process(target),
            HANDLE_ACCESS_WAIT | HANDLE_ACCESS_TERMINATE,
        )
    }

    pub fn insert_thread_handle(&self, target: Arc<ThreadObject>) -> Option<Handle> {
        self.insert_thread_handle_with_access(target, HANDLE_ACCESS_WAIT | HANDLE_ACCESS_TERMINATE)
    }

    pub(crate) fn insert_thread_handle_with_access(
        &self,
        target: Arc<ThreadObject>,
        access: HandleAccess,
    ) -> Option<Handle> {
        self.handle_table
            .lock()
            .insert(HandleObject::Thread(target), access)
    }

    /// Register an execution record before it becomes schedulable.
    pub fn register_thread(&self) {
        let mut completion = self.completion.lock();
        assert!(
            completion.exit_status.is_none() && !completion.termination_requested,
            "cannot create a thread in an exited process"
        );
        completion.active_threads += 1;
    }

    /// Record one thread's exit and wake process waiters when it was the last
    /// active execution record.
    pub fn thread_exited(&self, status: i32) -> Vec<usize> {
        let mut completion = self.completion.lock();
        assert!(
            completion.active_threads > 0,
            "process thread accounting underflow"
        );
        completion.active_threads -= 1;
        if completion.active_threads != 0 || completion.exit_status.is_some() {
            return Vec::new();
        }

        completion.exit_status = Some(status);
        completion.waiters.drain(..).collect()
    }

    /// Return a final process status, or register a scheduler thread that
    /// must be woken when the last active thread exits.
    pub fn observe_or_register_waiter(&self, waiter: usize) -> Option<i32> {
        let mut completion = self.completion.lock();
        match completion.exit_status {
            Some(status) => Some(status),
            None => {
                completion.waiters.push_back(waiter);
                None
            }
        }
    }

    /// Remove a timed-out scheduler thread before its next wait can begin.
    pub fn cancel_waiter(&self, waiter: usize) {
        self.completion
            .lock()
            .waiters
            .retain(|entry| *entry != waiter);
    }

    /// Prevent future threads from joining this process while the scheduler
    /// removes its existing execution records for external termination.
    pub fn begin_termination(&self) -> Result<(), ProcessTerminationState> {
        let mut completion = self.completion.lock();
        if completion.exit_status.is_some() {
            return Err(ProcessTerminationState::Exited);
        }
        if completion.termination_requested {
            return Err(ProcessTerminationState::Terminating);
        }
        completion.termination_requested = true;
        Ok(())
    }

    /// Undo a termination request that could not find any scheduler-owned
    /// thread records. This is defensive; it should be unreachable on the
    /// single-core scheduler.
    pub fn cancel_termination(&self) {
        self.completion.lock().termination_requested = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTerminationState {
    Exited,
    Terminating,
}

impl Drop for EProcess {
    fn drop(&mut self) {
        log::info!("Ps: reaped process pid={}", self.pid.0);
    }
}
