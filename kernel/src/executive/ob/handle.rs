//! `HandleTable` — maps opaque `Handle` integers to kernel objects.
//!
//! In NT each process has its own handle table.  A handle is a small integer
//! (multiple of 4 in real NT; here just an index+1).
//!
//! This module enforces the small lifecycle-rights subset needed before the
//! broader security model exists.

use crate::executive::ob::types::{OB_TYPE_PROCESS, OB_TYPE_THREAD};
use crate::executive::ps::{process::EProcess, thread::ThreadObject};
use alloc::{sync::Arc, vec::Vec};

/// Opaque handle value.  0 is the invalid/null handle.
pub type Handle = u64;
pub type HandleAccess = u32;

pub const HANDLE_ACCESS_WAIT: HandleAccess = 1 << 0;
pub const HANDLE_ACCESS_TERMINATE: HandleAccess = 1 << 1;

const INVALID_HANDLE: Handle = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleLookupError {
    Invalid,
    AccessDenied,
}

/// A strongly typed object retained by a process handle table.
///
/// Raw `ETHREAD` allocations remain scheduler-owned because their stacks must
/// be freed only after a context switch. A `ThreadObject` is instead a small
/// Arc-owned completion object that may safely outlive its execution record.
#[derive(Clone)]
pub enum HandleObject {
    Process(Arc<EProcess>),
    Thread(Arc<ThreadObject>),
}

struct Entry {
    object: HandleObject,
    access_mask: HandleAccess,
}

pub struct HandleTable {
    entries: Vec<Option<Entry>>,
}

impl HandleTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Insert an object and return its handle.
    pub fn insert(&mut self, object: HandleObject, access_mask: HandleAccess) -> Handle {
        match &object {
            HandleObject::Process(process) => {
                debug_assert!(core::ptr::eq(process.object_header.ty, &OB_TYPE_PROCESS));
            }
            HandleObject::Thread(thread) => {
                debug_assert!(core::ptr::eq(thread.object_header.ty, &OB_TYPE_THREAD));
            }
        }

        // Find a free slot
        for (i, slot) in self.entries.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(Entry {
                    object,
                    access_mask,
                });
                return (i + 1) as Handle;
            }
        }
        // No free slot — extend
        self.entries.push(Some(Entry {
            object,
            access_mask,
        }));
        self.entries.len() as Handle
    }

    /// Look up a typed object only after its requested lifecycle rights pass.
    pub fn lookup_with_access(
        &self,
        handle: Handle,
        required_access: HandleAccess,
    ) -> Result<HandleObject, HandleLookupError> {
        if handle == INVALID_HANDLE {
            return Err(HandleLookupError::Invalid);
        }
        let idx = (handle - 1) as usize;
        let entry = self
            .entries
            .get(idx)
            .and_then(Option::as_ref)
            .ok_or(HandleLookupError::Invalid)?;
        if entry.access_mask & required_access != required_access {
            return Err(HandleLookupError::AccessDenied);
        }
        Ok(entry.object.clone())
    }

    /// Close (remove) a handle.  Returns `true` if it existed.
    pub fn close(&mut self, handle: Handle) -> bool {
        if handle == INVALID_HANDLE {
            return false;
        }
        let idx = (handle - 1) as usize;
        if let Some(slot) = self.entries.get_mut(idx) {
            if slot.is_some() {
                *slot = None;
                return true;
            }
        }
        false
    }
}
