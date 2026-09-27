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
    scroll_top: u32,
    scroll_bottom: u32,
    left_margin: u32,
    right_margin: u32,
}

const CHAR_W: u32 = 8;
const CHAR_H: u32 = 16;
const PANEL_BG: u32 = 0x0010_2533;
const ACCENT: u32 = 0x003A_D6_C5;
const TITLE: u32 = 0x00E8_F7_F8;
const MUTED: u32 = 0x008B_AAB8;

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
            scroll_top: 0,
            scroll_bottom: rows,
            left_margin: 0,
            right_margin: cols,
        }
    }

    fn fill_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32) {
        let x_end = x.saturating_add(width).min(self.fb.width);
        let y_end = y.saturating_add(height).min(self.fb.height);
        for py in y..y_end {
            for px in x..x_end {
                self.put_pixel(px, py, color);
            }
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

    fn draw_scaled_text(&self, text: &str, x: u32, y: u32, scale: u32, color: u32) {
        for (index, ch) in text.bytes().enumerate() {
            let glyph = crate::hal::font::glyph(ch);
            let x_origin = x + index as u32 * CHAR_W * scale;
            for (gy, bits) in glyph.iter().enumerate() {
                for gx in 0..CHAR_W {
                    if bits & (0x80 >> gx) != 0 {
                        self.fill_rect(
                            x_origin + gx * scale,
                            y + gy as u32 * scale,
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
        }
    }

    fn show_shell_screen(&mut self) {
        unsafe {
            core::ptr::write_bytes(
                self.base as *mut u8,
                0,
                (self.fb.stride * self.fb.height) as usize,
            )
        };

        if self.cols < 50 || self.rows < 18 {
            self.scroll_top = 0;
            self.scroll_bottom = self.rows;
            self.left_margin = 0;
            self.right_margin = self.cols;
            self.col = 0;
            self.row = 0;
            return;
        }

        let panel_row = self.rows * 2 / 3;
        let panel_y = panel_row * CHAR_H;
        let logo = "CANTAYA OS";
        let logo_scale = if self.fb.width >= 960 { 6 } else { 4 };
        let logo_width = logo.len() as u32 * CHAR_W * logo_scale;
        let logo_x = (self.fb.width - logo_width) / 2;
        let logo_y = (panel_y - CHAR_H * logo_scale) / 2;
        self.draw_scaled_text(logo, logo_x, logo_y, logo_scale, TITLE);
        self.fill_rect(
            logo_x,
            logo_y + CHAR_H * logo_scale + 9,
            logo_width,
            3,
            ACCENT,
        );

        let version = env!("CARGO_PKG_VERSION");
        let version_scale = 2;
        let label = "VERSION ";
        let version_width = (label.len() + version.len()) as u32 * CHAR_W * version_scale;
        let version_x = (self.fb.width - version_width) / 2;
        let version_y = logo_y + CHAR_H * logo_scale + 28;
        self.draw_scaled_text(label, version_x, version_y, version_scale, MUTED);
        self.draw_scaled_text(
            version,
            version_x + label.len() as u32 * CHAR_W * version_scale,
            version_y,
            version_scale,
            MUTED,
        );

        self.fill_rect(
            0,
            panel_y,
            self.fb.width,
            self.fb.height - panel_y,
            PANEL_BG,
        );
        self.fill_rect(0, panel_y, self.fb.width, 3, ACCENT);
        self.draw_scaled_text("TERMINAL", 3 * CHAR_W, panel_y + 12, 1, ACCENT);
        let hint = "KEYBOARD + SERIAL";
        self.draw_scaled_text(
            hint,
            self.fb.width - (hint.len() as u32 + 3) * CHAR_W,
            panel_y + 12,
            1,
            MUTED,
        );

        self.fg = TITLE;
        self.bg = PANEL_BG;
        self.scroll_top = panel_row + 3;
        self.scroll_bottom = self.rows - 1;
        self.left_margin = 3;
        self.right_margin = self.cols - 3;
        self.col = self.left_margin;
        self.row = self.scroll_top;
    }

    fn scroll(&self) {
        // Keep the logo and terminal header fixed while output scrolls below.
        let row_bytes = (self.fb.stride * CHAR_H) as usize;
        let src = (self.base + (self.scroll_top as usize + 1) * row_bytes) as *const u8;
        let dst = (self.base + self.scroll_top as usize * row_bytes) as *mut u8;
        let copy_bytes = (self.scroll_bottom - self.scroll_top - 1) as usize * row_bytes;
        unsafe {
            core::ptr::copy(src, dst, copy_bytes);
        }
        self.fill_rect(
            0,
            (self.scroll_bottom - 1) * CHAR_H,
            self.fb.width,
            CHAR_H,
            self.bg,
        );
    }

    fn write_char(&mut self, c: char) {
        match c {
            '\n' => {
                self.col = self.left_margin;
                self.row += 1;
            }
            '\r' => {
                self.col = self.left_margin;
            }
            '\x08' => {
                if self.col > self.left_margin {
                    self.col -= 1;
                } else if self.row > self.scroll_top {
                    self.row -= 1;
                    self.col = self.right_margin - 1;
                }
            }
            '\x07' => {}
            _ => {
                self.draw_glyph(c as u8, self.col, self.row);
                self.col += 1;
                if self.col >= self.right_margin {
                    self.col = self.left_margin;
                    self.row += 1;
                }
            }
        }
        if self.row >= self.scroll_bottom {
            self.scroll();
            self.row = self.scroll_bottom - 1;
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
    with_console(|c| {
        let _ = c.write_fmt(args);
    });
}

/// Replace boot logs with the post-validation logo and lower terminal pane.
pub fn show_shell_screen() {
    with_console(Console::show_shell_screen);
}

fn with_console(f: impl FnOnce(&mut Console)) {
    let daif_saved: u64;
    unsafe {
        core::arch::asm!(
            "mrs {0}, DAIF",
            "msr DAIFSet, #0xf",
            out(reg) daif_saved,
        );
    }
    if let Some(ref mut console) = *CONSOLE.lock() {
        f(console);
    }
    unsafe { core::arch::asm!("msr DAIF, {0}", in(reg) daif_saved) };
}
