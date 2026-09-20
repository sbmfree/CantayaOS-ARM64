//! `ObjectHeader` and `ObjectType` — core of the NT Object Manager.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Dispatch table for a kernel object type.
pub struct ObjectType {
    pub name: &'static str,
    /// Called when the last reference is dropped.
    pub delete: fn(*mut ObjectHeader),
}

/// Every kernel object begins with this header (NT layout mirrors this).
pub struct ObjectHeader {
    pub ty: &'static ObjectType,
    /// Reference count managed by `Arc` (Arc IS the ref count here).
    /// Kept for diagnostic use.
    pub ref_count: AtomicUsize,
    /// Raw allocation for the object body (type-erased).
    /// Zero means no body was allocated separately.
    pub body_size: usize,
}

impl ObjectHeader {
    pub fn new(ty: &'static ObjectType, body_size: usize) -> Self {
        Self {
            ty,
            ref_count: AtomicUsize::new(1),
            body_size,
        }
    }
}

// Declare well-known built-in object types ────────────────────────────────────

fn noop_delete(_: *mut ObjectHeader) {}

pub static OB_TYPE_PROCESS: ObjectType = ObjectType {
    name: "Process",
    delete: noop_delete,
};
pub static OB_TYPE_THREAD: ObjectType = ObjectType {
    name: "Thread",
    delete: noop_delete,
};
pub static OB_TYPE_EVENT: ObjectType = ObjectType {
    name: "Event",
    delete: noop_delete,
};
pub static OB_TYPE_SECTION: ObjectType = ObjectType {
    name: "Section",
    delete: noop_delete,
};
pub static OB_TYPE_FILE: ObjectType = ObjectType {
    name: "File",
    delete: noop_delete,
};
