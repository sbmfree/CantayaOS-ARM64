//! Version-one desktop ABI. Pixels are packed 0x00RRGGBB, independent of GOP.
//! No physical addresses or kernel pointers cross this interface.

pub const QUERY_DISPLAY: u64 = 0x40;
pub const PRESENT: u64 = 0x41;
pub const READ_EVENT: u64 = 0x42;
pub const READ_OUTPUT: u64 = 0x43;
pub const MAX_BLIT_PIXELS: usize = 4096;
pub const KEY: u32 = 1;
pub const POINTER: u32 = 2;
pub const TEXT: u32 = 3;
pub const RESET: u32 = 4;
pub const FOCUS: u32 = 5;
pub const RESIZE: u32 = 6;

// Copied application surfaces. IDs are opaque, process-owned, never reused.
pub const CREATE_WINDOW: u64 = 0x44;
pub const PRESENT_WINDOW: u64 = 0x45;
pub const READ_WINDOW_EVENT: u64 = 0x46;
pub const WAIT_WINDOW_EVENT: u64 = 0x47;
pub const CLOSE_WINDOW: u64 = 0x48;
pub const ENUM_WINDOWS: u64 = 0x49;
pub const COPY_WINDOW: u64 = 0x4a;
pub const SEND_WINDOW_EVENT: u64 = 0x4b;
pub const ACK_WINDOWS: u64 = 0x50;
pub const RESIZE_WINDOW: u64 = 0x51;
pub const QUERY_WINDOW: u64 = 0x52;
pub const MAX_WINDOWS: usize = 4;
pub const MAX_WINDOW_WIDTH: usize = 320;
pub const MAX_WINDOW_HEIGHT: usize = 240;
pub const MAX_WINDOW_PIXELS: usize = MAX_WINDOW_WIDTH * MAX_WINDOW_HEIGHT;

/// Enumeration is desktop-only and returns an empty slot with id == 0.
/// Capture epoch before scanning, then acknowledge that epoch after copying.
/// A newer epoch remains pending and wakes the desktop again.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct WindowInfo {
    pub id: u64,
    pub revision: u64,
    pub epoch: u64,
    pub width: u32,
    pub height: u32,
    pub title: [u8; 32],
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    pub version: u32,
    pub max_blit_pixels: u32,
}

/// KEY: evdev code, press/repeat/release value, optional decoded ASCII text.
/// POINTER: complete button bitmap, absolute x in value, absolute y in text;
/// display coordinates span 0..32767; routed app coordinates are content-local.
/// TEXT carries one serial byte in text. FOCUS value is 0 (lost) or 1 (gained).
/// RESET cancels held input state after a bounded event-queue overflow.
/// RESIZE value/text are the new content width/height; requery before uploading.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct DesktopEvent {
    pub kind: u32,
    pub code: u32,
    pub value: i32,
    pub text: u32,
}

const _: () = assert!(core::mem::size_of::<WindowInfo>() == 64);
const _: () = assert!(core::mem::size_of::<DisplayInfo>() == 16);
const _: () = assert!(core::mem::size_of::<DesktopEvent>() == 16);
