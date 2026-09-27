//! `HandleTable` — maps opaque `Handle` integers to kernel objects.
//!
//! Each process has its own handle table. A handle contains a table slot and
//! its issuance generation, so closing and reusing a slot cannot revive an
//! older value.
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
const PSEUDO_CURRENT_PROCESS: Handle = u64::MAX;
const SLOT_MASK: Handle = u32::MAX as Handle;
const EXHAUSTED_GENERATION: u32 = u32::MAX;

fn encode_handle(index: usize, generation: u32) -> Option<Handle> {
    if generation == EXHAUSTED_GENERATION {
        return None;
    }
    let slot_number = u32::try_from(index.checked_add(1)?).ok()?;
    let handle = ((generation as Handle) << 32) | Handle::from(slot_number);
    (handle != INVALID_HANDLE && handle != PSEUDO_CURRENT_PROCESS).then_some(handle)
}

fn decode_handle(handle: Handle) -> Option<(usize, u32)> {
    if handle == INVALID_HANDLE || handle == PSEUDO_CURRENT_PROCESS {
        return None;
    }
    let slot_number = (handle & SLOT_MASK) as u32;
    (slot_number != 0).then_some(((slot_number - 1) as usize, (handle >> 32) as u32))
}

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

struct Slot {
    generation: u32,
    entry: Option<Entry>,
}

pub struct HandleTable {
    slots: Vec<Slot>,
}

impl HandleTable {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Insert an object and return its handle, or fail if no encodable slot
    /// remains. An exhausted generation is never wrapped and reused.
    pub fn insert(&mut self, object: HandleObject, access_mask: HandleAccess) -> Option<Handle> {
        match &object {
            HandleObject::Process(process) => {
                debug_assert!(core::ptr::eq(process.object_header.ty, &OB_TYPE_PROCESS));
            }
            HandleObject::Thread(thread) => {
                debug_assert!(core::ptr::eq(thread.object_header.ty, &OB_TYPE_THREAD));
            }
        }

        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.entry.is_none() && slot.generation != EXHAUSTED_GENERATION {
                let handle = encode_handle(index, slot.generation)?;
                slot.entry = Some(Entry {
                    object,
                    access_mask,
                });
                return Some(handle);
            }
        }
        let handle = encode_handle(self.slots.len(), 0)?;
        self.slots.push(Slot {
            generation: 0,
            entry: Some(Entry {
                object,
                access_mask,
            }),
        });
        Some(handle)
    }

    /// Look up a typed object only after its requested lifecycle rights pass.
    pub fn lookup_with_access(
        &self,
        handle: Handle,
        required_access: HandleAccess,
    ) -> Result<HandleObject, HandleLookupError> {
        let (index, generation) = decode_handle(handle).ok_or(HandleLookupError::Invalid)?;
        let entry = self
            .slots
            .get(index)
            .filter(|slot| slot.generation == generation)
            .and_then(|slot| slot.entry.as_ref())
            .ok_or(HandleLookupError::Invalid)?;
        if entry.access_mask & required_access != required_access {
            return Err(HandleLookupError::AccessDenied);
        }
        Ok(entry.object.clone())
    }

    /// Close (remove) a handle.  Returns `true` if it existed.
    pub fn close(&mut self, handle: Handle) -> bool {
        let Some((index, generation)) = decode_handle(handle) else {
            return false;
        };
        let Some(slot) = self.slots.get_mut(index) else {
            return false;
        };
        if slot.generation != generation || slot.entry.is_none() {
            return false;
        }
        slot.entry = None;
        slot.generation = slot.generation.saturating_add(1);
        true
    }
}

/// Boot-only, private-table boundary probe. Seed a closed slot immediately
/// before exhaustion so the normal insert/close path exercises its last
/// usable generation without billions of intermediate issuances.
pub(crate) fn probe_generation_exhaustion(object: HandleObject) {
    let mut table = HandleTable::new();
    let initial = table
        .insert(object.clone(), HANDLE_ACCESS_WAIT)
        .expect("generation probe could not issue initial handle");
    assert_eq!(decode_handle(initial), Some((0, 0)));
    assert!(table.close(initial));

    table.slots[0].generation = EXHAUSTED_GENERATION - 1;
    let last_usable = table
        .insert(object.clone(), HANDLE_ACCESS_WAIT)
        .expect("generation probe could not issue last usable handle");
    assert_eq!(
        decode_handle(last_usable),
        Some((0, EXHAUSTED_GENERATION - 1))
    );
    assert!(table
        .lookup_with_access(last_usable, HANDLE_ACCESS_WAIT)
        .is_ok());
    assert!(table.close(last_usable));
    assert_eq!(table.slots[0].generation, EXHAUSTED_GENERATION);
    assert!(matches!(
        table.lookup_with_access(last_usable, HANDLE_ACCESS_WAIT),
        Err(HandleLookupError::Invalid)
    ));
    assert!(!table.close(last_usable));

    let replacement = table
        .insert(object.clone(), HANDLE_ACCESS_WAIT)
        .expect("generation probe could not skip exhausted slot");
    assert_eq!(decode_handle(replacement), Some((1, 0)));
    assert_ne!(replacement, last_usable);
    assert!(!table.close(last_usable));
    assert!(!table.close(initial));
    assert!(matches!(
        table.lookup_with_access(initial, HANDLE_ACCESS_WAIT),
        Err(HandleLookupError::Invalid)
    ));
    assert!(matches!(
        table.lookup_with_access(last_usable, HANDLE_ACCESS_WAIT),
        Err(HandleLookupError::Invalid)
    ));
    assert!(table
        .lookup_with_access(replacement, HANDLE_ACCESS_WAIT)
        .is_ok());
    assert!(matches!(
        table.lookup_with_access(replacement, HANDLE_ACCESS_TERMINATE),
        Err(HandleLookupError::AccessDenied)
    ));
    assert!(table.close(replacement));

    let reused_second_slot = table
        .insert(object, HANDLE_ACCESS_WAIT)
        .expect("generation probe could not reuse second slot");
    assert_eq!(decode_handle(reused_second_slot), Some((1, 1)));
    assert!(table.close(reused_second_slot));
}

/// Boot-only probe: equal numbers in distinct nonempty tables name only the
/// object and rights held by the table used for lookup.
pub(crate) fn probe_table_local_collision(thread: Arc<ThreadObject>, process: Arc<EProcess>) {
    let mut thread_table = HandleTable::new();
    let mut process_table = HandleTable::new();
    let thread_handle = thread_table
        .insert(HandleObject::Thread(thread.clone()), HANDLE_ACCESS_WAIT)
        .expect("thread collision probe could not issue handle");
    let process_handle = process_table
        .insert(
            HandleObject::Process(process.clone()),
            HANDLE_ACCESS_TERMINATE,
        )
        .expect("process collision probe could not issue handle");
    assert_eq!(thread_handle, process_handle);
    assert!(matches!(
        thread_table.lookup_with_access(thread_handle, HANDLE_ACCESS_WAIT),
        Ok(HandleObject::Thread(found)) if Arc::ptr_eq(&found, &thread)
    ));
    assert!(matches!(
        process_table.lookup_with_access(process_handle, HANDLE_ACCESS_TERMINATE),
        Ok(HandleObject::Process(found)) if Arc::ptr_eq(&found, &process)
    ));
    assert!(matches!(
        thread_table.lookup_with_access(thread_handle, HANDLE_ACCESS_TERMINATE),
        Err(HandleLookupError::AccessDenied)
    ));
    assert!(matches!(
        process_table.lookup_with_access(process_handle, HANDLE_ACCESS_WAIT),
        Err(HandleLookupError::AccessDenied)
    ));

    assert!(thread_table.close(thread_handle));
    assert!(matches!(
        thread_table.lookup_with_access(thread_handle, HANDLE_ACCESS_WAIT),
        Err(HandleLookupError::Invalid)
    ));
    assert!(matches!(
        process_table.lookup_with_access(process_handle, HANDLE_ACCESS_TERMINATE),
        Ok(HandleObject::Process(found)) if Arc::ptr_eq(&found, &process)
    ));

    let replacement = thread_table
        .insert(HandleObject::Thread(thread.clone()), HANDLE_ACCESS_WAIT)
        .expect("thread collision probe could not reuse slot");
    assert_eq!(decode_handle(replacement), Some((0, 1)));
    assert_ne!(replacement, process_handle);
    assert!(!thread_table.close(process_handle));
    assert!(!process_table.close(replacement));
    assert!(matches!(
        thread_table.lookup_with_access(replacement, HANDLE_ACCESS_WAIT),
        Ok(HandleObject::Thread(found)) if Arc::ptr_eq(&found, &thread)
    ));
    assert!(matches!(
        process_table.lookup_with_access(process_handle, HANDLE_ACCESS_TERMINATE),
        Ok(HandleObject::Process(found)) if Arc::ptr_eq(&found, &process)
    ));
    assert!(matches!(
        process_table.lookup_with_access(process_handle, HANDLE_ACCESS_WAIT),
        Err(HandleLookupError::AccessDenied)
    ));
    assert!(thread_table.close(replacement));
    assert!(process_table.close(process_handle));
}

/// Boot-only negative probe for values that this private table has not issued.
pub(crate) fn probe_invalid_handle_values(object: HandleObject) {
    let mut table = HandleTable::new();
    let live = table
        .insert(object, HANDLE_ACCESS_WAIT)
        .expect("invalid-value probe could not issue live handle");
    assert_eq!(decode_handle(live), Some((0, 0)));

    let invalid_values = [
        INVALID_HANDLE,
        PSEUDO_CURRENT_PROCESS,
        1u64 << 32,       // zero slot number
        2,                // unallocated slot
        (1u64 << 32) | 1, // generation not yet issued
        (u64::from(EXHAUSTED_GENERATION) << 32) | 1,
        u64::from(u32::MAX), // out-of-range slot number
    ];
    for value in invalid_values {
        assert_ne!(value, live);
        assert!(matches!(
            table.lookup_with_access(value, HANDLE_ACCESS_TERMINATE),
            Err(HandleLookupError::Invalid)
        ));
        assert!(!table.close(value));
        assert!(table.lookup_with_access(live, HANDLE_ACCESS_WAIT).is_ok());
        assert!(matches!(
            table.lookup_with_access(live, HANDLE_ACCESS_TERMINATE),
            Err(HandleLookupError::AccessDenied)
        ));
    }
    assert!(table.close(live));
}
