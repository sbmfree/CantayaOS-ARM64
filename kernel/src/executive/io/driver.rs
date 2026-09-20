//! DriverObject and DeviceObject.

use super::irp::{Irp, IrpMajorFunction};

pub type DispatchFn = fn(*mut DeviceObject, *mut Irp);

/// NT dispatch table size (IRP_MJ_MAXIMUM_FUNCTION + 1 = 28).
pub const DISPATCH_TABLE_SIZE: usize = 28;

pub struct DriverObject {
    pub name: &'static str,
    pub dispatch_table: [Option<DispatchFn>; DISPATCH_TABLE_SIZE],
}

impl DriverObject {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            dispatch_table: [None; DISPATCH_TABLE_SIZE],
        }
    }

    pub const fn with_read_dispatch(name: &'static str, read: DispatchFn) -> Self {
        let mut dispatch_table = [None; DISPATCH_TABLE_SIZE];
        dispatch_table[IrpMajorFunction::Read as usize] = Some(read);
        Self {
            name,
            dispatch_table,
        }
    }
}

pub struct DeviceObject {
    pub driver: *const DriverObject,
    pub next: *mut DeviceObject, // next device on the stack
    pub flags: u32,
    pub characteristics: u32,
    /// Driver-specific extension immediately follows (in a real allocation).
    pub extension: *mut (),
}

impl DeviceObject {
    pub fn new(driver: *const DriverObject) -> Self {
        DeviceObject {
            driver,
            next: core::ptr::null_mut(),
            flags: 0,
            characteristics: 0,
            extension: core::ptr::null_mut(),
        }
    }
}
