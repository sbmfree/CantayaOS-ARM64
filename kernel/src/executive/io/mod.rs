//! Io — I/O Manager.
//!
//! NT-style I/O model:
//!   IRP (I/O Request Packet) → DriverObject dispatch table → DeviceObject stack
//!
//! Sub-modules:
//!   irp    — IRP structure
//!   driver — DriverObject + DeviceObject

pub mod driver;
pub mod image;
pub mod irp;

/// One-time initialisation.
pub fn init() {
    log::debug!("Io: I/O Manager ready");
}

/// Route an IRP to the top of a device stack.
///
/// # Safety
///
/// `device` and `irp` must remain valid for the complete synchronous dispatch.
pub unsafe fn io_call_driver(device: *mut driver::DeviceObject, irp: *mut irp::Irp) {
    let Some(request) = (unsafe { irp.as_mut() }) else {
        return;
    };
    let Some(dev) = (unsafe { device.as_ref() }) else {
        request.complete(irp::STATUS_INVALID_PARAMETER);
        return;
    };
    let Some(driver) = (unsafe { dev.driver.as_ref() }) else {
        request.complete(irp::STATUS_INVALID_PARAMETER);
        return;
    };
    let Some(location) = request.current_location() else {
        request.complete(irp::STATUS_INVALID_PARAMETER);
        return;
    };
    let function = irp::IrpMajorFunction::from_u8(location.major_function);
    if function == irp::IrpMajorFunction::Unknown {
        request.complete(irp::STATUS_NOT_SUPPORTED);
        return;
    }
    if let Some(dispatch) = driver.dispatch_table[function as usize] {
        dispatch(device, irp);
    } else {
        request.complete(irp::STATUS_NOT_SUPPORTED);
    }
}
