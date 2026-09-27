//! GOP framebuffer initialisation.

use uefi::proto::console::gop::{GraphicsOutput, PixelFormat as GopPixFmt};

use cantaya_shared::{FramebufferInfo, PixelFormat};

/// Locate the GOP protocol and return a `FramebufferInfo` describing the
/// current video mode's linear framebuffer.
pub fn init() -> Option<FramebufferInfo> {
    let gop_handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>().ok()?;
    let mut gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle).ok()?;

    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    // UEFI reports pixels per scanline; the handoff ABI uses bytes so that
    // the kernel can use it directly for byte-addressed framebuffer writes.
    let stride = mode_info.stride() as u32 * 4;

    let pixel_format = match mode_info.pixel_format() {
        GopPixFmt::Rgb => PixelFormat::Rgb,
        GopPixFmt::Bgr => PixelFormat::Bgr,
        GopPixFmt::Bitmask => PixelFormat::Bitmask,
        // BltOnly means no linear framebuffer — skip
        GopPixFmt::BltOnly => return None,
    };

    let base = gop.frame_buffer().as_mut_ptr() as u64;

    Some(FramebufferInfo {
        base,
        width: width as u32,
        height: height as u32,
        stride,
        bpp: 32,
        pixel_format,
    })
}
