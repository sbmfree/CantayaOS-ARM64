//! Polled VirtIO input keyboard on QEMU `virt` MMIO.
//!
//! Only the event queue is consumed. LED feedback through the status queue is
//! not needed by the built-in terminal.

use aarch64_cpu::asm::barrier;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

const MMIO_BASE: u64 = 0x0A00_0000;
const MMIO_STRIDE: u64 = 0x200;
const MMIO_SLOTS: u64 = 32;
const VIRTIO_IRQ_BASE: usize = 48; // QEMU virt SPI 16
const MAGIC_VALUE: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const INTERRUPT_STATUS: usize = 0x060;
const INTERRUPT_ACK: usize = 0x064;
const STATUS: usize = 0x070;
const QUEUE_DESC_LOW: usize = 0x080;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DEVICE_LOW: usize = 0x0A0;
const CONFIG_SELECT: usize = 0x100;
const CONFIG_SUBSEL: usize = 0x101;
const CONFIG_SIZE: usize = 0x102;
const CONFIG_BITMAP: usize = 0x108;

const VIRTIO_MAGIC: u32 = 0x7472_6976;
const MODERN_MMIO_VERSION: u32 = 2;
const INPUT_DEVICE_ID: u32 = 18;
const VERSION_1_FEATURE: u32 = 1;
const ACKNOWLEDGE: u32 = 1;
const DRIVER: u32 = 2;
const DRIVER_OK: u32 = 4;
const FEATURES_OK: u32 = 8;

const MAX_EVENTS: u16 = 64;
const PENDING_KEYS: usize = 128;
const DESC_WRITE: u16 = 2;
const DESC_OFFSET: usize = 0;
const AVAIL_OFFSET: usize = 1024;
const USED_OFFSET: usize = 2048;
const STATUS_DESC_OFFSET: usize = 3072;
const STATUS_AVAIL_OFFSET: usize = 3104;
const STATUS_USED_OFFSET: usize = 3136;
const EV_KEY: u16 = 1;
const EV_BITS: u8 = 0x11;

#[repr(C)]
struct VirtqDesc {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct VirtqUsedElem {
    id: u32,
    length: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct InputEvent {
    event_type: u16,
    code: u16,
    value: u32,
}

struct VirtioKeyboard {
    base: u64,
    queue_phys: u64,
    events_phys: u64,
    queue_size: u16,
    last_used: u16,
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

#[derive(Debug)]
enum InitError {
    MissingDevice,
    UnsupportedVersion,
    MissingVersionFeature,
    FeaturesRejected,
    UnsupportedKeys,
    QueueUnavailable,
}

/// Attach QEMU's VirtIO-MMIO keyboard. Serial input remains available if the
/// device is absent or fails negotiation.
pub fn init() {
    let mut keyboard = KEYBOARD.lock();
    if keyboard.is_some() {
        return;
    }
    match VirtioKeyboard::new() {
        Ok(device) => {
            log::info!("VirtIO keyboard: MMIO input ready at {:#x}", device.base);
            let slot = ((device.base - MMIO_BASE) / MMIO_STRIDE) as usize;
            *keyboard = Some(device);
            drop(keyboard);
            crate::hal::gic::register_handler(VIRTIO_IRQ_BASE + slot, on_input_interrupt);
        }
        Err(error) => log::warn!("VirtIO keyboard unavailable: {:?}", error),
    }
}

fn on_input_interrupt() {
    let irq_state = crate::executive::ke::spinlock::IrqState::disable();
    let received = KEYBOARD.lock().as_mut().is_some_and(|keyboard| {
        let status = read_reg(keyboard.base, INTERRUPT_STATUS);
        // Drain before and after ACK: an event that arrives just before ACK
        // can share the asserted ISR bit, so it must not be left unwoken.
        keyboard.drain_pending();
        if status != 0 {
            write_reg(keyboard.base, INTERRUPT_ACK, status);
        }
        keyboard.drain_pending();
        if status & 1 != 0 {
            INPUT_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        keyboard.pending_len != 0
    });
    if received {
        crate::console::wake_ready_waiter();
    }
    irq_state.restore();
}

pub fn input_interrupt_count() -> u64 {
    INPUT_INTERRUPT_COUNT.load(Ordering::Relaxed)
}

/// Return one decoded input byte, if the event queue has a key press.
pub fn try_read() -> Option<u8> {
    let irq_state = crate::executive::ke::spinlock::IrqState::disable();
    let result = KEYBOARD.lock().as_mut().and_then(|keyboard| {
        keyboard.drain_pending();
        keyboard.pop_pending()
    });
    irq_state.restore();
    result
}

/// Drain available VirtIO events into a bounded decoded-key FIFO, then report
/// whether an input byte can be read. Modifiers alone do not wake the shell.
pub fn has_pending_byte() -> bool {
    let irq_state = crate::executive::ke::spinlock::IrqState::disable();
    let pending = KEYBOARD.lock().as_mut().is_some_and(|keyboard| {
        keyboard.drain_pending();
        keyboard.pending_len != 0
    });
    irq_state.restore();
    pending
}

impl VirtioKeyboard {
    fn new() -> Result<Self, InitError> {
        let base = find_device().ok_or(InitError::MissingDevice)?;
        if read_reg(base, VERSION) != MODERN_MMIO_VERSION {
            return Err(InitError::UnsupportedVersion);
        }

        write_reg(base, STATUS, 0);
        write_reg(base, STATUS, ACKNOWLEDGE);
        write_reg(base, STATUS, ACKNOWLEDGE | DRIVER);

        write_reg(base, DEVICE_FEATURES_SEL, 1);
        if read_reg(base, DEVICE_FEATURES) & VERSION_1_FEATURE == 0 {
            return Err(InitError::MissingVersionFeature);
        }
        write_reg(base, DRIVER_FEATURES_SEL, 0);
        write_reg(base, DRIVER_FEATURES, 0);
        write_reg(base, DRIVER_FEATURES_SEL, 1);
        write_reg(base, DRIVER_FEATURES, VERSION_1_FEATURE);
        write_reg(base, STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK);
        if read_reg(base, STATUS) & FEATURES_OK == 0 {
            return Err(InitError::FeaturesRejected);
        }

        // The device must advertise keyboard events before we interpret
        // evdev codes as keys. Query event types, then the EV_KEY bitmap.
        // QEMU's keyboard reports the EV_KEY code bitmap even when its event
        // type bitmap (subselector zero) has zero length.
        let _ = config_has_bit(base, 0, EV_KEY as usize);
        let supports_a = config_has_bit(base, EV_KEY as u8, 30);
        let supports_enter = config_has_bit(base, EV_KEY as u8, 28);
        if !supports_a || !supports_enter {
            return Err(InitError::UnsupportedKeys);
        }

        write_reg(base, QUEUE_SEL, 0);
        let event_max = read_reg(base, QUEUE_NUM_MAX);
        if read_reg(base, QUEUE_READY) != 0 || event_max < 8 {
            return Err(InitError::QueueUnavailable);
        }
        let mut queue_size = 8u16;
        while u32::from(queue_size) * 2 <= event_max && queue_size < MAX_EVENTS {
            queue_size *= 2;
        }

        write_reg(base, QUEUE_SEL, 1);
        if read_reg(base, QUEUE_READY) != 0 || read_reg(base, QUEUE_NUM_MAX) == 0 {
            return Err(InitError::QueueUnavailable);
        }

        let queue_phys = crate::executive::mm::phys::alloc_page();
        let events_phys = crate::executive::mm::phys::alloc_page();
        let queue = crate::arch::mmu::phys_to_direct_map(queue_phys) as usize;
        let events = crate::arch::mmu::phys_to_direct_map(events_phys) as usize;
        unsafe {
            core::ptr::write_bytes(queue as *mut u8, 0, 4096);
            core::ptr::write_bytes(events as *mut u8, 0, 4096);
            for id in 0..queue_size {
                ((queue + DESC_OFFSET + id as usize * core::mem::size_of::<VirtqDesc>())
                    as *mut VirtqDesc)
                    .write(VirtqDesc {
                        address: events_phys
                            + id as u64 * core::mem::size_of::<InputEvent>() as u64,
                        length: core::mem::size_of::<InputEvent>() as u32,
                        flags: DESC_WRITE,
                        next: 0,
                    });
                core::ptr::write_volatile(
                    (queue + AVAIL_OFFSET + 4 + id as usize * 2) as *mut u16,
                    id,
                );
            }
        }
        barrier::dsb(barrier::ISHST);
        unsafe {
            core::ptr::write_volatile((queue + AVAIL_OFFSET + 2) as *mut u16, queue_size);
        }

        write_reg(base, QUEUE_SEL, 0);
        write_reg(base, QUEUE_NUM, queue_size as u32);
        write_address(base, QUEUE_DESC_LOW, queue_phys + DESC_OFFSET as u64);
        write_address(base, QUEUE_DRIVER_LOW, queue_phys + AVAIL_OFFSET as u64);
        write_address(base, QUEUE_DEVICE_LOW, queue_phys + USED_OFFSET as u64);
        write_reg(base, QUEUE_READY, 1);

        // Advertise the status queue even though this console sends no LEDs.
        write_reg(base, QUEUE_SEL, 1);
        write_reg(base, QUEUE_NUM, 1);
        write_address(base, QUEUE_DESC_LOW, queue_phys + STATUS_DESC_OFFSET as u64);
        write_address(
            base,
            QUEUE_DRIVER_LOW,
            queue_phys + STATUS_AVAIL_OFFSET as u64,
        );
        write_address(
            base,
            QUEUE_DEVICE_LOW,
            queue_phys + STATUS_USED_OFFSET as u64,
        );
        write_reg(base, QUEUE_READY, 1);

        write_reg(base, STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
        barrier::dsb(barrier::ISHST);
        write_reg(base, QUEUE_NOTIFY, 0);

        Ok(Self {
            base,
            queue_phys,
            events_phys,
            queue_size,
            last_used: 0,
            left_shift: false,
            right_shift: false,
            left_ctrl: false,
            right_ctrl: false,
            caps_lock: false,
            pending: [0; PENDING_KEYS],
            pending_head: 0,
            pending_len: 0,
        })
    }

    fn has_used_events(&self) -> bool {
        let queue = crate::arch::mmu::phys_to_direct_map(self.queue_phys) as usize;
        barrier::dsb(barrier::ISH);
        let used_index =
            unsafe { core::ptr::read_volatile((queue + USED_OFFSET + 2) as *const u16) };
        used_index != self.last_used
    }

    fn drain_pending(&mut self) {
        // Recycle descriptors even while EL0 is asleep. Each poll consumes up
        // to one queue's worth of events and returns at most one decoded byte.
        for _ in 0..self.queue_size {
            if self.pending_len == PENDING_KEYS || !self.has_used_events() {
                break;
            }
            if let Some(byte) = self.poll() {
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

    fn poll(&mut self) -> Option<u8> {
        let queue = crate::arch::mmu::phys_to_direct_map(self.queue_phys) as usize;
        let events = crate::arch::mmu::phys_to_direct_map(self.events_phys) as usize;
        for _ in 0..self.queue_size {
            barrier::dsb(barrier::ISH);
            let used_index =
                unsafe { core::ptr::read_volatile((queue + USED_OFFSET + 2) as *const u16) };
            if used_index == self.last_used {
                break;
            }
            let used_offset = queue
                + USED_OFFSET
                + 4
                + (self.last_used as usize % self.queue_size as usize)
                    * core::mem::size_of::<VirtqUsedElem>();
            let used = unsafe { core::ptr::read_volatile(used_offset as *const VirtqUsedElem) };
            self.last_used = self.last_used.wrapping_add(1);
            if used.id >= self.queue_size as u32 {
                break;
            }
            let event = if used.length >= core::mem::size_of::<InputEvent>() as u32 {
                Some(unsafe {
                    core::ptr::read_volatile(
                        (events + used.id as usize * core::mem::size_of::<InputEvent>())
                            as *const InputEvent,
                    )
                })
            } else {
                None
            };

            let avail_index =
                unsafe { core::ptr::read_volatile((queue + AVAIL_OFFSET + 2) as *const u16) };
            let avail_slot =
                queue + AVAIL_OFFSET + 4 + (avail_index as usize % self.queue_size as usize) * 2;
            unsafe { core::ptr::write_volatile(avail_slot as *mut u16, used.id as u16) };
            barrier::dsb(barrier::ISHST);
            unsafe {
                core::ptr::write_volatile(
                    (queue + AVAIL_OFFSET + 2) as *mut u16,
                    avail_index.wrapping_add(1),
                );
            }
            barrier::dsb(barrier::ISHST);
            write_reg(self.base, QUEUE_NOTIFY, 0);

            if let Some(event) = event {
                if let Some(byte) = self.decode(event) {
                    return Some(byte);
                }
            }
        }
        None
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

fn config_has_bit(base: u64, subsel: u8, bit: usize) -> bool {
    let config = crate::arch::mmu::phys_to_direct_map(base) as usize;
    unsafe {
        core::ptr::write_volatile((config + CONFIG_SELECT) as *mut u8, EV_BITS);
        core::ptr::write_volatile((config + CONFIG_SUBSEL) as *mut u8, subsel);
    }
    barrier::dsb(barrier::ISH);
    let size = unsafe { core::ptr::read_volatile((config + CONFIG_SIZE) as *const u8) } as usize;
    if bit / 8 >= size {
        return false;
    }
    let byte = unsafe { core::ptr::read_volatile((config + CONFIG_BITMAP + bit / 8) as *const u8) };
    byte & (1 << (bit % 8)) != 0
}

fn find_device() -> Option<u64> {
    (0..MMIO_SLOTS)
        .map(|slot| MMIO_BASE + slot * MMIO_STRIDE)
        .find(|&base| {
            read_reg(base, MAGIC_VALUE) == VIRTIO_MAGIC
                && read_reg(base, DEVICE_ID) == INPUT_DEVICE_ID
        })
}

fn read_reg(base: u64, offset: usize) -> u32 {
    unsafe {
        core::ptr::read_volatile(
            (crate::arch::mmu::phys_to_direct_map(base) as usize + offset) as *const u32,
        )
    }
}

fn write_reg(base: u64, offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile(
            (crate::arch::mmu::phys_to_direct_map(base) as usize + offset) as *mut u32,
            value,
        );
    }
}

fn write_address(base: u64, low_offset: usize, address: u64) {
    write_reg(base, low_offset, address as u32);
    write_reg(base, low_offset + 4, (address >> 32) as u32);
}
