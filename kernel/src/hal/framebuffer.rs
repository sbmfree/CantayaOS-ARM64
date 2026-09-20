//! Linear framebuffer console.
//!
//! Provides a simple pixel-level writer and an 8×16 bitmap font console
//! (using a built-in PSF2-style glyph table) so boot messages appear on the
//! QEMU virtual screen.

use cantaya_shared::{FramebufferInfo, PixelFormat};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Console state
// ─────────────────────────────────────────────────────────────────────────────

struct Console {
    fb: FramebufferInfo,
    base: usize,
    col: u32,
    row: u32,
    cols: u32,
    rows: u32,
    fg: u32,
    bg: u32,
}

const CHAR_W: u32 = 8;
const CHAR_H: u32 = 16;

impl Console {
    fn new(fb: FramebufferInfo) -> Self {
        let cols = fb.width / CHAR_W;
        let rows = fb.height / CHAR_H;
        Console {
            fb,
            base: framebuffer_base(&fb),
            col: 0,
            row: 0,
            cols,
            rows,
            fg: 0x00FF_FF00,
            bg: 0x0000_0010,
        }
    }

    fn put_pixel(&self, x: u32, y: u32, color: u32) {
        let offset = (y * self.fb.stride + x * (self.fb.bpp / 8)) as usize;
        let ptr = (self.base + offset) as *mut u32;
        let pixel = match self.fb.pixel_format {
            PixelFormat::Rgb | PixelFormat::Bgr => color,
            _ => color,
        };
        unsafe { core::ptr::write_volatile(ptr, pixel) };
    }

    fn draw_glyph(&self, ch: u8, col: u32, row: u32) {
        let glyph = crate::hal::font::glyph(ch);
        let px = col * CHAR_W;
        let py = row * CHAR_H;
        for (row_idx, &row_bits) in glyph.iter().enumerate() {
            for bit in 0..8u32 {
                let color = if row_bits & (0x80 >> bit) != 0 {
                    self.fg
                } else {
                    self.bg
                };
                self.put_pixel(px + bit, py + row_idx as u32, color);
            }
        }
    }

    fn scroll(&self) {
        // Scroll up by one character row (copy framebuffer rows upward)
        let row_bytes = (self.fb.stride * CHAR_H) as usize;
        let total = (self.fb.stride * self.fb.height) as usize;
        let src = (self.base + row_bytes) as *const u8;
        let dst = self.base as *mut u8;
        unsafe {
            core::ptr::copy(src, dst, total - row_bytes);
            // Clear the last row
            core::ptr::write_bytes((dst as usize + total - row_bytes) as *mut u8, 0, row_bytes);
        }
    }

    fn write_char(&mut self, c: char) {
        match c {
            '\n' => {
                self.col = 0;
                self.row += 1;
            }
            '\r' => {
                self.col = 0;
            }
            _ => {
                self.draw_glyph(c as u8, self.col, self.row);
                self.col += 1;
                if self.col >= self.cols {
                    self.col = 0;
                    self.row += 1;
                }
            }
        }
        if self.row >= self.rows {
            self.scroll();
            self.row = self.rows - 1;
        }
    }
}

impl core::fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            self.write_char(c);
        }
        Ok(())
    }
}

static CONSOLE: Mutex<Option<Console>> = Mutex::new(None);

fn framebuffer_base(fb: &FramebufferInfo) -> usize {
    crate::arch::mmu::phys_to_direct_map(fb.base) as usize
}

/// Initialise the framebuffer console and draw the CantayaTech splash header.
///
/// Returns `false` when the bootloader did not provide a usable linear
/// framebuffer (for example, when QEMU is running headless).
pub fn init(fb: &FramebufferInfo) -> bool {
    // The console writes 32-bit pixels.  Do not create a console from the
    // zeroed descriptor used by the bootloader when GOP is unavailable.
    if fb.base == 0
        || fb.width < CHAR_W
        || fb.height < CHAR_H
        || fb.bpp != 32
        || fb.stride < fb.width * (fb.bpp / 8)
    {
        return false;
    }

    use core::fmt::Write;
    let mut c = Console::new(*fb);
    // Clear screen
    let total = (fb.stride * fb.height) as usize;
    unsafe { core::ptr::write_bytes(framebuffer_base(fb) as *mut u8, 0, total) };
    let _ = writeln!(
        c,
        "  CantayaOS v{}  (CantayaTech)",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(c, "  ARM64 \u{b7} UEFI \u{b7} NT-Like Kernel");
    let _ = writeln!(c, "  \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");
    *CONSOLE.lock() = Some(c);
    true
}

/// Write a formatted string to the framebuffer console (used by logger).
pub fn write_fmt(args: core::fmt::Arguments) {
    use core::fmt::Write;
    if let Some(ref mut c) = *CONSOLE.lock() {
        let _ = c.write_fmt(args);
    }
}
