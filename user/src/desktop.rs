//! One EL0 desktop process: software composition, terminal and read-only browser.
use crate::shell::{self, Shell};
use cantaya_shared::{desktop::*, font};
use core::arch::asm;

const INK: u32 = 0xE7F0FA;
const MUTED: u32 = 0x91A8BE;
const ACCENT: u32 = 0x55DECD;
const PANEL: u32 = 0x14263A;
const TIMEOUT: u64 = 0x102;

fn syscall(number: u64, args: [u64; 6]) -> u64 {
    let result;
    unsafe {
        asm!("svc #0", inlateout("x0") args[0] => result,
        in("x1") args[1], in("x2") args[2], in("x3") args[3],
        in("x4") args[4], in("x5") args[5], in("x8") number);
    }
    result
}
fn allocate(size: usize) -> Option<u64> {
    let mut address = 0u64;
    (syscall(
        0x15,
        [
            u64::MAX,
            &mut address as *mut _ as u64,
            size as u64,
            0,
            0,
            0,
        ],
    ) == 0)
        .then_some(address)
}

struct Canvas {
    pixels: &'static mut [u32],
    previous: &'static mut [u32],
    width: i32,
    height: i32,
}
impl Canvas {
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        for py in y.max(0)..(y + h).min(self.height) {
            let start = (py * self.width + x.max(0).min(self.width)) as usize;
            let end = (py * self.width + (x + w).max(0).min(self.width)) as usize;
            if start < end {
                self.pixels[start..end].fill(color);
            }
        }
    }
    fn text(&mut self, text: &[u8], x: i32, y: i32, color: u32, scale: i32) {
        for (index, &byte) in text.iter().enumerate() {
            for (gy, bits) in font::glyph(byte).iter().enumerate() {
                for gx in 0..8 {
                    if bits & (0x80 >> gx) != 0 {
                        self.rect(
                            x + index as i32 * 8 * scale + gx * scale,
                            y + gy as i32 * scale,
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
        }
    }
    fn present(&mut self) -> Result<(), ()> {
        // Compose off-screen, then copy only changed bands. Each syscall copies
        // at most 16 KiB, so pointer updates never mask IRQs for an entire frame.
        let stride = self.width as usize;
        for y in (0..self.height as usize).step_by(4) {
            let rows = 4.min(self.height as usize - y);
            let mut left = stride;
            let mut right = 0;
            for row in y..y + rows {
                for x in 0..stride {
                    let index = row * stride + x;
                    if self.pixels[index] != self.previous[index] {
                        left = left.min(x);
                        right = right.max(x + 1);
                    }
                }
            }
            if left == stride {
                continue;
            }
            let source = self.pixels[y * stride + left..].as_ptr() as u64;
            if syscall(
                PRESENT,
                [
                    source,
                    left as u64,
                    y as u64,
                    (right - left) as u64,
                    rows as u64,
                    (stride * 4) as u64,
                ],
            ) != 0
            {
                return Err(());
            }
            for row in y..y + rows {
                let range = row * stride + left..row * stride + right;
                self.previous[range.clone()].copy_from_slice(&self.pixels[range]);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Terminal,
    Files,
    Application(usize),
}
#[derive(Clone, Copy)]
struct Window {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    visible: bool,
}
impl Window {
    fn contains(&self, x: i32, y: i32) -> bool {
        self.visible && x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

#[derive(Clone, Copy)]
enum Drag {
    Move(Focus, i32, i32),
    Resize(Focus, i32, i32, Window),
}
impl Drag {
    fn focus(self) -> Focus {
        match self {
            Self::Move(f, _, _) | Self::Resize(f, _, _, _) => f,
        }
    }
}

#[derive(Clone, Copy)]
struct Application {
    info: WindowInfo,
    window: Window,
}

/// Named launches never wait on the desktop thread. Scheduler references own
/// the running process after this temporary launch handle is closed.
fn launch_program(name: &str) -> bool {
    let mut handle = 0u64;
    if syscall(
        0x4c,
        [
            &mut handle as *mut _ as u64,
            2,
            0,
            name.as_ptr() as u64,
            name.len() as u64,
            0,
        ],
    ) != 0
    {
        return false;
    }
    shell::close_handle(handle);
    true
}

struct Terminal {
    cells: [u8; 96 * 40],
    wrapped: [bool; 40],
    cols: usize,
    rows: usize,
    col: usize,
    row: usize,
}
impl Terminal {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cells: [b' '; 96 * 40],
            wrapped: [false; 40],
            cols,
            rows,
            col: 0,
            row: 0,
        }
    }
    fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.clamp(1, 96);
        let rows = rows.clamp(1, 40);
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        let cells = self.cells;
        let wrapped = self.wrapped;
        let old_cols = self.cols;
        let old_row = self.row;
        let old_col = self.col;
        self.cols = cols;
        self.rows = rows;
        self.accept(12);
        for row in 0..=old_row {
            let text = &cells[row * old_cols..(row + 1) * old_cols];
            let length = if row == old_row {
                old_col
            } else if wrapped[row] {
                old_cols
            } else {
                text.iter().rposition(|b| *b != b' ').map_or(0, |i| i + 1)
            };
            for &byte in &text[..length] {
                self.accept(byte);
            }
            if row < old_row && !wrapped[row] && (self.col != 0 || length == 0) {
                self.accept(b'\n');
            }
        }
    }
    fn accept(&mut self, byte: u8) {
        match byte {
            12 => {
                self.cells.fill(b' ');
                self.wrapped.fill(false);
                self.row = 0;
                self.col = 0;
            }
            b'\r' => self.col = 0,
            b'\n' => {
                self.wrapped[self.row] = false;
                self.row += 1;
                self.col = 0;
            }
            8 => {
                if self.col != 0 {
                    self.col -= 1;
                } else if self.row != 0 {
                    self.row -= 1;
                    self.wrapped[self.row] = false;
                    self.col = self.cols - 1;
                }
            }
            b' '..=b'~' => {
                self.cells[self.row * self.cols + self.col] = byte;
                self.col += 1;
                if self.col == self.cols {
                    self.wrapped[self.row] = true;
                    self.col = 0;
                    self.row += 1;
                }
            }
            _ => {}
        }
        if self.row >= self.rows {
            self.cells.copy_within(self.cols..self.cols * self.rows, 0);
            self.cells[self.cols * (self.rows - 1)..self.cols * self.rows].fill(b' ');
            self.wrapped.copy_within(1..self.rows, 0);
            self.wrapped[self.rows - 1] = false;
            self.row = self.rows - 1;
        }
    }
}

struct Browser {
    entries: [[u8; 16]; 24],
    count: usize,
    selected: usize,
    directory: [u8; 12],
    directory_len: usize,
    preview: [u8; 1024],
    preview_len: usize,
}
impl Browser {
    fn new() -> Self {
        Self {
            entries: [[0; 16]; 24],
            count: 0,
            selected: 0,
            directory: [0; 12],
            directory_len: 0,
            preview: [0; 1024],
            preview_len: 0,
        }
    }
    fn directory(&self) -> Option<&str> {
        if self.directory_len == 0 {
            None
        } else {
            core::str::from_utf8(&self.directory[..self.directory_len]).ok()
        }
    }
    fn message(&mut self, text: &[u8]) {
        self.preview_len = text.len().min(self.preview.len());
        self.preview[..self.preview_len].copy_from_slice(&text[..self.preview_len]);
    }
    fn load(&mut self) {
        self.count = 0;
        self.selected = 0;
        for index in 0..self.entries.len() {
            let mut entry = [0; 16];
            match shell::directory_entry(self.directory(), index as u64, &mut entry) {
                0 => {
                    self.entries[index] = entry;
                    self.count += 1;
                }
                0x8000_0006 => break,
                _ => {
                    self.message(b"This folder is unavailable.\nUse Back to return to the root.");
                    return;
                }
            }
        }
        self.message(b"Select a text file to preview it.\nOpen folders with a click or Enter.\nFiles on the boot disk are read-only.");
    }
    fn back(&mut self) {
        self.directory_len = 0;
        self.load();
    }
    fn activate(&mut self) {
        if self.selected >= self.count {
            return;
        }
        let entry = self.entries[self.selected];
        let len = entry[..12].iter().position(|&b| b == 0).unwrap_or(12);
        if entry[12..16] == [255; 4] {
            if self.directory_len != 0 {
                self.message(b"Only one directory level is supported.");
                return;
            }
            self.directory[..len].copy_from_slice(&entry[..len]);
            self.directory_len = len;
            self.load();
            return;
        }
        let mut path = [0u8; 25];
        let mut offset = 0;
        if self.directory_len != 0 {
            path[..self.directory_len].copy_from_slice(&self.directory[..self.directory_len]);
            offset = self.directory_len;
            path[offset] = b'/';
            offset += 1;
        }
        path[offset..offset + len].copy_from_slice(&entry[..len]);
        let Ok(name) = core::str::from_utf8(&path[..offset + len]) else {
            return;
        };
        if name.ends_with(".ELF") {
            if launch_program(name) {
                self.message(b"Application started.\nIts window opens on the desktop.");
            } else {
                self.message(b"Cannot start this application.");
            }
            return;
        }
        if !name.ends_with(".TXT") {
            self.message(
                b"Preview is available for .TXT files.\nOpen ELF apps with a click or Enter.",
            );
            return;
        }
        let Ok(handle) = shell::open_file(name) else {
            self.message(b"Cannot open this file.");
            return;
        };
        self.preview_len = 0;
        while self.preview_len < self.preview.len() {
            let end = (self.preview_len + 128).min(self.preview.len());
            match shell::read_file(handle, &mut self.preview[self.preview_len..end]) {
                Ok(0) => break,
                Ok(count) => self.preview_len += count,
                Err(()) => {
                    self.message(b"File read failed.");
                    break;
                }
            }
        }
        shell::close_handle(handle);
        for byte in &mut self.preview[..self.preview_len] {
            if !matches!(*byte, b'\n' | b'\r' | b' '..=b'~') {
                *byte = b'?';
            }
        }
    }
}

pub(crate) struct Desktop {
    canvas: Canvas,
    terminal: Terminal,
    browser: Browser,
    terminal_window: Window,
    files_window: Window,
    focus: Focus,
    mouse_x: i32,
    mouse_y: i32,
    buttons: u32,
    drag: Option<Drag>,
    pending_resize: Option<(usize, i32, i32)>,
    applications: [Application; MAX_WINDOWS],
    application_pixels: &'static mut [u32],
    pointer_target: Option<usize>,
}
impl Desktop {
    // Keep the transcript/browser state off the 16 KiB guarded user stack.
    // The process owns this allocation for the lifetime of its desktop loop.
    pub(crate) fn start() -> Option<&'static mut Self> {
        let mut info = DisplayInfo::default();
        if syscall(QUERY_DISPLAY, [&mut info as *mut _ as u64, 0, 0, 0, 0, 0]) != 0
            || info.version != 1
            || info.width < 800
            || info.height < 600
        {
            return None;
        }
        let count = info.width as usize * info.height as usize;
        let pixels = allocate(count * 4)?;
        let Some(previous) = allocate(count * 4) else {
            syscall(0x1b, [u64::MAX, pixels, (count * 4) as u64, 0, 0, 0]);
            return None;
        };
        let Some(application_pixels) = allocate((MAX_WINDOWS + 1) * MAX_WINDOW_PIXELS * 4) else {
            syscall(0x1b, [u64::MAX, pixels, (count * 4) as u64, 0, 0, 0]);
            syscall(0x1b, [u64::MAX, previous, (count * 4) as u64, 0, 0, 0]);
            return None;
        };
        let canvas = Canvas {
            pixels: unsafe { core::slice::from_raw_parts_mut(pixels as *mut u32, count) },
            previous: unsafe { core::slice::from_raw_parts_mut(previous as *mut u32, count) },
            width: info.width as i32,
            height: info.height as i32,
        };
        let width = canvas.width;
        let height = canvas.height;
        let terminal_width = width * 56 / 100;
        let window_height = height - 218;
        let Some(state) = allocate(core::mem::size_of::<Self>()) else {
            syscall(0x1b, [u64::MAX, pixels, (count * 4) as u64, 0, 0, 0]);
            syscall(0x1b, [u64::MAX, previous, (count * 4) as u64, 0, 0, 0]);
            syscall(
                0x1b,
                [
                    u64::MAX,
                    application_pixels,
                    ((MAX_WINDOWS + 1) * MAX_WINDOW_PIXELS * 4) as u64,
                    0,
                    0,
                    0,
                ],
            );
            return None;
        };
        unsafe {
            (state as *mut Self).write(Self {
                canvas,
                terminal: Terminal::new(
                    ((terminal_width - 32) / 8) as usize,
                    ((window_height - 76) / 16) as usize,
                ),
                browser: Browser::new(),
                terminal_window: Window {
                    x: 24,
                    y: 142,
                    w: terminal_width,
                    h: window_height,
                    visible: true,
                },
                files_window: Window {
                    x: terminal_width + 44,
                    y: 142,
                    w: width - terminal_width - 68,
                    h: window_height,
                    visible: true,
                },
                focus: Focus::Terminal,
                mouse_x: width / 2,
                mouse_y: height / 2,
                buttons: 0,
                drag: None,
                pending_resize: None,
                applications: core::array::from_fn(|_| Application {
                    info: WindowInfo::default(),
                    window: Window {
                        x: 0,
                        y: 0,
                        w: 0,
                        h: 0,
                        visible: false,
                    },
                }),
                application_pixels: core::slice::from_raw_parts_mut(
                    application_pixels as *mut u32,
                    (MAX_WINDOWS + 1) * MAX_WINDOW_PIXELS,
                ),
                pointer_target: None,
            });
        }
        let desktop = unsafe { &mut *(state as *mut Self) };
        desktop.canvas.previous.fill(u32::MAX);
        desktop.browser.load();
        if desktop.render().is_err() {
            exit();
        }
        let _ = shell::output(format_args!("[desktop] EL0 desktop ready\n"));
        Some(desktop)
    }

    pub(crate) fn contract_probe(&mut self) -> bool {
        let mut event = DesktopEvent {
            kind: 0xFEED,
            ..DesktopEvent::default()
        };
        let address = self.canvas.pixels.as_ptr() as u64;
        let tests = [
            (QUERY_DISPLAY, [0, 0, 0, 0, 0, 0], 0xC000_0005),
            (PRESENT, [address, 0, 0, 0, 1, 4], 0xC000_000D),
            (
                PRESENT,
                [address, self.canvas.width as u64, 0, 1, 1, 4],
                0xC000_000D,
            ),
            (PRESENT, [address, 0, 0, 1024, 768, 4096], 0xC000_000D),
            (PRESENT, [address, 0, 0, 2, 1, 4], 0xC000_000D),
            (PRESENT, [u64::MAX, 0, 0, 1, 1, 4], 0xC000_000D),
            (PRESENT, [0, 0, 0, 1, 1, 4], 0xC000_0005),
            (READ_EVENT, [0, 0, 0, 0, 0, 0], 0xC000_0005),
            (READ_OUTPUT, [address, 1, 0, 0, 0, 0], 0xC000_0005),
            (READ_OUTPUT, [address, 1025, address, 0, 0, 0], 0xC000_000D),
            (READ_OUTPUT, [address, 1, address, 0, 0, 0], 0xC000_000D),
        ];
        if tests
            .iter()
            .any(|&(call, args, expected)| syscall(call, args) != expected)
        {
            return false;
        }
        let Some(page) = allocate(4096) else {
            return false;
        };
        unsafe {
            (page as *mut u32).add(1023).write(0xFF00FF);
        }
        let rejected = syscall(PRESENT, [page + 4092, 0, 0, 1, 2, 4]) == 0xC000_0005;
        syscall(0x1b, [u64::MAX, page, 4096, 0, 0, 0]);
        if !rejected {
            return false;
        }
        let Some(window_id) = self.window_contract_probe() else {
            return false;
        };
        let mut child = 0u64;
        if syscall(
            0x4c,
            [
                &mut child as *mut _ as u64,
                0,
                (window_id << 8) | 0x7f,
                0,
                0,
                0,
            ],
        ) != 0
        {
            return false;
        }
        let mut completion = 0i32;
        let waited = syscall(4, [child, 0, 0, &mut completion as *mut _ as u64, 0, 0]);
        shell::close_handle(child);
        if waited != 0 || completion != 0x44 {
            return false;
        }
        if syscall(CLOSE_WINDOW, [window_id, 0, 0, 0, 0, 0]) != 0 {
            return false;
        }
        // Close a blocked app, terminate another, and reset a third's session.
        for terminate in [false, true] {
            let Some((child, id)) = self.start_window_wait_probe() else {
                return false;
            };
            if !terminate {
                if syscall(RESIZE_WINDOW, [id, 32, 48, 0, 0, 0]) != 0 {
                    return false;
                }
                let timeout = 2u64;
                let mut completion = 0xFEEDi32;
                if syscall(
                    4,
                    [
                        child,
                        0,
                        &timeout as *const _ as u64,
                        &mut completion as *mut _ as u64,
                        0,
                        0,
                    ],
                ) != TIMEOUT
                    || completion != 0xFEED
                {
                    return false;
                }
                // Identical sizes produce no events and leave the app blocked.
                if syscall(RESIZE_WINDOW, [id, 32, 48, 0, 0, 0]) != 0
                    || syscall(
                        4,
                        [
                            child,
                            0,
                            &timeout as *const _ as u64,
                            &mut completion as *mut _ as u64,
                            0,
                            0,
                        ],
                    ) != TIMEOUT
                    || completion != 0xFEED
                {
                    return false;
                }
            }
            let status = if terminate {
                syscall(0x29, [child, 0x46, 0, 0, 0, 0])
            } else {
                syscall(CLOSE_WINDOW, [id, 0, 0, 0, 0, 0])
            };
            if status != 0 {
                return false;
            }
            let mut completion = 0i32;
            let waited = syscall(4, [child, 0, 0, &mut completion as *mut _ as u64, 0, 0]);
            shell::close_handle(child);
            if waited != 0 || completion != if terminate { 0x46 } else { 0x45 } {
                return false;
            }
            let mut info = WindowInfo::default();
            if syscall(ENUM_WINDOWS, [0, &mut info as *mut _ as u64, 0, 0, 0, 0]) != 0
                || info.id != 0
                || syscall(CLOSE_WINDOW, [id, 0, 0, 0, 0, 0]) != 0xC000_0008
            {
                return false;
            }
        }
        let Some((session_child, _)) = self.start_window_wait_probe() else {
            return false;
        };
        if syscall(0xf, [u64::MAX - 1, 0, 0, 0, 0, 0]) != 0
            || syscall(READ_EVENT, [&mut event as *mut _ as u64, 0, 0, 0, 0, 0]) != 0xC000_0022
        {
            return false;
        }
        let mut session_completion = 0i32;
        let waited = syscall(
            4,
            [
                session_child,
                0,
                0,
                &mut session_completion as *mut _ as u64,
                0,
                0,
            ],
        );
        shell::close_handle(session_child);
        if waited != 0 || session_completion != 0x45 {
            return false;
        }
        let mut byte = 0xFEu8;
        let mut count = 0xFEEDu64;
        if syscall(
            6,
            [
                u64::MAX - 1,
                &mut byte as *mut _ as u64,
                1,
                &mut count as *mut _ as u64,
                0,
                0,
            ],
        ) != TIMEOUT
            || byte != 0xFE
            || count != 0xFEED
            || syscall(PRESENT, [address, 0, 0, 1, 1, 4]) != 0
        {
            return false;
        }
        let _ = shell::output(format_args!("[desktop] graphics claim release validated\n"));
        let _ = shell::output(format_args!(
            "[desktop] window close, termination and session reset validated\n"
        ));
        let empty = syscall(READ_EVENT, [&mut event as *mut _ as u64, 0, 0, 0, 0, 0]);
        if empty != TIMEOUT || event.kind != 0xFEED {
            return false;
        }
        let _ = shell::output(format_args!(
            "[desktop] graphics and event contract validated\n"
        ));
        true
    }

    fn window_contract_probe(&mut self) -> Option<u64> {
        let mut id = 0xFEEDu64;
        let out = &mut id as *mut _ as u64;
        let name = b"Probe".as_ptr() as u64;
        for (args, expected) in [
            ([out, name, 5, 321, 64, 0], 0xC000_000D),
            ([out, name, 5, 64, 241, 0], 0xC000_000D),
            ([out, name, 32, 64, 64, 0], 0xC000_000D),
            ([out, name, 5, 0, 64, 0], 0xC000_000D),
            ([0, name, 5, 64, 64, 0], 0xC000_0005),
            ([out, 0, 5, 64, 64, 0], 0xC000_0005),
            ([out, b"bad\n".as_ptr() as u64, 4, 64, 64, 0], 0xC000_000D),
        ] {
            if syscall(CREATE_WINDOW, args) != expected || id != 0xFEED {
                return None;
            }
        }
        let mut info = WindowInfo::default();
        if syscall(ENUM_WINDOWS, [0, &mut info as *mut _ as u64, 0, 0, 0, 0]) != 0 || info.id != 0 {
            return None;
        }
        if syscall(CREATE_WINDOW, [out, name, 5, 64, 64, 0]) != 0 {
            return None;
        }
        let primary = id;
        let mut others = [0u64; 3];
        for other in &mut others {
            if syscall(CREATE_WINDOW, [other as *mut _ as u64, name, 5, 64, 64, 0]) != 0 {
                return None;
            }
        }
        id = 0xFEED;
        if syscall(CREATE_WINDOW, [out, name, 5, 64, 64, 0]) != 0xC000_0017 || id != 0xFEED {
            return None;
        }
        for other in others {
            if syscall(CLOSE_WINDOW, [other, 0, 0, 0, 0, 0]) != 0 {
                return None;
            }
        }
        let pixels = [0x123456u32, 0xFFEEDD];
        let source = pixels.as_ptr() as u64;
        for (args, expected) in [
            ([primary, source, 0, 0, 0, 0], 0xC000_000D),
            ([primary, source, 0, 4097, 0, 0], 0xC000_000D),
            ([primary, source, 4095, 2, 0, 0], 0xC000_000D),
            ([primary, source, u64::MAX, 2, 0, 0], 0xC000_000D),
            ([primary, 0, 0, 1, 0, 0], 0xC000_0005),
        ] {
            if syscall(PRESENT_WINDOW, args) != expected {
                return None;
            }
        }
        let page = allocate(4096)?;
        unsafe {
            (page as *mut u32).add(1023).write(0xFF00FF);
        }
        let rejected = syscall(PRESENT_WINDOW, [primary, page + 4092, 0, 2, 0, 0]) == 0xC000_0005;
        syscall(0x1b, [u64::MAX, page, 4096, 0, 0, 0]);
        if !rejected {
            return None;
        }
        let mut copied = [0xFEEDu32; 2];
        let output = copied.as_mut_ptr() as u64;
        if syscall(ENUM_WINDOWS, [0, &mut info as *mut _ as u64, 0, 0, 0, 0]) != 0
            || info.revision != 1
            || syscall(COPY_WINDOW, [primary, output, 0, 2, info.revision, 0]) != 0
            || copied != [0; 2]
        {
            return None;
        }
        if syscall(PRESENT_WINDOW, [primary, source, 0, 2, 0, 0]) != 0 {
            return None;
        }
        copied.fill(0xFEED);
        if syscall(COPY_WINDOW, [primary, output, 0, 2, info.revision, 0]) != 0xC000_022D
            || copied != [0xFEED; 2]
        {
            return None;
        }
        if syscall(ENUM_WINDOWS, [0, &mut info as *mut _ as u64, 0, 0, 0, 0]) != 0
            || syscall(COPY_WINDOW, [primary, 0, 0, 2, info.revision, 0]) != 0xC000_0005
            || syscall(COPY_WINDOW, [primary, output, 0, 2, info.revision, 0]) != 0
            || copied != pixels
        {
            return None;
        }
        let mut event = DesktopEvent {
            kind: 0xFEED,
            ..DesktopEvent::default()
        };
        let event_out = &mut event as *mut _ as u64;
        let empty = syscall(READ_WINDOW_EVENT, [primary, event_out, 0, 0, 0, 0]);
        let invalid = syscall(SEND_WINDOW_EVENT, [primary, event_out, 0, 0, 0, 0]);
        if empty != TIMEOUT || event.kind != 0xFEED || invalid != 0xC000_000D {
            return None;
        }
        let key = DesktopEvent {
            kind: KEY,
            code: 30,
            value: 1,
            text: 97,
        };
        let sent = syscall(
            SEND_WINDOW_EVENT,
            [primary, &key as *const _ as u64, 0, 0, 0, 0],
        );
        let invalid = syscall(READ_WINDOW_EVENT, [primary, 0, 0, 0, 0, 0]);
        let read = syscall(READ_WINDOW_EVENT, [primary, event_out, 0, 0, 0, 0]);
        if sent != 0 || invalid != 0xC000_0005 || read != 0 || event.text != 97 {
            return None;
        }
        for i in 0..40 {
            let key = DesktopEvent {
                text: 33 + i,
                ..key
            };
            if syscall(
                SEND_WINDOW_EVENT,
                [primary, &key as *const _ as u64, 0, 0, 0, 0],
            ) != 0
            {
                return None;
            }
        }
        if syscall(READ_WINDOW_EVENT, [primary, event_out, 0, 0, 0, 0]) != 0 || event.kind != RESET
        {
            return None;
        }
        let mut last = 0;
        while syscall(READ_WINDOW_EVENT, [primary, event_out, 0, 0, 0, 0]) == 0 {
            last = event.text;
        }
        if last != 72
            || !self.resize_contract_probe(primary)
            || syscall(CLOSE_WINDOW, [primary, 0, 0, 0, 0, 0]) != 0
        {
            return None;
        }
        for call in [
            PRESENT_WINDOW,
            READ_WINDOW_EVENT,
            WAIT_WINDOW_EVENT,
            CLOSE_WINDOW,
            QUERY_WINDOW,
        ] {
            if syscall(call, [primary, 0, 0, 0, 0, 0]) != 0xC000_0008 {
                return None;
            }
        }
        if syscall(CREATE_WINDOW, [out, name, 5, 64, 64, 0]) != 0 || id <= primary {
            return None;
        }
        let _ = shell::output(format_args!(
            "[desktop] bounded window and copied surface contract validated\n"
        ));
        Some(id)
    }
    fn resize_contract_probe(&self, id: u64) -> bool {
        let mut info = WindowInfo::default();
        let out = &mut info as *mut _ as u64;
        if syscall(QUERY_WINDOW, [id, out, 0, 0, 0, 0]) != 0
            || (info.width, info.height) != (64, 64)
            || syscall(QUERY_WINDOW, [id, 0, 0, 0, 0, 0]) != 0xC000_0005
        {
            return false;
        }
        let revision = info.revision;
        let epoch = info.epoch;
        let mut event = DesktopEvent {
            kind: KEY,
            code: 19,
            value: 1,
            text: 114,
        };
        let event_out = &mut event as *mut _ as u64;
        if syscall(SEND_WINDOW_EVENT, [id, event_out, 0, 0, 0, 0]) != 0 {
            return false;
        }
        for (w, h) in [
            (0, 64),
            (31, 64),
            (321, 64),
            (64, 31),
            (64, 241),
            (u64::MAX, 64),
        ] {
            if syscall(RESIZE_WINDOW, [id, w, h, 0, 0, 0]) != 0xC000_000D {
                return false;
            }
        }
        if syscall(RESIZE_WINDOW, [id, 64, 64, 0, 0, 0]) != 0
            || syscall(QUERY_WINDOW, [id, out, 0, 0, 0, 0]) != 0
            || info.revision != revision
            || info.epoch != epoch
            || syscall(READ_WINDOW_EVENT, [id, event_out, 0, 0, 0, 0]) != 0
            || (event.kind, event.text) != (KEY, 114)
            || syscall(READ_WINDOW_EVENT, [id, event_out, 0, 0, 0, 0]) != TIMEOUT
        {
            return false;
        }
        event = DesktopEvent {
            kind: POINTER,
            code: 1,
            value: 63,
            text: 63,
        };
        if syscall(SEND_WINDOW_EVENT, [id, event_out, 0, 0, 0, 0]) != 0 {
            return false;
        }
        // A row marker proves resize preserves rows rather than a linear prefix.
        let pixels = [0x123456u32, 0xFFEEDD];
        if syscall(
            PRESENT_WINDOW,
            [id, pixels.as_ptr() as u64, 64, 2, revision, 0],
        ) != 0
            || syscall(RESIZE_WINDOW, [id, 32, 48, 0, 0, 0]) != 0
            || syscall(QUERY_WINDOW, [id, out, 0, 0, 0, 0]) != 0
            || (info.width, info.height, info.revision) != (32, 48, revision + 2)
            || info.epoch <= epoch
        {
            return false;
        }
        let mut copied = [0xFEEDu32; 2];
        let rejected_pixels = [0xA5A5A5u32; 2];
        if syscall(
            PRESENT_WINDOW,
            [id, rejected_pixels.as_ptr() as u64, 0, 2, revision, 0],
        ) != 0xC000_022D
            || syscall(
                COPY_WINDOW,
                [id, copied.as_mut_ptr() as u64, 32, 2, info.revision, 0],
            ) != 0
            || copied != pixels
            || syscall(
                COPY_WINDOW,
                [id, copied.as_mut_ptr() as u64, 0, 2, info.revision, 0],
            ) != 0
            || copied != pixels
        {
            return false;
        }
        if syscall(READ_WINDOW_EVENT, [id, 0, 0, 0, 0, 0]) != 0xC000_0005
            || syscall(READ_WINDOW_EVENT, [id, event_out, 0, 0, 0, 0]) != 0
            || event.kind != RESET
            || syscall(READ_WINDOW_EVENT, [id, event_out, 0, 0, 0, 0]) != 0
            || (event.kind, event.value, event.text) != (RESIZE, 32, 48)
            || syscall(SEND_WINDOW_EVENT, [id, event_out, 0, 0, 0, 0]) != 0xC000_000D
            || syscall(RESIZE_WINDOW, [id, 32, 48, 0, 0, 0]) != 0
            || syscall(READ_WINDOW_EVENT, [id, event_out, 0, 0, 0, 0]) != TIMEOUT
        {
            return false;
        }
        if syscall(RESIZE_WINDOW, [id, 64, 64, 0, 0, 0]) != 0
            || syscall(QUERY_WINDOW, [id, out, 0, 0, 0, 0]) != 0
            || syscall(
                COPY_WINDOW,
                [id, copied.as_mut_ptr() as u64, 64, 2, info.revision, 0],
            ) != 0
            || copied != pixels
            || syscall(
                COPY_WINDOW,
                [id, copied.as_mut_ptr() as u64, 32, 2, info.revision, 0],
            ) != 0
            || copied != [0; 2]
            || syscall(
                PRESENT_WINDOW,
                [id, pixels.as_ptr() as u64, 0, 2, info.revision, 0],
            ) != 0
        {
            return false;
        }
        let _ = shell::output(format_args!(
            "[desktop] bounded resize and revision guard validated\n"
        ));
        true
    }
    fn start_window_wait_probe(&self) -> Option<(u64, u64)> {
        let mut child = 0u64;
        if syscall(0x4c, [&mut child as *mut _ as u64, 0, 0x78, 0, 0, 0]) != 0 {
            return None;
        }
        let timeout = 2u64;
        let mut completion = 0xFEEDi32;
        if syscall(
            4,
            [
                child,
                0,
                &timeout as *const _ as u64,
                &mut completion as *mut _ as u64,
                0,
                0,
            ],
        ) != TIMEOUT
            || completion != 0xFEED
        {
            return None;
        }
        let zero = 0u64;
        if syscall(
            4,
            [
                child,
                0,
                &zero as *const _ as u64,
                &mut completion as *mut _ as u64,
                0,
                0,
            ],
        ) != TIMEOUT
            || completion != 0xFEED
        {
            return None;
        }
        let mut info = WindowInfo::default();
        if syscall(ENUM_WINDOWS, [0, &mut info as *mut _ as u64, 0, 0, 0, 0]) != 0
            || info.id == 0
            || info.title[..4] != *b"Wait"
        {
            return None;
        }
        Some((child, info.id))
    }

    fn drain_output(&mut self) -> Result<bool, ()> {
        let mut changed = false;
        loop {
            let mut bytes = [0u8; 1024];
            let mut count = 0u64;
            match syscall(
                READ_OUTPUT,
                [
                    bytes.as_mut_ptr() as u64,
                    bytes.len() as u64,
                    &mut count as *mut _ as u64,
                    0,
                    0,
                    0,
                ],
            ) {
                0 if count <= bytes.len() as u64 => {
                    for &byte in &bytes[..count as usize] {
                        self.terminal.accept(byte);
                    }
                    changed = true;
                }
                TIMEOUT => return Ok(changed),
                _ => return Err(()),
            }
        }
    }

    pub(crate) fn reset_transcript(&mut self) {
        if self.drain_output().is_err() {
            exit();
        }
        self.terminal.accept(12);
    }

    pub(crate) fn run(&mut self, shell: &mut Shell) -> ! {
        loop {
            let mut dirty = false;
            // Bound each batch so a steady input stream still gets presented.
            for _ in 0..32 {
                let mut event = DesktopEvent::default();
                match syscall(READ_EVENT, [&mut event as *mut _ as u64, 0, 0, 0, 0, 0]) {
                    0 => {
                        if self.event(event, shell).is_err() {
                            exit();
                        }
                        dirty = true;
                    }
                    TIMEOUT => break,
                    _ => exit(),
                }
            }
            self.flush_resize();
            if shell.poll_programs().is_err() {
                exit();
            }
            match self.poll_windows() {
                Ok(changed) => dirty |= changed,
                Err(()) => exit(),
            }
            match self.drain_output() {
                Ok(changed) => dirty |= changed,
                Err(()) => exit(),
            }
            if dirty && self.render().is_err() {
                exit();
            }
            if shell::wait_for_input().is_err() {
                exit();
            }
        }
    }

    fn resize(&mut self, focus: Focus, original: Window, dx: i32, dy: i32) {
        let (min_w, min_h, max_w, max_h) = match focus {
            Focus::Terminal => (320, 220, 800, 716),
            Focus::Files => (288, 332, self.canvas.width, self.canvas.height),
            Focus::Application(_) => (
                176,
                170,
                MAX_WINDOW_WIDTH as i32 + 16,
                MAX_WINDOW_HEIGHT as i32 + 50,
            ),
        };
        let max_w = max_w.min(self.canvas.width - original.x - 8).max(min_w);
        let max_h = max_h.min(self.canvas.height - 64 - original.y).max(min_h);
        let width = (original.w + dx).clamp(min_w, max_w);
        let height = (original.h + dy).clamp(min_h, max_h);
        if let Focus::Application(index) = focus {
            self.pending_resize = Some((index, width - 16, height - 50));
            return;
        }
        let window = self.window(focus);
        window.w = width;
        window.h = height;
        if focus == Focus::Terminal {
            self.terminal
                .resize(((width - 32) / 8) as usize, ((height - 76) / 16) as usize);
        }
    }
    fn flush_resize(&mut self) {
        if let Some((index, width, height)) = self.pending_resize.take() {
            let id = self.applications[index].info.id;
            if id != 0 {
                syscall(RESIZE_WINDOW, [id, width as u64, height as u64, 0, 0, 0]);
            }
        }
    }
    fn minimize(&mut self, focus: Focus) {
        self.window(focus).visible = false;
        self.pointer_target = None;
        self.drag = None;
        self.focus(if focus == Focus::Terminal {
            Focus::Files
        } else {
            Focus::Terminal
        });
    }

    fn send(&self, index: usize, event: DesktopEvent) {
        let id = self.applications[index].info.id;
        if id != 0 {
            syscall(
                SEND_WINDOW_EVENT,
                [id, &event as *const _ as u64, 0, 0, 0, 0],
            );
        }
    }
    fn focus(&mut self, focus: Focus) {
        if self.focus != focus {
            if let Focus::Application(index) = self.focus {
                self.send(
                    index,
                    DesktopEvent {
                        kind: FOCUS,
                        value: 0,
                        ..DesktopEvent::default()
                    },
                );
            }
            self.pointer_target = None;
            self.drag = None;
            if let Focus::Application(index) = focus {
                self.send(
                    index,
                    DesktopEvent {
                        kind: FOCUS,
                        value: 1,
                        ..DesktopEvent::default()
                    },
                );
            }
        }
        self.focus = focus;
        self.window(focus).visible = true;
    }
    fn window(&mut self, focus: Focus) -> &mut Window {
        match focus {
            Focus::Terminal => &mut self.terminal_window,
            Focus::Files => &mut self.files_window,
            Focus::Application(index) => &mut self.applications[index].window,
        }
    }
    fn paint(&mut self) {
        if let Some(index) = self
            .applications
            .iter()
            .position(|app| app.info.id != 0 && app.info.title[..6] == *b"Paint\0")
        {
            self.focus(Focus::Application(index));
        } else if !launch_program("BIN/PAINT.ELF") {
            let _ = shell::output(format_args!("Cannot start Paint.\n"));
        }
    }
    fn cycle_focus(&mut self) {
        let next = match self.focus {
            Focus::Terminal => Focus::Files,
            Focus::Files => self
                .applications
                .iter()
                .position(|a| a.info.id != 0)
                .map_or(Focus::Terminal, Focus::Application),
            Focus::Application(index) => (index + 1..MAX_WINDOWS)
                .find(|i| self.applications[*i].info.id != 0)
                .map_or(Focus::Terminal, Focus::Application),
        };
        self.focus(next);
    }
    fn event(&mut self, event: DesktopEvent, shell: &mut Shell) -> Result<(), ()> {
        match event.kind {
            RESET => {
                if let Focus::Application(index) = self.focus {
                    self.send(index, event);
                }
                if let Some(index) = self.pointer_target {
                    self.send(index, event);
                }
                self.pointer_target = None;
                self.drag = None;
                self.pending_resize = None;
                self.buttons = 0;
            }
            TEXT => {
                self.focus(Focus::Terminal);
                shell.accept(event.text as u8)?;
            }
            KEY => {
                if event.value != 0 {
                    match event.code {
                        59 => self.focus(Focus::Terminal),
                        60 => self.focus(Focus::Files),
                        61 if event.value == 1 => self.paint(),
                        15 => self.cycle_focus(),
                        1 if self.focus == Focus::Files => self.browser.back(),
                        102 if self.focus == Focus::Files => self.browser.selected = 0,
                        103 if self.focus == Focus::Files => {
                            self.browser.selected = self.browser.selected.saturating_sub(1)
                        }
                        108 if self.focus == Focus::Files => {
                            self.browser.selected = (self.browser.selected + 1)
                                .min(self.browser.count.saturating_sub(1))
                        }
                        28 if self.focus == Focus::Files => self.browser.activate(),
                        _ if self.focus == Focus::Terminal && event.text != 0 => {
                            shell.accept(event.text as u8)?
                        }
                        _ => {}
                    }
                }
                if !matches!(event.code, 15 | 59 | 60 | 61) {
                    if let Focus::Application(index) = self.focus {
                        self.send(index, event);
                    }
                }
            }
            POINTER => {
                self.mouse_x = ((event.value.clamp(0, 32767) as i64
                    * (self.canvas.width - 1) as i64)
                    / 32767) as i32;
                self.mouse_y = ((event.text.min(32767) as i64 * (self.canvas.height - 1) as i64)
                    / 32767) as i32;
                let pressed = event.code & 1 != 0;
                if pressed && self.buttons & 1 == 0 {
                    self.click();
                }
                if pressed {
                    match self.drag {
                        Some(Drag::Move(focus, dx, dy)) => {
                            let x = (self.mouse_x - dx).clamp(0, self.canvas.width - 80);
                            let y = (self.mouse_y - dy).clamp(80, self.canvas.height - 96);
                            let w = self.window(focus);
                            w.x = x;
                            w.y = y;
                        }
                        Some(Drag::Resize(focus, x, y, original)) => {
                            self.resize(focus, original, self.mouse_x - x, self.mouse_y - y)
                        }
                        None => {}
                    }
                } else {
                    self.drag = None;
                }
                let target = self.pointer_target.or_else(|| {
                    if let Focus::Application(index) = self.focus {
                        let w = self.applications[index].window;
                        if self.mouse_x >= w.x + 8
                            && self.mouse_x < w.x + w.w - 8
                            && self.mouse_y >= w.y + 42
                            && self.mouse_y < w.y + w.h - 8
                            && self.drag.is_none()
                        {
                            return Some(index);
                        }
                    }
                    None
                });
                if let Some(index) = target {
                    let a = self.applications[index];
                    self.send(
                        index,
                        DesktopEvent {
                            kind: POINTER,
                            code: event.code & 7,
                            value: (self.mouse_x - a.window.x - 8)
                                .clamp(0, a.info.width as i32 - 1),
                            text: (self.mouse_y - a.window.y - 42)
                                .clamp(0, a.info.height as i32 - 1)
                                as u32,
                        },
                    );
                }
                if event.code == 0 {
                    self.pointer_target = None;
                }
                self.buttons = event.code;
            }
            _ => {}
        }
        Ok(())
    }
    fn click(&mut self) {
        let x = self.mouse_x;
        let y = self.mouse_y;
        if y >= self.canvas.height - 56 {
            if (24..172).contains(&x) {
                self.focus(Focus::Terminal);
            }
            if (184..332).contains(&x) {
                self.focus(Focus::Files);
            }
            if (344..492).contains(&x) {
                self.paint();
            }
            return;
        }
        let mut target = None;
        if self.window(self.focus).contains(x, y) {
            target = Some(self.focus);
        } else {
            for index in (0..MAX_WINDOWS).rev() {
                if self.applications[index].window.contains(x, y) {
                    target = Some(Focus::Application(index));
                    break;
                }
            }
            if target.is_none() && self.files_window.contains(x, y) {
                target = Some(Focus::Files);
            }
            if target.is_none() && self.terminal_window.contains(x, y) {
                target = Some(Focus::Terminal);
            }
        }
        let Some(target) = target else {
            return;
        };
        self.focus(target);
        let w = *self.window(target);
        if y < w.y + 34 {
            if x >= w.x + w.w - 34 {
                if let Focus::Application(index) = target {
                    syscall(
                        CLOSE_WINDOW,
                        [self.applications[index].info.id, 0, 0, 0, 0, 0],
                    );
                } else {
                    self.window(target).visible = false;
                }
                let next = if target == Focus::Terminal {
                    Focus::Files
                } else {
                    Focus::Terminal
                };
                self.focus(next);
            } else if x >= w.x + w.w - 68 {
                self.minimize(target);
            } else {
                self.drag = Some(Drag::Move(target, x - w.x, y - w.y));
            }
        } else if x >= w.x + w.w - 18 && y >= w.y + w.h - 18 {
            self.pointer_target = None;
            self.drag = Some(Drag::Resize(target, x, y, w));
        } else if target == Focus::Files {
            if y < w.y + 78 && x < w.x + 68 {
                self.browser.back();
            } else if y >= w.y + 98 && y < w.y + 98 + 24 * self.visible_files() as i32 {
                let visible = self.visible_files().min(self.browser.count);
                let start = self
                    .browser
                    .selected
                    .saturating_sub(visible.saturating_sub(1));
                let index = start + (y - w.y - 98) as usize / 24;
                if index < self.browser.count {
                    self.browser.selected = index;
                    self.browser.activate();
                }
            }
        } else if let Focus::Application(index) = target {
            if x >= w.x + 8 && x < w.x + w.w - 8 && y >= w.y + 42 && y < w.y + w.h - 8 {
                self.pointer_target = Some(index);
            }
        }
    }
    fn poll_windows(&mut self) -> Result<bool, ()> {
        let mut epoch = 0;
        let mut dirty = false;
        let mut complete = true;
        for index in 0..MAX_WINDOWS {
            let mut info = WindowInfo::default();
            if syscall(
                ENUM_WINDOWS,
                [index as u64, &mut info as *mut _ as u64, 0, 0, 0, 0],
            ) != 0
            {
                return Err(());
            }
            if index == 0 {
                epoch = info.epoch;
            }
            let old = self.applications[index].info;
            if info.id == 0 {
                if old.id != 0 {
                    self.applications[index].info = info;
                    self.applications[index].window.visible = false;
                    if self.focus == Focus::Application(index) {
                        self.focus(Focus::Terminal);
                    }
                    if self.pointer_target == Some(index) {
                        self.pointer_target = None;
                    }
                    if self
                        .drag
                        .is_some_and(|drag| drag.focus() == Focus::Application(index))
                    {
                        self.drag = None;
                    }
                    dirty = true;
                }
                continue;
            }
            if info.id == old.id && info.revision == old.revision {
                continue;
            }
            let count = info.width as usize * info.height as usize;
            if count > MAX_WINDOW_PIXELS || info.width == 0 || info.height == 0 {
                return Err(());
            }
            let scratch = MAX_WINDOWS * MAX_WINDOW_PIXELS;
            let mut copied = true;
            for offset in (0..count).step_by(MAX_BLIT_PIXELS) {
                let status = syscall(
                    COPY_WINDOW,
                    [
                        info.id,
                        self.application_pixels[scratch + offset..].as_mut_ptr() as u64,
                        offset as u64,
                        MAX_BLIT_PIXELS.min(count - offset) as u64,
                        info.revision,
                        0,
                    ],
                );
                if matches!(status, 0xC000_022D | 0xC000_0008) {
                    copied = false;
                    complete = false;
                    break;
                }
                if status != 0 {
                    return Err(());
                }
            }
            if !copied {
                continue;
            }
            self.application_pixels
                .copy_within(scratch..scratch + count, index * MAX_WINDOW_PIXELS);
            self.applications[index].info = info;
            if (info.width, info.height) != (old.width, old.height) {
                self.applications[index].window.w = info.width as i32 + 16;
                self.applications[index].window.h = info.height as i32 + 50;
            }
            if info.id != old.id {
                let w = info.width as i32 + 16;
                self.applications[index].window = Window {
                    x: (self.canvas.width - w) / 2 + index as i32 * 18,
                    y: 212 + index as i32 * 24,
                    w,
                    h: info.height as i32 + 50,
                    visible: true,
                };
                self.focus(Focus::Application(index));
            }
            dirty = true;
        }
        if complete && syscall(ACK_WINDOWS, [epoch, 0, 0, 0, 0, 0]) != 0 {
            return Err(());
        }
        Ok(dirty)
    }
    fn draw_application(&mut self, index: usize) {
        let app = self.applications[index];
        if app.info.id == 0 || !app.window.visible {
            return;
        }
        let length = app
            .info
            .title
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(32)
            .min(((app.window.w - 108) / 8).max(0) as usize);
        self.frame(
            app.window,
            &app.info.title[..length],
            self.focus == Focus::Application(index),
        );
        for y in 0..app.info.height as i32 {
            for x in 0..app.info.width as i32 {
                let color = self.application_pixels
                    [index * MAX_WINDOW_PIXELS + y as usize * app.info.width as usize + x as usize];
                self.canvas
                    .rect(app.window.x + 8 + x, app.window.y + 42 + y, 1, 1, color);
            }
        }
    }
    fn visible_files(&self) -> usize {
        ((self.files_window.h - 248) / 24).max(1) as usize
    }

    fn frame(&mut self, w: Window, title: &[u8], focused: bool) {
        let c = &mut self.canvas;
        c.rect(w.x + 5, w.y + 7, w.w, w.h, 0x050F1C);
        c.rect(
            w.x,
            w.y,
            w.w,
            w.h,
            if focused { 0x397084 } else { 0x294057 },
        );
        c.rect(w.x + 1, w.y + 1, w.w - 2, w.h - 2, PANEL);
        c.rect(
            w.x + 1,
            w.y + 1,
            w.w - 2,
            33,
            if focused { 0x23445A } else { 0x1D3248 },
        );
        c.rect(
            w.x + 13,
            w.y + 12,
            9,
            9,
            if focused { ACCENT } else { MUTED },
        );
        c.text(title, w.x + 34, w.y + 9, INK, 1);
        c.text(b"_", w.x + w.w - 58, w.y + 8, MUTED, 1);
        c.text(b"x", w.x + w.w - 24, w.y + 9, MUTED, 1);
    }
    fn draw_terminal(&mut self) {
        let w = self.terminal_window;
        if !w.visible {
            return;
        }
        self.frame(w, b"Terminal", self.focus == Focus::Terminal);
        let c = &mut self.canvas;
        c.rect(w.x + 1, w.y + 34, w.w - 2, w.h - 35, 0x0C1B2A);
        for row in 0..self.terminal.rows {
            let start = row * self.terminal.cols;
            c.text(
                &self.terminal.cells[start..start + self.terminal.cols],
                w.x + 16,
                w.y + 48 + row as i32 * 16,
                INK,
                1,
            );
        }
        if self.focus == Focus::Terminal {
            c.rect(
                w.x + 16 + self.terminal.col as i32 * 8,
                w.y + 61 + self.terminal.row as i32 * 16,
                8,
                2,
                ACCENT,
            );
        }
        c.text(
            b"F1 Terminal   F2 Files   Tab Switch",
            w.x + 16,
            w.y + w.h - 23,
            MUTED,
            1,
        );
    }
    fn draw_files(&mut self) {
        let w = self.files_window;
        if !w.visible {
            return;
        }
        self.frame(w, b"Files", self.focus == Focus::Files);
        let rows = self.visible_files().min(self.browser.count);
        // Keep keyboard selection visible when a directory exceeds the viewport.
        let start = self.browser.selected.saturating_sub(rows.saturating_sub(1));
        let c = &mut self.canvas;
        c.rect(w.x + 12, w.y + 46, 48, 28, 0x263E54);
        c.text(b"Back", w.x + 20, w.y + 52, INK, 1);
        c.text(b"/", w.x + 76, w.y + 52, ACCENT, 1);
        c.text(
            &self.browser.directory[..self.browser.directory_len],
            w.x + 88,
            w.y + 52,
            INK,
            1,
        );
        c.text(b"BOOT DISK / READ ONLY", w.x + 16, w.y + 80, MUTED, 1);
        for row in 0..rows {
            let index = start + row;
            let entry = self.browser.entries[index];
            let y = w.y + 98 + row as i32 * 24;
            if index == self.browser.selected {
                c.rect(w.x + 12, y, w.w - 24, 23, 0x24485B);
            }
            let directory = entry[12..16] == [255; 4];
            c.rect(
                w.x + 21,
                y + 7,
                12,
                12,
                if directory { 0xE8BF70 } else { 0x78AFDA },
            );
            let len = entry[..12].iter().position(|&b| b == 0).unwrap_or(12);
            c.text(&entry[..len], w.x + 44, y + 4, INK, 1);
            if directory {
                c.text(b"DIR", w.x + w.w - 48, y + 4, MUTED, 1);
            }
        }
        let preview_y = w.y + w.h - 144;
        c.rect(w.x + 12, preview_y, w.w - 24, 108, 0x0E2030);
        c.text(b"TEXT PREVIEW", w.x + 22, preview_y + 8, ACCENT, 1);
        let cols = ((w.w - 44) / 8).max(1) as usize;
        let mut col = 0;
        let mut row = 0;
        for &byte in &self.browser.preview[..self.browser.preview_len] {
            if byte == b'\r' {
                continue;
            }
            if byte == b'\n' {
                row += 1;
                col = 0;
            } else {
                if row >= 4 {
                    break;
                }
                c.text(
                    &[byte],
                    w.x + 22 + col as i32 * 8,
                    preview_y + 32 + row * 16,
                    INK,
                    1,
                );
                col += 1;
                if col == cols {
                    col = 0;
                    row += 1;
                }
            }
        }
        c.text(b"Enter Open   Esc Back", w.x + 16, w.y + w.h - 23, MUTED, 1);
    }
    fn grip(&mut self, focus: Focus) {
        let w = *self.window(focus);
        if !w.visible {
            return;
        }
        for offset in [4, 8, 12] {
            for step in 0..offset - 2 {
                self.canvas
                    .rect(w.x + w.w - 3 - step, w.y + w.h - offset + step, 1, 1, MUTED);
            }
        }
    }
    fn render(&mut self) -> Result<(), ()> {
        let c = &mut self.canvas;
        for y in 0..c.height {
            let shade = (y * 12 / c.height) as u32;
            c.rect(
                0,
                y,
                c.width,
                1,
                ((8 + shade / 2) << 16) | ((20 + shade) << 8) | (36 + shade * 2),
            );
            // A quiet geometric wallpaper, drawn with the same software canvas.
            let x = c.width - 260 + y / 3;
            c.rect(x, y, 2, 1, 0x254458);
            c.rect(x - 110, y, 1, 1, 0x1B354B);
        }
        c.text(b"CANTAYA", 24, 27, INK, 3);
        c.text(b"OS", 206, 27, ACCENT, 3);
        c.text(b"Your ARM64 workspace", 26, 88, MUTED, 1);
        c.text(b"DESKTOP / 0.1", c.width - 148, 30, ACCENT, 1);
        c.text(b"QEMU VIRT", c.width - 148, 53, MUTED, 1);
        if self.focus != Focus::Terminal {
            self.draw_terminal();
            self.grip(Focus::Terminal);
        }
        if self.focus != Focus::Files {
            self.draw_files();
            self.grip(Focus::Files);
        }
        for index in 0..MAX_WINDOWS {
            if self.focus != Focus::Application(index) {
                self.draw_application(index);
                self.grip(Focus::Application(index));
            }
        }
        match self.focus {
            Focus::Terminal => self.draw_terminal(),
            Focus::Files => self.draw_files(),
            Focus::Application(index) => self.draw_application(index),
        }
        self.grip(self.focus);
        let c = &mut self.canvas;
        let bar_y = c.height - 56;
        c.rect(0, bar_y, c.width, 56, 0x0D1D2E);
        c.rect(0, bar_y, c.width, 1, 0x345066);
        for (x, title, focus) in [
            (24, b"Terminal".as_slice(), Focus::Terminal),
            (184, b"Files".as_slice(), Focus::Files),
        ] {
            c.rect(
                x,
                bar_y + 10,
                148,
                36,
                if self.focus == focus {
                    0x284C60
                } else {
                    0x1B3046
                },
            );
            c.rect(x + 12, bar_y + 21, 10, 12, ACCENT);
            c.text(title, x + 34, bar_y + 20, INK, 1);
        }
        c.rect(
            344,
            bar_y + 10,
            148,
            36,
            if matches!(self.focus, Focus::Application(_)) {
                0x284C60
            } else {
                0x1B3046
            },
        );
        c.rect(356, bar_y + 21, 10, 12, ACCENT);
        c.text(b"Paint", 378, bar_y + 20, INK, 1);
        c.text(b"F1/F2/F3 | Tab", 516, bar_y + 20, MUTED, 1);
        let seconds = shell::system_value(1).unwrap_or(0) / 100;
        let mut clock = *b"UP 00:00";
        clock[3] = b'0' + ((seconds / 60 / 10) % 10) as u8;
        clock[4] = b'0' + ((seconds / 60) % 10) as u8;
        clock[6] = b'0' + ((seconds % 60) / 10) as u8;
        clock[7] = b'0' + (seconds % 10) as u8;
        c.text(&clock, c.width - 96, bar_y + 20, MUTED, 1);
        // A software cursor is composed last, so it always sits above windows.
        for y in 0..18 {
            for x in 0..=y / 2 {
                let edge = x == 0 || x == y / 2 || y == 17;
                c.rect(
                    self.mouse_x + x,
                    self.mouse_y + y,
                    1,
                    1,
                    if edge { 0x071422 } else { INK },
                );
            }
        }
        c.present()
    }
}
fn exit() -> ! {
    syscall(0x29, [u64::MAX, 7, 0, 0, 0, 0]);
    loop {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn el0_desktop_denied_probe(_stack: u64, mode: u64) -> ! {
    let rejected = [
        PRESENT,
        READ_EVENT,
        READ_OUTPUT,
        ENUM_WINDOWS,
        COPY_WINDOW,
        SEND_WINDOW_EVENT,
        ACK_WINDOWS,
        RESIZE_WINDOW,
    ]
    .iter()
    .all(|&call| syscall(call, [0; 6]) == 0xC000_0022)
        && [
            PRESENT_WINDOW,
            READ_WINDOW_EVENT,
            WAIT_WINDOW_EVENT,
            CLOSE_WINDOW,
            QUERY_WINDOW,
        ]
        .iter()
        .all(|&call| syscall(call, [mode >> 8, 0, 0, 0, 0, 0]) == 0xC000_0022)
        && syscall(0xf, [u64::MAX - 1, 0, 0, 0, 0, 0]) == 0xC000_0008;
    if rejected {
        let _ = shell::output(format_args!("[desktop] isolated process access denied\n"));
    }
    syscall(
        0x29,
        [u64::MAX, if rejected { 0x44 } else { 8 }, 0, 0, 0, 0],
    );
    loop {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn el0_window_wait_probe(_stack: u64, _mode: u64) -> ! {
    let mut id = 0u64;
    let mut good = syscall(
        CREATE_WINDOW,
        [
            &mut id as *mut _ as u64,
            b"Wait".as_ptr() as u64,
            4,
            64,
            64,
            0,
        ],
    ) == 0;
    if good {
        let status = syscall(WAIT_WINDOW_EVENT, [id, 0, 0, 0, 0, 0]);
        if status == 0 {
            let mut event = DesktopEvent::default();
            let out = &mut event as *mut _ as u64;
            let mut info = WindowInfo::default();
            good = syscall(READ_WINDOW_EVENT, [id, out, 0, 0, 0, 0]) == 0
                && event.kind == RESET
                && syscall(READ_WINDOW_EVENT, [id, out, 0, 0, 0, 0]) == 0
                && (event.kind, event.value, event.text) == (RESIZE, 32, 48)
                && syscall(QUERY_WINDOW, [id, &mut info as *mut _ as u64, 0, 0, 0, 0]) == 0
                && (info.width, info.height) == (32, 48);
            if good {
                let _ = shell::output(format_args!("[desktop] blocked window resize delivered\n"));
                good = syscall(WAIT_WINDOW_EVENT, [id, 0, 0, 0, 0, 0]) == 0xC000_0008;
            }
        } else {
            good = status == 0xC000_0008;
        }
    }
    if good {
        let _ = shell::output(format_args!("[desktop] blocked window wait cancelled\n"));
    }
    syscall(0x29, [u64::MAX, if good { 0x45 } else { 9 }, 0, 0, 0, 0]);
    loop {
        core::hint::spin_loop();
    }
}
