//! IRP — I/O Request Packet.

use alloc::vec::Vec;

pub const STATUS_SUCCESS: u32 = 0x0000_0000;
pub const STATUS_UNSUCCESSFUL: u32 = 0xC000_0001;
pub const STATUS_NOT_SUPPORTED: u32 = 0xC000_0002;
pub const STATUS_INVALID_PARAMETER: u32 = 0xC000_000D;
pub const STATUS_NO_SUCH_FILE: u32 = 0xC000_000F;
pub const STATUS_FILE_INVALID: u32 = 0xC000_0098;

/// Major function codes (matches NT IRP_MJ_* values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IrpMajorFunction {
    Create = 0,
    Close = 2,
    Read = 3,
    Write = 4,
    QueryInfo = 5,
    SetInfo = 6,
    DeviceControl = 14,
    InternalDevCtl = 15,
    Shutdown = 16,
    Cleanup = 18,
    Unknown = 0xFF,
}

impl IrpMajorFunction {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Create,
            2 => Self::Close,
            3 => Self::Read,
            4 => Self::Write,
            5 => Self::QueryInfo,
            6 => Self::SetInfo,
            14 => Self::DeviceControl,
            15 => Self::InternalDevCtl,
            16 => Self::Shutdown,
            18 => Self::Cleanup,
            _ => Self::Unknown,
        }
    }
}

/// Per-stack-location parameters.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IoStackLocation {
    pub major_function: u8,
    pub minor_function: u8,
    pub flags: u8,
    pub _pad: u8,
    pub parameters: [u64; 4],
    pub device: *mut super::driver::DeviceObject,
    pub completion: Option<fn(*mut super::driver::DeviceObject, *mut Irp, usize) -> u32>,
}

impl IoStackLocation {
    const fn empty() -> Self {
        Self {
            major_function: IrpMajorFunction::Unknown as u8,
            minor_function: 0,
            flags: 0,
            _pad: 0,
            parameters: [0; 4],
            device: core::ptr::null_mut(),
            completion: None,
        }
    }
}

/// I/O Request Packet.
#[repr(C)]
pub struct Irp {
    pub status: u32,
    pub information: usize,
    pub stack_count: u8,
    pub current_idx: u8,
    _pad: [u8; 6],
    pub stack: [IoStackLocation; 8],
}

impl Irp {
    /// Construct a synchronous read request whose buffer is owned by the
    /// kernel caller for the duration of dispatch.
    pub(super) fn kernel_read(output: &mut Vec<u8>) -> Self {
        output.clear();
        let mut irp = Self {
            status: STATUS_UNSUCCESSFUL,
            information: 0,
            stack_count: 1,
            current_idx: 0,
            _pad: [0; 6],
            stack: [IoStackLocation::empty(); 8],
        };
        irp.stack[0] = IoStackLocation {
            major_function: IrpMajorFunction::Read as u8,
            minor_function: 0,
            flags: 0,
            _pad: 0,
            parameters: [output as *mut Vec<u8> as u64, 0, 0, 0],
            device: core::ptr::null_mut(),
            completion: None,
        };
        irp
    }

    pub fn current_location(&self) -> Option<&IoStackLocation> {
        let index = self.current_idx as usize;
        (index < self.stack_count as usize)
            .then(|| self.stack.get(index))
            .flatten()
    }

    /// Return the private kernel buffer supplied by [`Self::kernel_read`].
    ///
    /// # Safety
    ///
    /// The caller must service the request synchronously before its creator
    /// drops the buffer passed to `kernel_read`.
    pub(super) unsafe fn kernel_read_buffer(&mut self) -> Option<&mut Vec<u8>> {
        let location = self.current_location()?;
        if location.major_function != IrpMajorFunction::Read as u8 || location.parameters[0] == 0 {
            return None;
        }
        unsafe { (location.parameters[0] as *mut Vec<u8>).as_mut() }
    }

    pub fn complete(&mut self, status: u32) {
        self.complete_with_information(status, 0);
    }

    pub fn complete_with_information(&mut self, status: u32, information: usize) {
        self.status = status;
        self.information = information;
    }
}
