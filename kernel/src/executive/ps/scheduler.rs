//! Round-robin preemptive scheduler.

use super::process::{EProcess, ProcessTerminationState};
use super::thread::{
    arch_context_save_and_enter_user, arch_context_switch, EThread, ThreadMode, ThreadState,
    WaitTarget,
};
use crate::executive::ob::handle::{
    Handle, HandleAccess, HandleLookupError, HandleObject, HANDLE_ACCESS_TERMINATE,
    HANDLE_ACCESS_WAIT,
};
use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};
use spin::Mutex;

/// Raw thread pointer stored as usize to satisfy Send+Sync on the static.
type ThreadPtr = usize;

fn tp(p: *mut EThread) -> ThreadPtr {
    p as usize
}
fn from_tp(t: ThreadPtr) -> *mut EThread {
    t as *mut EThread
}

/// Global run queue.
static RUN_QUEUE: Mutex<VecDeque<ThreadPtr>> = Mutex::new(VecDeque::new());

/// Waiting threads ordered by their finite-wait expiration tick.
static TIMED_WAITERS: Mutex<VecDeque<ThreadPtr>> = Mutex::new(VecDeque::new());

/// Threads blocked on retained process or thread completion objects.
static COMPLETION_WAITERS: Mutex<VecDeque<ThreadPtr>> = Mutex::new(VecDeque::new());

/// Threads that have switched away after termination and can be destroyed by
/// the next active context.
static RETIRED_THREADS: Mutex<VecDeque<ThreadPtr>> = Mutex::new(VecDeque::new());

/// Currently executing thread (per-CPU; single-core).
static mut CURRENT: ThreadPtr = 0;

/// Idle thread context.
static mut IDLE_CONTEXT: super::thread::ThreadContext = super::thread::ThreadContext {
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
    x29: 0,
    x30: 0,
    sp: 0,
    elr: 0,
    spsr: 0,
    daif: 0x40,
    sp_el0: 0,
};

/// Global tick counter (incremented every 100 Hz timer tick).
pub static TICK_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

static CONSOLE_INPUT_PROBE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Finite waits use relative 100 Hz scheduler ticks and are capped at 10 seconds.
pub const MAX_FINITE_WAIT_TICKS: u64 = 1_000;

/// Initialise the scheduler and create the System kernel thread.
pub fn init(console_input_probe: bool) {
    CONSOLE_INPUT_PROBE.store(console_input_probe, core::sync::atomic::Ordering::Relaxed);
    let system_proc = EProcess::new_kernel_process();
    let system_thread = EThread::new_kernel(system_proc, system_thread_main, 4);
    RUN_QUEUE.lock().push_back(tp(system_thread));
    log::debug!("Ps/Scheduler: System thread enqueued");
}

/// Called from the timer IRQ (100 Hz).
pub fn on_timer_tick() {
    let now = TICK_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    expire_timed_waiters(now);
    crate::executive::ke::dpc::flush_queue();
    schedule();
}

/// Enqueue a thread pointer for scheduling.
pub fn enqueue(t: *mut EThread) {
    RUN_QUEUE.lock().push_back(tp(t));
}

fn retire_thread(thread: ThreadPtr) {
    RETIRED_THREADS.lock().push_back(thread);
}

fn reap_retired_threads() {
    loop {
        let retired = { RETIRED_THREADS.lock().pop_front() };
        let Some(thread_ptr) = retired else {
            return;
        };

        assert_ne!(
            thread_ptr,
            unsafe { CURRENT },
            "attempted to reclaim the active thread stack"
        );
        let thread = unsafe { Box::from_raw(from_tp(thread_ptr)) };
        log::info!(
            "Ps: reaped thread tid={} pid={}",
            thread.tid.0,
            thread.process.pid.0,
        );
        drop(thread);
    }
}

/// Return the currently executing thread, if execution has left the idle
/// context.  This is single-core state until SMP support is introduced.
pub fn current_thread() -> Option<*mut EThread> {
    let current = unsafe { CURRENT };
    (current != 0).then(|| from_tp(current))
}

/// Clone the process reference for the active thread.
pub fn current_process() -> Option<Arc<EProcess>> {
    let current = current_thread()?;
    Some(unsafe { Arc::clone(&(*current).process) })
}

/// Mark the current thread as waiting without scheduling it away.
///
/// Callers use this while holding their own IRQ-safe lock, then release that
/// lock before calling [`schedule`].  Holding a lock across a context switch
/// would otherwise deadlock the thread that wakes the waiter.
pub fn mark_current_waiting() -> Option<*mut EThread> {
    let current = current_thread()?;
    unsafe { (*current).state = ThreadState::Waiting };
    Some(current)
}

/// Make a waiting thread runnable and enqueue it exactly once.
pub fn wake_thread(thread: *mut EThread) {
    if thread.is_null() {
        return;
    }

    let thread_ref = unsafe { &mut *thread };
    if thread_ref.state == ThreadState::Waiting {
        finish_timed_wait(thread);
        finish_completion_wait(thread);
        clear_wait_registration(thread);
        thread_ref.state = ThreadState::Ready;
        enqueue(thread);
    }
}

fn begin_timed_wait(thread: *mut EThread, deadline: u64) {
    let thread_ref = unsafe { &mut *thread };
    thread_ref.wait_deadline = Some(deadline);
    thread_ref.wait_timed_out = false;

    let mut waiters = TIMED_WAITERS.lock();
    let insertion = waiters
        .iter()
        .position(|entry| {
            let queued_deadline = unsafe { (*from_tp(*entry)).wait_deadline };
            matches!(queued_deadline, Some(queued) if deadline < queued)
        })
        .unwrap_or(waiters.len());
    waiters.insert(insertion, tp(thread));
}

fn finish_timed_wait(thread: *mut EThread) {
    let thread_ref = unsafe { &mut *thread };
    thread_ref.wait_deadline = None;
    thread_ref.wait_timed_out = false;
    TIMED_WAITERS.lock().retain(|entry| *entry != tp(thread));
}

fn begin_completion_wait(thread: *mut EThread) {
    let mut waiters = COMPLETION_WAITERS.lock();
    assert!(
        !waiters.contains(&tp(thread)),
        "thread registered duplicate completion wait"
    );
    waiters.push_back(tp(thread));
}

fn finish_completion_wait(thread: *mut EThread) {
    COMPLETION_WAITERS
        .lock()
        .retain(|entry| *entry != tp(thread));
}

fn take_completion_waiters_matching(mut matches: impl FnMut(&EThread) -> bool) -> Vec<ThreadPtr> {
    let mut waiters = COMPLETION_WAITERS.lock();
    let mut survivors = VecDeque::new();
    let mut removed = Vec::new();
    while let Some(thread_ptr) = waiters.pop_front() {
        let thread = unsafe { &mut *from_tp(thread_ptr) };
        if matches(thread) {
            debug_assert_eq!(
                thread.state,
                ThreadState::Waiting,
                "completion waiter was not blocked"
            );
            thread.state = ThreadState::Terminated;
            removed.push(thread_ptr);
        } else {
            survivors.push_back(thread_ptr);
        }
    }
    *waiters = survivors;
    removed
}

fn take_wait_timeout(thread: *mut EThread) -> bool {
    let thread_ref = unsafe { &mut *thread };
    let timed_out = thread_ref.wait_timed_out;
    thread_ref.wait_timed_out = false;
    timed_out
}

fn clear_wait_registration(thread: *mut EThread) {
    let target = unsafe { (&mut *thread).wait_target.take() };
    if let Some(target) = target {
        target.cancel_waiter(tp(thread));
    }
}

fn expire_timed_waiters(now: u64) {
    loop {
        let expired = {
            let mut waiters = TIMED_WAITERS.lock();
            let Some(thread_ptr) = waiters.front().copied() else {
                return;
            };
            let thread = unsafe { &mut *from_tp(thread_ptr) };
            match (thread.state, thread.wait_deadline) {
                (ThreadState::Waiting, Some(deadline)) if deadline <= now => {
                    waiters.pop_front();
                    thread.wait_deadline = None;
                    thread.wait_timed_out = true;
                    thread.state = ThreadState::Ready;
                    Some(thread_ptr)
                }
                (_, None) | (ThreadState::Ready | ThreadState::Terminated, _) => {
                    waiters.pop_front();
                    None
                }
                (ThreadState::Waiting, Some(_)) => return,
                (ThreadState::Running, _) => unreachable!("running thread cannot be waiting"),
            }
        };
        if let Some(thread_ptr) = expired {
            finish_completion_wait(from_tp(thread_ptr));
            clear_wait_registration(from_tp(thread_ptr));
            enqueue(from_tp(thread_ptr));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    NoCurrentThread,
    InvalidHandle,
    AccessDenied,
    SelfWait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Signaled(i32),
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateProcessError {
    NoCurrentThread,
    SelfTarget,
    Exited,
    Terminating,
    MissingThread,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateThreadError {
    NoCurrentThread,
    SelfTarget,
    Exited,
    MissingThread,
}

fn wake_waiters(waiters: impl IntoIterator<Item = usize>) {
    for waiter in waiters {
        wake_thread(from_tp(waiter));
    }
}

fn irq_was_masked_and_disable() -> bool {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, DAIF", out(reg) daif);
        core::arch::asm!("msr DAIFSet, #2");
    }
    daif & (1 << 7) != 0
}

fn restore_irq_mask(was_masked: bool) {
    if !was_masked {
        unsafe { core::arch::asm!("msr DAIFClr, #2") };
    }
}

fn wait_for_completion(
    current: *mut EThread,
    timeout_ticks: Option<u64>,
    target: WaitTarget,
) -> WaitOutcome {
    let mut timeout_registered = false;
    loop {
        let was_masked = irq_was_masked_and_disable();
        if let Some(status) = target.observe_or_register_waiter(tp(current)) {
            if timeout_registered {
                finish_timed_wait(current);
            }
            clear_wait_registration(current);
            restore_irq_mask(was_masked);
            return WaitOutcome::Signaled(status);
        }

        unsafe {
            (*current).wait_target = Some(target.clone());
        }

        if let Some(ticks) = timeout_ticks.filter(|_| !timeout_registered) {
            let deadline = TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed) + ticks;
            begin_timed_wait(current, deadline);
            timeout_registered = true;
        }

        let waiting = mark_current_waiting();
        debug_assert_eq!(waiting, Some(current));
        begin_completion_wait(current);
        schedule();
        if timeout_registered && take_wait_timeout(current) {
            finish_timed_wait(current);
            clear_wait_registration(current);
            restore_irq_mask(was_masked);
            return WaitOutcome::TimedOut;
        }
        restore_irq_mask(was_masked);
    }
}

/// Wait for a typed process or thread handle owned by the active process.
/// Returns the target's exit status after it is signaled.
pub fn wait_for_handle(handle: Handle) -> Result<i32, WaitError> {
    match wait_for_handle_with_timeout(handle, None)? {
        WaitOutcome::Signaled(status) => Ok(status),
        WaitOutcome::TimedOut => unreachable!("infinite wait cannot time out"),
    }
}

/// Wait for a typed process or thread handle, optionally until a relative
/// scheduler-tick deadline expires.
pub fn wait_for_handle_with_timeout(
    handle: Handle,
    timeout_ticks: Option<u64>,
) -> Result<WaitOutcome, WaitError> {
    let current = current_thread().ok_or(WaitError::NoCurrentThread)?;
    let owner = unsafe { Arc::clone(&(*current).process) };
    let object = match owner
        .handle_table
        .lock()
        .lookup_with_access(handle, HANDLE_ACCESS_WAIT)
    {
        Ok(object) => object,
        Err(HandleLookupError::Invalid) => return Err(WaitError::InvalidHandle),
        Err(HandleLookupError::AccessDenied) => return Err(WaitError::AccessDenied),
    };

    match object {
        HandleObject::Process(target) => {
            if Arc::ptr_eq(&owner, &target) {
                return Err(WaitError::SelfWait);
            }
            Ok(wait_for_completion(
                current,
                timeout_ticks,
                WaitTarget::Process(target),
            ))
        }
        HandleObject::Thread(target) => {
            if Arc::ptr_eq(&target, unsafe { &(*current).object }) {
                return Err(WaitError::SelfWait);
            }
            Ok(wait_for_completion(
                current,
                timeout_ticks,
                WaitTarget::Thread(target),
            ))
        }
    }
}

/// Destroy a newly created thread that was never made runnable.
///
/// This is used to roll back syscall setup failures before the raw execution
/// record has acquired a scheduler-visible lifetime.
pub fn discard_unstarted(thread_ptr: *mut EThread, status: i32) {
    assert!(!thread_ptr.is_null(), "cannot discard a null thread");
    let mut thread = unsafe { Box::from_raw(thread_ptr) };
    assert_eq!(
        thread.state,
        ThreadState::Ready,
        "cannot discard a thread that was scheduled"
    );
    let thread_waiters = thread.signal_exit(status);
    let process_waiters = thread.process.thread_exited(status);
    wake_waiters(thread_waiters);
    wake_waiters(process_waiters);
    drop(thread);
}

/// Terminate every non-current thread of a target process.
///
/// On the single-core scheduler, an externally targeted process cannot own
/// `CURRENT`; ready targets remain in `RUN_QUEUE`, while typed completion
/// waiters remain in `COMPLETION_WAITERS`. Their stacks are retired and
/// reclaimed after a different stack becomes active, preserving the
/// scheduler's stack-lifetime rule.
pub fn terminate_process(target: Arc<EProcess>, status: i32) -> Result<(), TerminateProcessError> {
    let current = current_thread().ok_or(TerminateProcessError::NoCurrentThread)?;
    if Arc::ptr_eq(&target, unsafe { &(*current).process }) {
        return Err(TerminateProcessError::SelfTarget);
    }

    let was_masked = irq_was_masked_and_disable();
    match target.begin_termination() {
        Ok(()) => {}
        Err(ProcessTerminationState::Exited) => {
            restore_irq_mask(was_masked);
            return Err(TerminateProcessError::Exited);
        }
        Err(ProcessTerminationState::Terminating) => {
            restore_irq_mask(was_masked);
            return Err(TerminateProcessError::Terminating);
        }
    }

    let mut target_threads = Vec::new();
    {
        let mut queue = RUN_QUEUE.lock();
        let mut survivors = VecDeque::new();
        while let Some(thread_ptr) = queue.pop_front() {
            let thread = unsafe { &mut *from_tp(thread_ptr) };
            if Arc::ptr_eq(&thread.process, &target) {
                debug_assert!(
                    matches!(thread.state, ThreadState::Ready | ThreadState::Waiting),
                    "external termination found a non-queued target thread"
                );
                thread.state = ThreadState::Terminated;
                target_threads.push(thread_ptr);
            } else {
                survivors.push_back(thread_ptr);
            }
        }
        *queue = survivors;
    }
    target_threads.extend(take_completion_waiters_matching(|thread| {
        Arc::ptr_eq(&thread.process, &target)
    }));

    if target_threads.is_empty() {
        target.cancel_termination();
        restore_irq_mask(was_masked);
        return Err(TerminateProcessError::MissingThread);
    }

    let pid = target.pid.0;
    let cleared_wait_registrations = target_threads
        .iter()
        .filter(|thread_ptr| unsafe { (*from_tp(**thread_ptr)).wait_target.is_some() })
        .count();
    for thread_ptr in &target_threads {
        finish_timed_wait(from_tp(*thread_ptr));
        clear_wait_registration(from_tp(*thread_ptr));
    }
    for thread_ptr in &target_threads {
        let thread = unsafe { &mut *from_tp(*thread_ptr) };
        let thread_waiters = thread.signal_exit(status);
        let process_waiters = thread.process.thread_exited(status);
        wake_waiters(thread_waiters);
        wake_waiters(process_waiters);
        retire_thread(*thread_ptr);
    }
    restore_irq_mask(was_masked);
    log::info!(
        "Ps: externally terminated pid={} threads={} status={:#x}",
        pid,
        target_threads.len(),
        status as u32,
    );
    if cleared_wait_registrations != 0 {
        log::info!(
            "Ps: external process termination cleared {} typed wait registration(s)",
            cleared_wait_registrations,
        );
    }
    Ok(())
}

/// Terminate one non-current thread selected by its typed completion object.
///
/// The single-core scheduler keeps a non-current live target either ready in
/// `RUN_QUEUE` or blocked in `COMPLETION_WAITERS`. Its raw execution record is
/// retired only after another stack becomes active, while the handle-backed
/// completion object remains valid for waits after reaping.
pub fn terminate_thread(
    target: Arc<super::thread::ThreadObject>,
    status: i32,
) -> Result<(), TerminateThreadError> {
    let current = current_thread().ok_or(TerminateThreadError::NoCurrentThread)?;
    if Arc::ptr_eq(&target, unsafe { &(*current).object }) {
        return Err(TerminateThreadError::SelfTarget);
    }

    let was_masked = irq_was_masked_and_disable();
    let removed = {
        let mut queue = RUN_QUEUE.lock();
        let mut removed = None;
        let mut survivors = VecDeque::new();
        while let Some(thread_ptr) = queue.pop_front() {
            let thread = unsafe { &mut *from_tp(thread_ptr) };
            if Arc::ptr_eq(&thread.object, &target) {
                assert!(
                    removed.is_none(),
                    "external thread termination found a duplicate queue entry"
                );
                debug_assert!(
                    matches!(thread.state, ThreadState::Ready | ThreadState::Waiting),
                    "external thread termination found a non-queued target"
                );
                thread.state = ThreadState::Terminated;
                removed = Some(thread_ptr);
            } else {
                survivors.push_back(thread_ptr);
            }
        }
        *queue = survivors;
        removed
    };
    let removed = removed.or_else(|| {
        take_completion_waiters_matching(|thread| Arc::ptr_eq(&thread.object, &target)).pop()
    });

    let Some(thread_ptr) = removed else {
        let result = if target.exit_status().is_some() {
            Err(TerminateThreadError::Exited)
        } else {
            Err(TerminateThreadError::MissingThread)
        };
        restore_irq_mask(was_masked);
        return result;
    };

    let (
        thread_waiters,
        process_waiters,
        tid,
        pid,
        cleared_wait_registration,
        cleared_process_wait_registration,
        cleared_timed_wait,
    ) = unsafe {
        let cleared_wait_registration = (*from_tp(thread_ptr)).wait_target.is_some();
        let cleared_process_wait_registration = matches!(
            (*from_tp(thread_ptr)).wait_target.as_ref(),
            Some(WaitTarget::Process(_))
        );
        let cleared_timed_wait = (*from_tp(thread_ptr)).wait_deadline.is_some();
        finish_timed_wait(from_tp(thread_ptr));
        clear_wait_registration(from_tp(thread_ptr));
        let thread = &mut *from_tp(thread_ptr);
        let thread_waiters = thread.signal_exit(status);
        let process_waiters = thread.process.thread_exited(status);
        (
            thread_waiters,
            process_waiters,
            thread.tid.0,
            thread.process.pid.0,
            cleared_wait_registration,
            cleared_process_wait_registration,
            cleared_timed_wait,
        )
    };
    wake_waiters(thread_waiters);
    wake_waiters(process_waiters);
    retire_thread(thread_ptr);
    restore_irq_mask(was_masked);
    log::info!(
        "Ps: externally terminated thread tid={} pid={} status={:#x}",
        tid,
        pid,
        status as u32,
    );
    if cleared_wait_registration {
        log::info!("Ps: external thread termination cleared typed wait registration");
    }
    if cleared_process_wait_registration {
        log::info!("Ps: external thread termination cleared typed process wait registration");
    }
    if cleared_timed_wait {
        log::info!("Ps: external thread termination cleared finite typed wait");
    }
    Ok(())
}

/// Terminate the active thread and schedule another runnable thread.
///
/// This function does not return to the terminated context.
pub fn terminate_current(status: i32) -> ! {
    let current = current_thread().expect("terminate_current called without an active thread");
    let (thread_waiters, process_waiters, tid, pid) = unsafe {
        let thread = &mut *current;
        let thread_waiters = thread.signal_exit(status);
        let process_waiters = thread.process.thread_exited(status);
        thread.state = ThreadState::Terminated;
        (
            thread_waiters,
            process_waiters,
            thread.tid.0,
            thread.process.pid.0,
        )
    };
    wake_waiters(thread_waiters);
    wake_waiters(process_waiters);
    log::info!(
        "Ps: process pid={} thread tid={} exited with status={:#x}",
        pid,
        tid,
        status as u32,
    );

    schedule();
    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}

/// Terminate the current process as a single lifecycle operation.
///
/// The active caller cannot be reclaimed until another context is running, so
/// every queued sibling is retired first and the caller joins them only when
/// it switches away. All thread and process completions retain `status`.
pub fn terminate_current_process(status: i32) -> ! {
    let current = current_thread().expect("terminate_current_process without active thread");
    let process = unsafe { Arc::clone(&(*current).process) };
    let was_masked = irq_was_masked_and_disable();
    match process.begin_termination() {
        Ok(()) => {}
        Err(ProcessTerminationState::Exited | ProcessTerminationState::Terminating) => {
            restore_irq_mask(was_masked);
            terminate_current(status);
        }
    }

    let mut siblings = Vec::new();
    {
        let mut queue = RUN_QUEUE.lock();
        let mut survivors = VecDeque::new();
        while let Some(thread_ptr) = queue.pop_front() {
            let thread = unsafe { &mut *from_tp(thread_ptr) };
            if Arc::ptr_eq(&thread.process, &process) {
                debug_assert!(
                    matches!(thread.state, ThreadState::Ready | ThreadState::Waiting),
                    "current-process termination found a non-queued sibling"
                );
                thread.state = ThreadState::Terminated;
                siblings.push(thread_ptr);
            } else {
                survivors.push_back(thread_ptr);
            }
        }
        *queue = survivors;
    }
    siblings.extend(take_completion_waiters_matching(|thread| {
        Arc::ptr_eq(&thread.process, &process)
    }));

    let sibling_count = siblings.len();
    let cleared_wait_registrations = siblings
        .iter()
        .filter(|thread_ptr| unsafe { (*from_tp(**thread_ptr)).wait_target.is_some() })
        .count();
    for thread_ptr in &siblings {
        let thread = from_tp(*thread_ptr);
        finish_timed_wait(thread);
        clear_wait_registration(thread);
    }
    for thread_ptr in siblings {
        let thread = from_tp(thread_ptr);
        let thread = unsafe { &mut *thread };
        let thread_waiters = thread.signal_exit(status);
        let process_waiters = thread.process.thread_exited(status);
        wake_waiters(thread_waiters);
        wake_waiters(process_waiters);
        retire_thread(thread_ptr);
    }

    finish_timed_wait(current);
    clear_wait_registration(current);
    let (thread_waiters, process_waiters, pid) = unsafe {
        let thread = &mut *current;
        let thread_waiters = thread.signal_exit(status);
        let process_waiters = thread.process.thread_exited(status);
        thread.state = ThreadState::Terminated;
        (thread_waiters, process_waiters, thread.process.pid.0)
    };
    wake_waiters(thread_waiters);
    wake_waiters(process_waiters);
    log::info!(
        "Ps: current process pid={} terminated siblings={} status={:#x}",
        pid,
        sibling_count,
        status as u32,
    );
    if cleared_wait_registrations != 0 {
        log::info!(
            "Ps: current-process termination cleared {} typed wait registration(s)",
            cleared_wait_registrations,
        );
    }
    schedule();
    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}

/// Perform a round-robin context switch.
pub fn schedule() {
    reap_retired_threads();
    let mut queue = RUN_QUEUE.lock();

    let current_ptr = unsafe { CURRENT };
    let mut retiring_current = false;
    if current_ptr != 0 {
        let thread = unsafe { &mut *from_tp(current_ptr) };
        if thread.state == ThreadState::Running {
            thread.state = ThreadState::Ready;
            queue.push_back(current_ptr);
        } else if thread.state == ThreadState::Terminated {
            retiring_current = true;
        }
    }

    let next_ptr = loop {
        match queue.pop_front() {
            None => break 0usize,
            Some(t) => {
                let thread = unsafe { &mut *from_tp(t) };
                if thread.state == ThreadState::Ready {
                    thread.state = ThreadState::Running;
                    break t;
                }
                if thread.state == ThreadState::Waiting {
                    queue.push_back(t);
                }
            }
        }
    };

    if next_ptr == 0 {
        drop(queue);
        let from = if current_ptr == 0 {
            core::ptr::addr_of_mut!(IDLE_CONTEXT)
        } else {
            unsafe { &mut (*from_tp(current_ptr)).context as *mut _ }
        };
        if retiring_current {
            retire_thread(current_ptr);
        }
        unsafe {
            CURRENT = 0;
            crate::arch::mmu::activate_bootstrap_ttbr0();
            arch_context_switch(from, core::ptr::addr_of!(IDLE_CONTEXT));
        }
        return;
    }

    let prev_ctx = if current_ptr == 0 {
        core::ptr::addr_of_mut!(IDLE_CONTEXT)
    } else {
        unsafe { &mut (*from_tp(current_ptr)).context as *mut _ }
    };

    unsafe {
        CURRENT = next_ptr;
    }
    drop(queue);
    if retiring_current {
        retire_thread(current_ptr);
    }

    let next = unsafe { &mut *from_tp(next_ptr) };
    if next.mode == ThreadMode::User && !next.user_started {
        next.user_started = true;
        unsafe {
            arch_context_save_and_enter_user(
                prev_ctx,
                next.process.page_table_base,
                next.user_entry,
                next.user_stack_top,
                next.context.sp,
                next.user_argument,
            );
        };
        return;
    }

    unsafe {
        if next.mode == ThreadMode::User {
            crate::arch::mmu::activate_ttbr0(next.process.page_table_base);
        } else {
            crate::arch::mmu::activate_bootstrap_ttbr0();
        }
    }
    unsafe { arch_context_switch(prev_ctx, &next.context) };
}

/// Cooperative yield — push current thread back into the run queue and
/// let the scheduler pick the next ready thread.
pub fn yield_now() {
    unsafe { core::arch::asm!("msr DAIFSet, #2") };
    schedule();
    unsafe { core::arch::asm!("msr DAIFClr, #2") };
}

/// Read the ARM generic timer counter (free-running, counts at CNTFRQ_EL0 Hz).
#[inline]
pub fn cntpct() -> u64 {
    let cnt: u64;
    unsafe { core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) cnt) };
    cnt
}

/// Approx. counter ticks for 10 ms at 62.5 MHz (QEMU virt default).
const SLICE_TICKS: u64 = 625_000; // 62_500_000 / 100

/// The idle loop — drives cooperative preemption via the CNTPCT free-running
/// counter.  No WFI to avoid blocking when the timer IRQ is not wired.
pub fn idle_loop() -> ! {
    loop {
        // Cooperative time-slice: disable IRQs, switch, re-enable.
        unsafe { core::arch::asm!("msr DAIFSet, #2") };
        schedule();
        unsafe { core::arch::asm!("msr DAIFClr, #2") };

        // Burn the rest of the time slice before switching again.
        let deadline = cntpct().wrapping_add(SLICE_TICKS);
        while cntpct() < deadline {
            unsafe { core::arch::asm!("nop") };
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kernel System thread
// ─────────────────────────────────────────────────────────────────────────────

fn system_thread_main() {
    // Direct UART probe — visible even if the logger is broken
    crate::hal::uart::write_byte(b'[');
    for b in b"SYS_THREAD_START]" {
        crate::hal::uart::write_byte(*b);
    }
    crate::hal::uart::write_byte(b'\n');

    // Print timer frequency for diagnostics
    let freq: u64;
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq) };
    log::info!("CNTFRQ_EL0 = {} Hz  (slice = {} cycles)", freq, SLICE_TICKS);

    log::info!("System thread running");

    validate_handle_access_rights();
    validate_stale_handle_reuse();
    validate_handle_generation_exhaustion();
    validate_table_local_handle_collision();
    validate_invalid_handle_values();
    validate_external_thread_termination();
    validate_handle_lifecycle();

    while !super::initial_processes_complete() {
        yield_now();
    }
    // Let the scheduler reap the final EL0 thread before showing the prompt.
    yield_now();
    let mut shell = crate::shell::Shell::start();
    if CONSOLE_INPUT_PROBE.load(core::sync::atomic::Ordering::Relaxed) {
        // Only the private smoke disk requests this controlled init copy.
        // It exercises the handoff while the prompt is live and waits for
        // injected bytes, so must never run during an ordinary boot.
        let (probe_process, probe_thread) =
            super::create_process_from_source(super::INITIAL_IMAGE_SOURCE, 0x79)
                .expect("failed to start fixed console-input probe");
        enqueue(probe_thread);
        drop(probe_process);
    }

    loop {
        shell.poll();
        // Keep the serial console responsive without monopolising the CPU.
        yield_now();
    }
}

fn validate_handle_access_rights() {
    let owner = current_process().expect("handle access validation needs the System process");
    let current = current_thread().expect("handle access validation needs System thread");
    let target = unsafe { Arc::clone(&(*current).object) };

    let wait_only = owner
        .insert_thread_handle_with_access(target.clone(), HANDLE_ACCESS_WAIT)
        .expect("handle access probe exhausted slots");
    let terminate_only = owner
        .insert_thread_handle_with_access(target, HANDLE_ACCESS_TERMINATE)
        .expect("handle access probe exhausted slots");

    assert!(matches!(
        owner
            .handle_table
            .lock()
            .lookup_with_access(wait_only, HANDLE_ACCESS_TERMINATE),
        Err(HandleLookupError::AccessDenied)
    ));
    assert_eq!(
        wait_for_handle(terminate_only),
        Err(WaitError::AccessDenied)
    );
    assert!(owner.close_handle(wait_only));
    assert!(owner.close_handle(terminate_only));
    log::info!("Ps: typed handle access rights validated");
}

fn assert_stale_handle_rejected(
    owner: &Arc<EProcess>,
    old: Handle,
    replacement: Handle,
    replacement_access: HandleAccess,
) {
    assert_eq!(old as u32, replacement as u32, "slot was not reused");
    assert_ne!(old, replacement, "slot reused an old handle value");
    assert!(matches!(
        owner
            .handle_table
            .lock()
            .lookup_with_access(old, replacement_access),
        Err(HandleLookupError::Invalid)
    ));
    assert!(
        !owner.close_handle(old),
        "stale close removed a replacement"
    );
    assert!(owner
        .handle_table
        .lock()
        .lookup_with_access(replacement, replacement_access)
        .is_ok());
}

fn validate_stale_handle_reuse() {
    let owner = current_process().expect("handle reuse probe needs the System process");
    let current = current_thread().expect("handle reuse probe needs System thread");
    let thread = unsafe { Arc::clone(&(*current).object) };

    let old_thread = owner
        .insert_thread_handle(thread.clone())
        .expect("thread handle probe exhausted slots");
    assert!(owner.close_handle(old_thread));
    let replacement_thread = owner
        .insert_thread_handle_with_access(thread.clone(), HANDLE_ACCESS_TERMINATE)
        .expect("thread handle probe exhausted slots");
    assert_stale_handle_rejected(
        &owner,
        old_thread,
        replacement_thread,
        HANDLE_ACCESS_TERMINATE,
    );
    assert!(matches!(
        owner
            .handle_table
            .lock()
            .lookup_with_access(replacement_thread, HANDLE_ACCESS_WAIT),
        Err(HandleLookupError::AccessDenied)
    ));
    assert!(owner.close_handle(replacement_thread));

    let old_process = owner
        .insert_process_handle(EProcess::new_kernel_process())
        .expect("process handle probe exhausted slots");
    assert!(owner.close_handle(old_process));
    let replacement_process = owner
        .insert_process_handle(EProcess::new_kernel_process())
        .expect("process handle probe exhausted slots");
    assert_stale_handle_rejected(&owner, old_process, replacement_process, HANDLE_ACCESS_WAIT);
    assert!(owner.close_handle(replacement_process));

    let old_process = owner
        .insert_process_handle(EProcess::new_kernel_process())
        .expect("cross-type handle probe exhausted slots");
    assert!(owner.close_handle(old_process));
    let replacement_thread = owner
        .insert_thread_handle(thread.clone())
        .expect("cross-type handle probe exhausted slots");
    assert_stale_handle_rejected(&owner, old_process, replacement_thread, HANDLE_ACCESS_WAIT);
    assert!(owner.close_handle(replacement_thread));

    let old_thread = owner
        .insert_thread_handle(thread)
        .expect("cross-type handle probe exhausted slots");
    assert!(owner.close_handle(old_thread));
    let replacement_process = owner
        .insert_process_handle(EProcess::new_kernel_process())
        .expect("cross-type handle probe exhausted slots");
    assert_stale_handle_rejected(&owner, old_thread, replacement_process, HANDLE_ACCESS_WAIT);
    assert!(owner.close_handle(replacement_process));
    log::info!("Ps: stale typed handle reuse rejected");
}

fn validate_handle_generation_exhaustion() {
    let current = current_thread().expect("generation probe needs System thread");
    let object = HandleObject::Thread(unsafe { Arc::clone(&(*current).object) });
    crate::executive::ob::handle::probe_generation_exhaustion(object);
    log::info!("Ps: exhausted typed handle slot skipped");
}

fn validate_table_local_handle_collision() {
    let current = current_thread().expect("collision probe needs System thread");
    let thread = unsafe { Arc::clone(&(*current).object) };
    let process = EProcess::new_kernel_process();
    crate::executive::ob::handle::probe_table_local_collision(thread, process);
    log::info!("Ps: process-local numeric handle collision validated");
}

fn validate_invalid_handle_values() {
    let current = current_thread().expect("invalid-value probe needs System thread");
    let object = HandleObject::Thread(unsafe { Arc::clone(&(*current).object) });
    crate::executive::ob::handle::probe_invalid_handle_values(object);
    log::info!("Ps: malformed typed handle values rejected");
}

fn validate_external_thread_termination() {
    const TERMINATED_STATUS: i32 = 0x54;

    let owner = current_process().expect("thread termination validation needs System process");
    let current = current_thread().expect("thread termination validation needs System thread");

    let self_handle = owner
        .insert_thread_handle(unsafe { Arc::clone(&(*current).object) })
        .expect("self-target probe exhausted slots");
    let self_target = match owner
        .handle_table
        .lock()
        .lookup_with_access(self_handle, HANDLE_ACCESS_TERMINATE)
    {
        Ok(HandleObject::Thread(target)) => target,
        _ => panic!("self thread handle changed type"),
    };
    assert_eq!(
        terminate_thread(self_target, TERMINATED_STATUS),
        Err(TerminateThreadError::SelfTarget),
    );
    assert!(owner.close_handle(self_handle));
    log::info!("Ps: external current-thread termination rejected");

    let target_process = EProcess::new_kernel_process();
    let target_thread = EThread::new_kernel(target_process, externally_terminated_thread, 4);
    let target_object = unsafe { Arc::clone(&(*target_thread).object) };
    let target_handle = owner
        .insert_thread_handle(target_object)
        .expect("thread termination probe exhausted slots");
    enqueue(target_thread);

    let queued_target = match owner
        .handle_table
        .lock()
        .lookup_with_access(target_handle, HANDLE_ACCESS_TERMINATE)
    {
        Ok(HandleObject::Thread(target)) => target,
        _ => panic!("queued thread handle changed type"),
    };
    assert_eq!(terminate_thread(queued_target, TERMINATED_STATUS), Ok(()));
    assert_eq!(wait_for_handle(target_handle), Ok(TERMINATED_STATUS));
    log::info!("Ps: external queued-thread termination validated");

    let completed_target = match owner
        .handle_table
        .lock()
        .lookup_with_access(target_handle, HANDLE_ACCESS_TERMINATE)
    {
        Ok(HandleObject::Thread(target)) => target,
        _ => panic!("completed thread handle changed type"),
    };
    assert_eq!(
        terminate_thread(completed_target, TERMINATED_STATUS + 1),
        Err(TerminateThreadError::Exited),
    );
    assert!(owner.close_handle(target_handle));
    log::info!("Ps: external completed-thread termination validated");
}

fn externally_terminated_thread() {
    loop {
        yield_now();
    }
}

fn validate_handle_lifecycle() {
    let owner = current_process().expect("handle validation needs the System process");
    let target_process = EProcess::new_kernel_process();
    let target_thread = EThread::new_kernel(target_process.clone(), handle_test_thread, 4);
    let target_object = unsafe { Arc::clone(&(*target_thread).object) };
    let process_handle = owner
        .insert_process_handle(target_process)
        .expect("process wait probe exhausted slots");
    let thread_handle = owner
        .insert_thread_handle(target_object)
        .expect("thread wait probe exhausted slots");
    enqueue(target_thread);

    assert_eq!(wait_for_handle(thread_handle), Ok(0));
    assert_eq!(wait_for_handle(process_handle), Ok(0));
    assert!(owner.close_handle(thread_handle));
    assert!(owner.close_handle(process_handle));
    log::info!("Ps: typed process and thread handle waits validated");
}

fn handle_test_thread() {
    terminate_current(0);
}

// End of scheduler.
