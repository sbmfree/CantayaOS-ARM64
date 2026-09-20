//! Ob — Object Manager.
//!
//! The NT Object Manager is responsible for:
//!   - Typed kernel objects with reference-counted lifetimes
//!   - Name space (simplified: flat string map for now)
//!   - Handle tables (per-process integer → object mappings)
//!
//! Key types: `ObjectHeader`, `ObjectType`, `HandleTable`.

pub mod handle;
pub mod types;

use alloc::{boxed::Box, collections::BTreeMap, string::String, sync::Arc};
use spin::Mutex;
use types::{ObjectHeader, ObjectType};

/// Global object name directory (maps name → Arc<ObjectHeader>).
static NAME_DIR: Mutex<BTreeMap<String, Arc<ObjectHeader>>> = Mutex::new(BTreeMap::new());

/// One-time initialisation (empty — no built-in objects yet).
pub fn init() {
    log::debug!("Ob: Object Manager ready");
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API  (NT-style names)
// ─────────────────────────────────────────────────────────────────────────────

/// Create a new kernel object of the given type.  Returns an `Arc` owning the header.
pub fn ob_create_object(ty: &'static ObjectType, body_size: usize) -> Arc<ObjectHeader> {
    Arc::new(ObjectHeader::new(ty, body_size))
}

/// Insert a named object into the global directory.
pub fn ob_insert_named(name: &str, obj: Arc<ObjectHeader>) {
    NAME_DIR.lock().insert(String::from(name), obj);
}

/// Look up a named object.
pub fn ob_open_object_by_name(name: &str) -> Option<Arc<ObjectHeader>> {
    NAME_DIR.lock().get(name).cloned()
}
