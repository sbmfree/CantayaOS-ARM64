//! Kernel-owned, read-only executable image sources.

use alloc::vec::Vec;

use crate::drivers::{
    fat::LiveFatVolume,
    virtio_blk::{BOOT_DISK, MAX_BOOT_VOLUME_SECTORS},
};

use super::{
    driver::{DeviceObject, DriverObject},
    irp::{
        Irp, STATUS_FILE_INVALID, STATUS_INVALID_PARAMETER, STATUS_NO_SUCH_FILE, STATUS_SUCCESS,
    },
};

const MAX_EXECUTABLE_IMAGE_SIZE: usize = 2 * 1024 * 1024;

/// A fixed root-level FAT image source used by a kernel-owned file object.
///
/// Construction is intentionally private: no caller can supply a filename,
/// storage device, or size limit.
struct ReadOnlyImage {
    name83: &'static [u8; 11],
    label: &'static str,
    max_size: usize,
}

/// The private image backing the dedicated child file.
static FAT_CHILD_IMAGE: ReadOnlyImage = ReadOnlyImage {
    name83: b"CHILD   ELF",
    label: "CHILD.ELF",
    max_size: MAX_EXECUTABLE_IMAGE_SIZE,
};

/// A fixed kernel-owned read-only file object for an executable image.
///
/// This is intentionally not registered in the object-manager name directory:
/// no EL0 caller can enumerate, open, or substitute it by name.
pub struct ReadOnlyFile {
    label: &'static str,
    image: &'static ReadOnlyImage,
}

/// The only current kernel-owned executable file object.
pub static FAT_CHILD_FILE: ReadOnlyFile = ReadOnlyFile {
    label: "CHILD.ELF",
    image: &FAT_CHILD_IMAGE,
};

static FAT_IMAGE_DRIVER: DriverObject =
    DriverObject::with_read_dispatch("FatImage", dispatch_image_read);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageReadError {
    InvalidVolume,
    MissingImage,
    DispatchFailed(u32),
    InvalidCompletion,
}

impl ReadOnlyImage {
    /// Read the complete fixed image through a synchronous kernel-only IRP.
    fn read(&self) -> Result<Vec<u8>, ImageReadError> {
        let mut image = Vec::new();
        let mut irp = Irp::kernel_read(&mut image);
        let mut device = DeviceObject::new(&FAT_IMAGE_DRIVER);
        device.extension = (self as *const Self).cast_mut().cast();
        unsafe { super::io_call_driver(&mut device, &mut irp) };

        match irp.status {
            STATUS_SUCCESS if irp.information == image.len() => {
                log::info!(
                    "Io: IRP read-only image {} completed ({} bytes)",
                    self.label,
                    image.len()
                );
                Ok(image)
            }
            STATUS_SUCCESS => Err(ImageReadError::InvalidCompletion),
            STATUS_FILE_INVALID => Err(ImageReadError::InvalidVolume),
            STATUS_NO_SUCH_FILE => Err(ImageReadError::MissingImage),
            status => Err(ImageReadError::DispatchFailed(status)),
        }
    }

    fn read_from_fat(&self, image: &mut Vec<u8>) -> Result<(), ImageReadError> {
        let volume = LiveFatVolume::open_bounded(&BOOT_DISK, MAX_BOOT_VOLUME_SECTORS)
            .ok_or(ImageReadError::InvalidVolume)?;
        if !volume.read_file_to_buf_bounded(self.name83, image, self.max_size) {
            return Err(ImageReadError::MissingImage);
        }
        Ok(())
    }
}

impl ReadOnlyFile {
    /// Read the fixed executable file through its I/O-owned image source.
    pub fn read_image(&self) -> Result<Vec<u8>, ImageReadError> {
        log::info!("Io: read-only file {} dispatching image IRP", self.label);
        self.image.read()
    }
}

fn dispatch_image_read(device: *mut DeviceObject, irp: *mut Irp) {
    let Some(device) = (unsafe { device.as_ref() }) else {
        return;
    };
    let Some(image) = (unsafe { device.extension.cast::<ReadOnlyImage>().as_ref() }) else {
        unsafe { (*irp).complete(STATUS_INVALID_PARAMETER) };
        return;
    };
    let Some(request) = (unsafe { irp.as_mut() }) else {
        return;
    };
    let Some(output) = (unsafe { request.kernel_read_buffer() }) else {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    };
    let result = image.read_from_fat(output);
    let information = output.len();
    match result {
        Ok(()) => request.complete_with_information(STATUS_SUCCESS, information),
        Err(ImageReadError::InvalidVolume) => request.complete(STATUS_FILE_INVALID),
        Err(ImageReadError::MissingImage) => request.complete(STATUS_NO_SUCH_FILE),
        Err(_) => request.complete(STATUS_INVALID_PARAMETER),
    }
}
