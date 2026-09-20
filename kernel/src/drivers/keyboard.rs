//! PL050 KMI (Keyboard/Mouse Interface) driver.
//!
//! QEMU `virt` exposes PL050 at:
//!   Keyboard: 0x0906_0000   IRQ 33
//!   Mouse:    0x0907_0000   IRQ 34
//!
//! PS/2 scan codes (set 2) are decoded into ASCII for basic input.

use alloc::collections::VecDeque;
use spin::Mutex;

const KMI_BASE: usize = 0x0906_0000;
const KMI_IRQ: usize = 33;

// PL050 register offsets
const KMICR: usize = 0x00; // control
const KMISTAT: usize = 0x04; // status
const KMIDATA: usize = 0x08; // data
const KMICLKDIV: usize = 0x0C; // clock divisor
const KMIIR: usize = 0x10; // interrupt status/clear

const KMICR_EN: u32 = 1 << 2; // enable KMI
const KMICR_RXINTREN: u32 = 1 << 4; // enable RX interrupt
const KMISTAT_RXFULL: u32 = 1 << 4; // RX data available

#[inline]
fn kmi_base() -> usize {
    crate::arch::mmu::phys_to_direct_map(KMI_BASE as u64) as usize
}

#[inline]
unsafe fn kmi_write(offset: usize, val: u32) {
    unsafe { core::ptr::write_volatile((kmi_base() + offset) as *mut u32, val) }
}
#[inline]
unsafe fn kmi_read(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((kmi_base() + offset) as *const u32) }
}

/// Input queue — stores decoded ASCII characters.
static INPUT_QUEUE: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());

/// PS/2 scan-code decoder state.
struct ScanDecoder {
    extended: bool,
    released: bool,
}

impl ScanDecoder {
    const fn new() -> Self {
        Self {
            extended: false,
            released: false,
        }
    }

    /// Feed one byte; returns an ASCII char if a key press was decoded.
    fn feed(&mut self, byte: u8) -> Option<u8> {
        match byte {
            0xE0 => {
                self.extended = true;
                None
            }
            0xF0 => {
                self.released = true;
                None
            }
            scan => {
                let ext = self.extended;
                let rel = self.released;
                self.extended = false;
                self.released = false;
                if rel {
                    return None;
                } // key release — ignore
                if ext {
                    return None;
                } // extended keys — ignore for now
                scancode_to_ascii(scan)
            }
        }
    }
}

static DECODER: Mutex<ScanDecoder> = Mutex::new(ScanDecoder::new());

/// Initialise the PL050 KMI and register the IRQ handler.
pub fn init() {
    unsafe {
        // Enable KMI with RX interrupt
        kmi_write(KMICR, KMICR_EN | KMICR_RXINTREN);
    }
    crate::hal::gic::register_handler(KMI_IRQ, on_kmi_irq);
    log::debug!("PL050 keyboard @ {:#x}, IRQ {}", KMI_BASE, KMI_IRQ);
}

/// Try to read one ASCII character from the input queue (non-blocking).
pub fn try_read() -> Option<u8> {
    INPUT_QUEUE.lock().pop_front()
}

/// IRQ handler — reads all available scan codes and queues decoded characters.
fn on_kmi_irq() {
    loop {
        let stat = unsafe { kmi_read(KMISTAT) };
        if stat & KMISTAT_RXFULL == 0 {
            break;
        }
        let byte = (unsafe { kmi_read(KMIDATA) } & 0xFF) as u8;
        if let Some(ch) = DECODER.lock().feed(byte) {
            INPUT_QUEUE.lock().push_back(ch);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PS/2 Set-2 scan code → ASCII (US QWERTY layout, unshifted)
// ─────────────────────────────────────────────────────────────────────────────

fn scancode_to_ascii(scan: u8) -> Option<u8> {
    let c = match scan {
        0x1C => b'a',
        0x32 => b'b',
        0x21 => b'c',
        0x23 => b'd',
        0x24 => b'e',
        0x2B => b'f',
        0x34 => b'g',
        0x33 => b'h',
        0x43 => b'i',
        0x3B => b'j',
        0x42 => b'k',
        0x4B => b'l',
        0x3A => b'm',
        0x31 => b'n',
        0x44 => b'o',
        0x4D => b'p',
        0x15 => b'q',
        0x2D => b'r',
        0x1B => b's',
        0x2C => b't',
        0x3C => b'u',
        0x2A => b'v',
        0x1D => b'w',
        0x22 => b'x',
        0x35 => b'y',
        0x1A => b'z',
        0x45 => b'0',
        0x16 => b'1',
        0x1E => b'2',
        0x26 => b'3',
        0x25 => b'4',
        0x2E => b'5',
        0x36 => b'6',
        0x3D => b'7',
        0x3E => b'8',
        0x46 => b'9',
        0x29 => b' ',
        0x5A => b'\n',
        0x66 => b'\x08', // space, enter, backspace
        0x0D => b'\t',
        0x41 => b',',
        0x49 => b'.',
        0x4A => b'/',
        0x4E => b'-',
        0x55 => b'=',
        0x54 => b'[',
        0x5B => b']',
        0x4C => b';',
        0x52 => b'\'',
        0x5D => b'\\',
        0x0E => b'`',
        _ => return None,
    };
    Some(c)
}
