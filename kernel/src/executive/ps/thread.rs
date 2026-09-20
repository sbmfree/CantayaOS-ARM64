//! ETHREAD — Executive Thread structure and context switch support.

use super::process::EProcess;
use crate::executive::ob::types::{ObjectHeader, OB_TYPE_THREAD};
use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use spin::Mutex;

const PAGE_SIZE: usize = 4096;

/// Thread unique ID counter.
static NEXT_TID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadId(pub u64);

/// Arc-owned thread identity and completion state.
///
/// This intentionally has a different lifetime from `EThread`: the scheduler
/// owns the raw execution record and releases its stack after a context
/// switch, while a handle may retain this completion object to observe exit.
pub struct ThreadObject {
    pub object_header: ObjectHeader,
    pub tid: ThreadId,
    completion: Mutex<ThreadCompletion>,
}

struct ThreadCompletion {
    exit_status: Option<i32>,
    waiters: VecDeque<usize>,
}

#[derive(Clone)]
pub enum WaitTarget {
    Process(Arc<EProcess>),
    Thread(Arc<ThreadObject>),
}

impl WaitTarget {
    pub fn observe_or_register_waiter(&self, waiter: usize) -> Option<i32> {
        match self {
            Self::Process(process) => process.observe_or_register_waiter(waiter),
            Self::Thread(thread) => thread.observe_or_register_waiter(waiter),
        }
    }

    pub fn cancel_waiter(&self, waiter: usize) {
        match self {
            Self::Process(process) => process.cancel_waiter(waiter),
            Self::Thread(thread) => thread.cancel_waiter(waiter),
        }
    }
}

impl ThreadObject {
    fn new(tid: ThreadId) -> Arc<Self> {
        Arc::new(Self {
            object_header: ObjectHeader::new(&OB_TYPE_THREAD, core::mem::size_of::<EThread>()),
            tid,
            completion: Mutex::new(ThreadCompletion {
                exit_status: None,
                waiters: VecDeque::new(),
            }),
        })
    }

    /// Return a recorded exit status, or register a scheduler thread to be
    /// woken when this thread exits. The scheduler marks that thread waiting
    /// while coordinating this operation.
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

    /// Return the recorded completion status without registering a waiter.
    pub fn exit_status(&self) -> Option<i32> {
        self.completion.lock().exit_status
    }

    /// Mark the execution record as terminated and return scheduler threads
    /// that must be made ready after the completion lock is released.
    pub fn signal_exit(&self, status: i32) -> Vec<usize> {
        let mut completion = self.completion.lock();
        if completion.exit_status.is_some() {
            return Vec::new();
        }
        completion.exit_status = Some(status);
        completion.waiters.drain(..).collect()
    }
}

/// AArch64 callee-saved register context.
/// Layout MUST match the offsets in `boot.s` `arch_context_switch`.
#[repr(C)]
pub struct ThreadContext {
    pub x19: u64,    // offset 0
    pub x20: u64,    // offset 8
    pub x21: u64,    // offset 16
    pub x22: u64,    // offset 24
    pub x23: u64,    // offset 32
    pub x24: u64,    // offset 40
    pub x25: u64,    // offset 48
    pub x26: u64,    // offset 56
    pub x27: u64,    // offset 64
    pub x28: u64,    // offset 72
    pub x29: u64,    // fp  offset 80
    pub x30: u64,    // lr  offset 88 — return address for resume
    pub sp: u64,     // offset 96
    pub elr: u64,    // offset 104 — EL0 return address (for user threads)
    pub spsr: u64,   // offset 112
    pub daif: u64,   // offset 120 — interrupt mask; 0 = all unmasked (IRQs on)
    pub sp_el0: u64, // offset 128 — per-thread EL0 user stack pointer
}

impl ThreadContext {
    pub fn new_kernel(entry: fn(), stack_top: u64) -> Self {
        ThreadContext {
            x19: 0,
            x20: 0,
            x21: 0,
            x22: 0,
            x23: 0,
            x24: 0,
            x25: 0,
            x26: 0,
            x27: 0,
            x28: 0,
            x29: stack_top,
            x30: entry as u64,
            sp: stack_top,
            elr: 0,
            spsr: 0x3C5,
            daif: 0x40, // FIQ masked (bit6=1), IRQ unmasked (bit7=0)
            sp_el0: 0,
        }
    }

    pub fn new_user(kernel_stack_top: u64, user_stack_top: u64) -> Self {
        ThreadContext {
            x19: 0,
            x20: 0,
            x21: 0,
            x22: 0,
            x23: 0,
            x24: 0,
            x25: 0,
            x26: 0,
            x27: 0,
            x28: 0,
            x29: kernel_stack_top,
            x30: 0,
            sp: kernel_stack_top,
            elr: 0,
            spsr: 0x3C5,
            daif: 0x40, // FIQ masked (bit6=1), IRQ unmasked (bit7=0)
            sp_el0: user_stack_top,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Running,
    Waiting,
    Terminated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadMode {
    Kernel,
    User,
}

pub struct EThread {
    pub tid: ThreadId,
    pub object: Arc<ThreadObject>,
    pub process: Arc<EProcess>,
    /// Set after the scheduler has signaled both thread and process exit.
    pub exit_reported: bool,
    pub state: ThreadState,
    /// Absolute scheduler tick at which this completion wait expires.
    pub wait_deadline: Option<u64>,
    /// Set by the timer path before it readies an expired waiter.
    pub wait_timed_out: bool,
    /// Completion object that owns this thread's pending wait registration.
    pub wait_target: Option<WaitTarget>,
    pub mode: ThreadMode,
    /// True after the initial ERET has established this thread's EL0 state.
    pub user_started: bool,
    pub user_entry: u64,
    pub user_stack_top: u64,
    /// Value placed in `x1` when the thread first enters EL0.
    pub user_argument: u64,
    pub context: ThreadContext,
    /// Physical address of this thread's kernel stack.
    pub stack_phys: u64,
    /// TTBR1 base address of this thread's kernel stack.
    pub stack_virt: u64,
    pub stack_size: usize,
}

impl EThread {
    /// Create a new kernel-mode thread.
    pub fn new_kernel(process: Arc<EProcess>, entry: fn(), stack_pages: usize) -> *mut EThread {
        let (stack_phys, stack_virt, stack_size, stack_top) = allocate_kernel_stack(stack_pages);
        let tid = ThreadId(NEXT_TID.fetch_add(1, core::sync::atomic::Ordering::SeqCst));
        process.register_thread();

        let t = alloc::boxed::Box::new(EThread {
            tid,
            object: ThreadObject::new(tid),
            process,
            exit_reported: false,
            state: ThreadState::Ready,
            wait_deadline: None,
            wait_timed_out: false,
            wait_target: None,
            mode: ThreadMode::Kernel,
            user_started: false,
            user_entry: 0,
            user_stack_top: 0,
            user_argument: 0,
            context: ThreadContext::new_kernel(entry, stack_top),
            stack_phys,
            stack_virt,
            stack_size,
        });

        alloc::boxed::Box::into_raw(t)
    }

    /// Create a thread which starts in EL0 through the architecture ERET path.
    pub fn new_user(
        process: Arc<EProcess>,
        entry: u64,
        user_stack_top: u64,
        kernel_stack_pages: usize,
    ) -> *mut EThread {
        Self::new_user_with_argument(process, entry, user_stack_top, kernel_stack_pages, 0)
    }

    /// Create a thread which starts in EL0 with a controlled `x1` argument.
    pub fn new_user_with_argument(
        process: Arc<EProcess>,
        entry: u64,
        user_stack_top: u64,
        kernel_stack_pages: usize,
        user_argument: u64,
    ) -> *mut EThread {
        assert!(entry != 0, "user thread entry must be nonzero");
        assert!(user_stack_top != 0, "user thread stack top must be nonzero");
        let (stack_phys, stack_virt, stack_size, stack_top) =
            allocate_kernel_stack(kernel_stack_pages);
        let tid = ThreadId(NEXT_TID.fetch_add(1, core::sync::atomic::Ordering::SeqCst));
        process.register_thread();

        let t = alloc::boxed::Box::new(EThread {
            tid,
            object: ThreadObject::new(tid),
            process,
            exit_reported: false,
            state: ThreadState::Ready,
            wait_deadline: None,
            wait_timed_out: false,
            wait_target: None,
            mode: ThreadMode::User,
            user_started: false,
            user_entry: entry,
            user_stack_top,
            user_argument,
            context: ThreadContext::new_user(stack_top, user_stack_top),
            stack_phys,
            stack_virt,
            stack_size,
        });

        alloc::boxed::Box::into_raw(t)
    }

    /// Signal the thread's handle-backed completion object exactly once.
    pub fn signal_exit(&mut self, status: i32) -> Vec<usize> {
        assert!(
            !self.exit_reported,
            "thread exit was reported more than once"
        );
        self.exit_reported = true;
        self.object.signal_exit(status)
    }
}

impl Drop for EThread {
    fn drop(&mut self) {
        debug_assert!(
            self.exit_reported,
            "dropping a thread that was never reported as exited"
        );
        // The scheduler defers this destructor until another stack is active.
        crate::executive::mm::phys::free_contiguous_pages(
            self.stack_phys,
            self.stack_size / PAGE_SIZE,
        );
    }
}

fn allocate_kernel_stack(stack_pages: usize) -> (u64, u64, usize, u64) {
    use crate::{arch::mmu, executive::mm::phys};

    assert!(
        stack_pages > 0,
        "kernel thread stack must contain at least one page"
    );
    let stack_size = stack_pages * PAGE_SIZE;
    // AArch64 uses this as one linear stack range. The backing pages remain
    // contiguous for ownership, but every live stack pointer uses TTBR1.
    let stack_phys = phys::alloc_contiguous_pages(stack_pages);
    let stack_virt = mmu::phys_to_direct_map(stack_phys);
    let stack_top = stack_virt + stack_size as u64;
    assert!(
        mmu::is_direct_map_address(stack_top - 1),
        "kernel stack is not mapped through TTBR1",
    );
    (stack_phys, stack_virt, stack_size, stack_top)
}

extern "C" {
    /// Assembly context switch — saves `from`, restores `to`.
    pub fn arch_context_switch(from: *mut ThreadContext, to: *const ThreadContext);

    /// Save the active scheduler context and perform the first ERET into an
    /// EL0 thread. It physically returns only when `from` is later restored.
    pub fn arch_context_save_and_enter_user(
        from: *mut ThreadContext,
        ttbr0_root: u64,
        entry: u64,
        user_stack_top: u64,
        kernel_stack_top: u64,
        user_argument: u64,
    );
}
