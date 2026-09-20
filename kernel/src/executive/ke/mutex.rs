//! `KeMutex` — sleeping mutex.
//!
//! Threads that cannot acquire the mutex are added to a wait queue and
//! descheduled; the releasing thread wakes the first waiter.
//!
//! This is a simplified version of the NT KMUTEX / FAST_MUTEX.

use super::spinlock::KeSpinLock;
use crate::executive::ps::thread::EThread;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicBool, Ordering};

/// Opaque thread ID used by the wait queue.  Matches ETHREAD pointer.
pub type ThreadId = *mut EThread;

pub struct KeMutex {
    locked: AtomicBool,
    queue_lock: KeSpinLock,
    waiters: spin::Mutex<VecDeque<ThreadId>>,
}

unsafe impl Send for KeMutex {}
unsafe impl Sync for KeMutex {}

impl KeMutex {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            queue_lock: KeSpinLock::new(),
            waiters: spin::Mutex::new(VecDeque::new()),
        }
    }

    /// Try to acquire without blocking.  Returns `true` on success.
    pub fn try_acquire(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Block the current thread until the mutex is acquired.
    ///
    /// Before the scheduler starts there is no thread context to suspend, so
    /// early boot callers fall back to spinning.  Once a thread is active, a
    /// contended mutex puts it on the wait queue and schedules another thread.
    pub fn acquire(&self) {
        loop {
            if self.try_acquire() {
                return;
            }

            let Some(current) = crate::executive::ps::scheduler::current_thread() else {
                core::hint::spin_loop();
                continue;
            };

            {
                let _queue_guard = self.queue_lock.lock();

                // The mutex may have been released while we prepared to join
                // the wait queue.  Retest under the queue lock to avoid a
                // missed wakeup.
                if self.try_acquire() {
                    return;
                }

                let waiting = crate::executive::ps::scheduler::mark_current_waiting();
                debug_assert_eq!(waiting, Some(current));
                self.waiters.lock().push_back(current);
            }

            // `queue_lock` is dropped before the context switch: `release`
            // needs that lock to dequeue and wake this thread.
            crate::executive::ps::scheduler::schedule();
        }
    }

    /// Release the mutex and wake the first waiter (if any).
    pub fn release(&self) {
        let waiter = {
            let _queue_guard = self.queue_lock.lock();
            self.locked.store(false, Ordering::Release);
            self.waiters.lock().pop_front()
        };

        if let Some(waiter) = waiter {
            crate::executive::ps::scheduler::wake_thread(waiter);
        }
    }
}

/// RAII guard.
pub struct MutexGuard<'a> {
    mutex: &'a KeMutex,
}

impl<'a> MutexGuard<'a> {
    pub fn new(mutex: &'a KeMutex) -> Self {
        mutex.acquire();
        Self { mutex }
    }
}

impl Drop for MutexGuard<'_> {
    fn drop(&mut self) {
        self.mutex.release();
    }
}

impl KeMutex {
    pub fn lock(&self) -> MutexGuard<'_> {
        MutexGuard::new(self)
    }
}
