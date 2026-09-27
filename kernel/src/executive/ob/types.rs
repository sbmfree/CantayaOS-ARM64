//! `ObjectHeader` and `ObjectType` — core of the NT Object Manager.

/// Dispatch table for a kernel object type.
pub struct ObjectType {
    pub name: &'static str,
    /// Called when the last reference is dropped.
    pub delete: fn(*mut ObjectHeader),
}

/// Every kernel object begins with this header (NT layout mirrors this).
pub struct ObjectHeader {
    pub ty: &'static ObjectType,
    /// Size of the associated object for diagnostics; the header does not
    /// allocate a separate body. Reference counting belongs to its `Arc`.
    pub body_size: usize,
}

impl ObjectHeader {
    pub fn new(ty: &'static ObjectType, body_size: usize) -> Self {
        Self { ty, body_size }
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
