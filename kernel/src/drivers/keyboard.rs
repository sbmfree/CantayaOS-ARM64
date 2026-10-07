//! Interrupt-driven VirtIO keyboard with text-console and desktop event delivery.
use super::virtio_input::{InputEvent, VirtioInput};
use cantaya_shared::desktop::{DesktopEvent, KEY};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
const PENDING_KEYS: usize = 128;
const EV_KEY: u16 = 1;
struct VirtioKeyboard {
    device: VirtioInput,
    left_shift: bool,
    right_shift: bool,
    left_ctrl: bool,
    right_ctrl: bool,
    caps_lock: bool,
    pending: [u8; PENDING_KEYS],
    pending_head: usize,
    pending_len: usize,
}
static KEYBOARD: Mutex<Option<VirtioKeyboard>> = Mutex::new(None);
static INPUT_INTERRUPT_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    match VirtioInput::new(false) {
        Ok(device) => {
            let irq = device.irq();
            log::info!("VirtIO keyboard: MMIO input ready at {:#x}", device.base);
            *KEYBOARD.lock() = Some(VirtioKeyboard {
                device,
                left_shift: false,
                right_shift: false,
                left_ctrl: false,
                right_ctrl: false,
                caps_lock: false,
                pending: [0; PENDING_KEYS],
                pending_head: 0,
                pending_len: 0,
            });
            crate::hal::gic::register_handler(irq, on_input_interrupt);
        }
        Err(error) => log::warn!("VirtIO keyboard unavailable: {:?}", error),
    }
}
fn on_input_interrupt() {
    let irq = crate::executive::ke::spinlock::IrqState::disable();
    let received = KEYBOARD.lock().as_mut().is_some_and(|k| {
        k.drain_pending();
        if k.device.acknowledge() {
            INPUT_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        k.drain_pending();
        k.pending_len != 0
    });
    if received || crate::desktop::has_pending() {
        crate::console::wake_ready_waiter();
    }
    irq.restore();
}
pub fn input_interrupt_count() -> u64 {
    INPUT_INTERRUPT_COUNT.load(Ordering::Relaxed)
}
pub fn try_read() -> Option<u8> {
    let irq = crate::executive::ke::spinlock::IrqState::disable();
    let result = KEYBOARD.lock().as_mut().and_then(|k| {
        k.drain_pending();
        k.pop_pending()
    });
    irq.restore();
    result
}
pub fn has_pending_byte() -> bool {
    let irq = crate::executive::ke::spinlock::IrqState::disable();
    let result = KEYBOARD.lock().as_mut().is_some_and(|k| {
        k.drain_pending();
        k.pending_len != 0
    });
    irq.restore();
    result
}
impl VirtioKeyboard {
    fn drain_pending(&mut self) {
        if crate::desktop::is_active() {
            // Preserve bytes decoded while the graphical session was starting.
            while let Some(byte) = self.pop_pending() {
                crate::desktop::queue_event(DesktopEvent {
                    kind: cantaya_shared::desktop::TEXT,
                    text: u32::from(byte),
                    ..DesktopEvent::default()
                });
            }
        }
        for _ in 0..self.device.queue_size {
            if self.pending_len == PENDING_KEYS {
                break;
            }
            let Some(event) = self.device.poll() else {
                break;
            };
            let byte = self.decode(event);
            if crate::desktop::is_active() {
                if u16::from_le(event.event_type) == EV_KEY {
                    crate::desktop::queue_event(DesktopEvent {
                        kind: KEY,
                        code: u32::from(u16::from_le(event.code)),
                        value: u32::from_le(event.value) as i32,
                        text: byte.map_or(0, u32::from),
                    });
                }
            } else if let Some(byte) = byte {
                let tail = (self.pending_head + self.pending_len) % PENDING_KEYS;
                self.pending[tail] = byte;
                self.pending_len += 1;
            }
        }
    }
    fn pop_pending(&mut self) -> Option<u8> {
        if self.pending_len == 0 {
            return None;
        }
        let byte = self.pending[self.pending_head];
        self.pending_head = (self.pending_head + 1) % PENDING_KEYS;
        self.pending_len -= 1;
        Some(byte)
    }
    fn decode(&mut self, event: InputEvent) -> Option<u8> {
        if u16::from_le(event.event_type) != EV_KEY {
            return None;
        }
        let code = u16::from_le(event.code);
        let value = u32::from_le(event.value);
        let pressed = value != 0;
        match code {
            42 => self.left_shift = pressed,
            54 => self.right_shift = pressed,
            29 => self.left_ctrl = pressed,
            97 => self.right_ctrl = pressed,
            58 if value == 1 => self.caps_lock = !self.caps_lock,
            _ => {}
        }
        if !pressed {
            return None;
        }
        if self.left_ctrl || self.right_ctrl {
            return match code {
                22 => Some(0x15), // Ctrl-U
                38 => Some(0x0c), // Ctrl-L
                _ => None,
            };
        }
        match code {
            14 => return Some(8),          // Backspace
            28 | 96 => return Some(b'\r'), // Enter
            57 => return Some(b' '),
            _ => {}
        }
        let (plain, shifted) = ascii_for_key(code)?;
        let shift = self.left_shift || self.right_shift;
        if plain.is_ascii_lowercase() {
            Some(if shift ^ self.caps_lock {
                shifted
            } else {
                plain
            })
        } else {
            Some(if shift { shifted } else { plain })
        }
    }
}

fn ascii_for_key(code: u16) -> Option<(u8, u8)> {
    Some(match code {
        2 => (b'1', b'!'),
        3 => (b'2', b'@'),
        4 => (b'3', b'#'),
        5 => (b'4', b'$'),
        6 => (b'5', b'%'),
        7 => (b'6', b'^'),
        8 => (b'7', b'&'),
        9 => (b'8', b'*'),
        10 => (b'9', b'('),
        11 => (b'0', b')'),
        12 => (b'-', b'_'),
        13 => (b'=', b'+'),
        16 => (b'q', b'Q'),
        17 => (b'w', b'W'),
        18 => (b'e', b'E'),
        19 => (b'r', b'R'),
        20 => (b't', b'T'),
        21 => (b'y', b'Y'),
        22 => (b'u', b'U'),
        23 => (b'i', b'I'),
        24 => (b'o', b'O'),
        25 => (b'p', b'P'),
        26 => (b'[', b'{'),
        27 => (b']', b'}'),
        30 => (b'a', b'A'),
        31 => (b's', b'S'),
        32 => (b'd', b'D'),
        33 => (b'f', b'F'),
        34 => (b'g', b'G'),
        35 => (b'h', b'H'),
        36 => (b'j', b'J'),
        37 => (b'k', b'K'),
        38 => (b'l', b'L'),
        39 => (b';', b':'),
        40 => (b'\'', b'"'),
        41 => (b'`', b'~'),
        43 => (b'\\', b'|'),
        44 => (b'z', b'Z'),
        45 => (b'x', b'X'),
        46 => (b'c', b'C'),
        47 => (b'v', b'V'),
        48 => (b'b', b'B'),
        49 => (b'n', b'N'),
        50 => (b'm', b'M'),
        51 => (b',', b'<'),
        52 => (b'.', b'>'),
        53 => (b'/', b'?'),
        _ => return None,
    })
}
