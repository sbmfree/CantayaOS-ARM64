#![no_std]
#![no_main]

use cantaya_shared::{desktop::*, font};
use core::arch::{asm, global_asm};
global_asm!(".section .text.start,\"ax\"\n.global _start\n_start:\nb paint_main");
global_asm!(include_str!("../../user/src/simd_probe.s"));
extern "C" {
    fn el0_simd_probe(seed: u64) -> u64;
}
const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const ART_WIDTH: usize = WIDTH - 16;
const ART_HEIGHT: usize = HEIGHT - 78;
const INK: u32 = 0xE7F0FA;
const COLORS: [u32; 4] = [0x193B55, 0xD95F76, 0x168A7A, 0xD79A35];
const RETRY: u64 = 0xC000_022D;

fn syscall(number: u64, args: [u64; 6]) -> u64 {
    let result;
    unsafe {
        asm!("svc #0", inlateout("x0") args[0] => result,
        in("x1") args[1], in("x2") args[2], in("x3") args[3],
        in("x4") args[4], in("x5") args[5], in("x8") number);
    }
    result
}
fn exit(status: u64) -> ! {
    syscall(0x29, [u64::MAX, status, 0, 0, 0, 0]);
    loop {
        core::hint::spin_loop();
    }
}
fn query(id: u64) -> WindowInfo {
    let mut info = WindowInfo::default();
    if syscall(QUERY_WINDOW, [id, &mut info as *mut _ as u64, 0, 0, 0, 0]) != 0 {
        exit(0);
    }
    info
}
struct Surface<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
}
impl Surface<'_> {
    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        for row in y..(y + h).min(self.height) {
            self.pixels[row * self.width + x..row * self.width + (x + w).min(self.width)]
                .fill(color);
        }
    }
    fn text(&mut self, value: &[u8], x: usize, y: usize) {
        for (index, &byte) in value.iter().enumerate() {
            for (row, bits) in font::glyph(byte).iter().enumerate() {
                for col in 0..8 {
                    if bits & (0x80 >> col) != 0 {
                        self.rect(x + index * 8 + col, y + row, 1, 1, INK);
                    }
                }
            }
        }
    }
    fn compose(&mut self, art: &[u32], selected: usize) {
        self.pixels[..self.width * self.height].fill(0x14263A);
        self.rect(0, 0, self.width, 44, 0x1B3046);
        if self.width >= 300 {
            self.text(b"PAINT", 12, 15);
        }
        for (i, color) in COLORS.iter().enumerate() {
            let x = palette_start(self.width) + i * 36;
            self.rect(x - 2, 8, 32, 28, if i == selected { INK } else { 0x1B3046 });
            self.rect(x, 10, 28, 24, *color);
        }
        if self.width >= 208 {
            self.text(b"CLR", self.width - 44, 15);
        }
        // Keep the full drawing privately, even when only a smaller part is visible.
        let visible_width = self.width.saturating_sub(16).min(ART_WIDTH);
        let visible_height = self.height.saturating_sub(78).min(ART_HEIGHT);
        for row in 0..visible_height {
            let target = (row + 54) * self.width + 8;
            self.pixels[target..target + visible_width]
                .copy_from_slice(&art[row * ART_WIDTH..row * ART_WIDTH + visible_width]);
        }
        let footer: &[u8] = if self.width >= 240 {
            b"1-4 color  C clear  Esc quit"
        } else {
            b"1-4 C Esc"
        };
        self.text(footer, 12, self.height.saturating_sub(18));
    }
}
fn palette_start(width: usize) -> usize {
    if width >= 300 {
        104
    } else {
        12
    }
}
/// Return false when the desktop changed geometry during this upload.
fn upload(id: u64, surface: &Surface<'_>, revision: &mut u64) -> bool {
    let end = surface.width * surface.height;
    for offset in (0..end).step_by(MAX_BLIT_PIXELS) {
        let count = MAX_BLIT_PIXELS.min(end - offset);
        match syscall(
            PRESENT_WINDOW,
            [
                id,
                surface.pixels[offset..].as_ptr() as u64,
                offset as u64,
                count as u64,
                *revision,
                0,
            ],
        ) {
            0 => *revision += 1,
            RETRY => return false,
            _ => exit(0),
        }
    }
    true
}
fn point(art: &mut [u32], x: i32, y: i32, width: usize, height: usize, color: u32) {
    for py in (y - 2).max(54)..=(y + 2).min(height as i32 - 25) {
        for px in (x - 2).max(8)..=(x + 2).min(width as i32 - 9) {
            art[(py as usize - 54) * ART_WIDTH + px as usize - 8] = color;
        }
    }
}
fn line(
    art: &mut [u32],
    from: (i32, i32),
    to: (i32, i32),
    width: usize,
    height: usize,
    color: u32,
) {
    let (mut x, mut y) = from;
    let dx = (to.0 - x).abs();
    let dy = -(to.1 - y).abs();
    let sx = if x < to.0 { 1 } else { -1 };
    let sy = if y < to.1 { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        point(art, x, y, width, height, color);
        if (x, y) == to {
            break;
        }
        let twice = error * 2;
        if twice >= dy {
            error += dy;
            x += sx;
        }
        if twice <= dx {
            error += dx;
            y += sy;
        }
    }
}
#[no_mangle]
extern "C" fn paint_main() -> ! {
    if unsafe { el0_simd_probe(0xa5) } != 0 {
        exit(4);
    }
    let length = MAX_WINDOW_PIXELS + ART_WIDTH * ART_HEIGHT;
    let mut address = 0u64;
    if syscall(
        0x15,
        [
            u64::MAX,
            &mut address as *mut _ as u64,
            (length * 4) as u64,
            0,
            0,
            0,
        ],
    ) != 0
    {
        exit(1);
    }
    let buffer = unsafe { core::slice::from_raw_parts_mut(address as *mut u32, length) };
    let (pixels, art) = buffer.split_at_mut(MAX_WINDOW_PIXELS);
    art.fill(INK);
    let mut id = 0;
    if syscall(
        CREATE_WINDOW,
        [
            &mut id as *mut _ as u64,
            b"Paint".as_ptr() as u64,
            5,
            WIDTH as u64,
            HEIGHT as u64,
            0,
        ],
    ) != 0
    {
        exit(2);
    }
    let mut surface = Surface {
        pixels,
        width: WIDTH,
        height: HEIGHT,
    };
    let mut selected = 0;
    let mut buttons = 0;
    let mut previous = None;
    let mut dirty = true;
    let mut announced = false;
    loop {
        // Query again after each resize race. A revision guard keeps old strides
        // from being uploaded into a surface whose geometry has already changed.
        let info = query(id);
        if surface.width != info.width as usize || surface.height != info.height as usize {
            surface.width = info.width as usize;
            surface.height = info.height as usize;
            previous = None;
            buttons = 0;
            dirty = true;
        }
        if dirty {
            surface.compose(art, selected);
            let mut revision = info.revision;
            if !upload(id, &surface, &mut revision) {
                continue;
            }
            dirty = false;
            if !announced {
                let message = b"Paint ready.\n";
                syscall(
                    0x8,
                    [
                        u64::MAX,
                        message.as_ptr() as u64,
                        message.len() as u64,
                        0,
                        0,
                        0,
                    ],
                );
                announced = true;
            }
        }
        for _ in 0..32 {
            let mut event = DesktopEvent::default();
            match syscall(
                READ_WINDOW_EVENT,
                [id, &mut event as *mut _ as u64, 0, 0, 0, 0],
            ) {
                0 => {}
                0x102 => break,
                _ => exit(0),
            }
            let mut clear = false;
            let mut color = None;
            match event.kind {
                KEY if event.value != 0 => match event.text as u8 {
                    b'1'..=b'4' => color = Some((event.text as u8 - b'1') as usize),
                    b'c' | b'C' => clear = true,
                    _ if event.code == 1 => {
                        syscall(CLOSE_WINDOW, [id, 0, 0, 0, 0, 0]);
                        exit(0);
                    }
                    _ => {}
                },
                POINTER => {
                    let x = event.value;
                    let y = event.text as i32;
                    let down = event.code & 1 != 0;
                    let palette = palette_start(surface.width) as i32;
                    if down && buttons & 1 == 0 && (10..34).contains(&y) {
                        if (palette..palette + 136).contains(&x) {
                            color = Some(((x - palette) / 36) as usize);
                        }
                        if surface.width >= 208
                            && (surface.width as i32 - 50..surface.width as i32 - 4).contains(&x)
                        {
                            clear = true;
                        }
                    }
                    if down
                        && (8..surface.width as i32 - 8).contains(&x)
                        && (54..surface.height as i32 - 24).contains(&y)
                    {
                        let from = previous.unwrap_or((x, y));
                        line(
                            art,
                            from,
                            (x, y),
                            surface.width,
                            surface.height,
                            COLORS[selected],
                        );
                        previous = Some((x, y));
                        dirty = true;
                    } else {
                        previous = None;
                    }
                    buttons = event.code;
                }
                RESIZE => {
                    previous = None;
                    buttons = 0;
                    dirty = true;
                    // Do not interpret subsequent coordinates using the old geometry.
                    break;
                }
                RESET | FOCUS => {
                    previous = None;
                    buttons = 0;
                }
                _ => {}
            }
            if let Some(color) = color.filter(|c| *c < COLORS.len()) {
                selected = color;
                dirty = true;
            }
            if clear {
                art.fill(INK);
                previous = None;
                dirty = true;
            }
        }
        if dirty {
            continue;
        }
        if syscall(WAIT_WINDOW_EVENT, [id, 0, 0, 0, 0, 0]) != 0 {
            exit(0);
        }
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(3);
}
