//! Shared modern VirtIO-MMIO event-queue transport for keyboard and tablet.
use aarch64_cpu::asm::barrier;
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
const DESC_WRITE: u16 = 2;
const DESC_OFFSET: usize = 0;
const AVAIL_OFFSET: usize = 1024;
const USED_OFFSET: usize = 2048;
const STATUS_DESC_OFFSET: usize = 3072;
const STATUS_AVAIL_OFFSET: usize = 3104;
const STATUS_USED_OFFSET: usize = 3136;
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
pub struct InputEvent {
    pub event_type: u16,
    pub code: u16,
    pub value: u32,
}

pub struct VirtioInput {
    pub base: u64,
    queue_phys: u64,
    events_phys: u64,
    pub queue_size: u16,
    last_used: u16,
}
#[derive(Debug)]
pub enum InitError {
    MissingDevice,
    UnsupportedVersion,
    MissingVersionFeature,
    FeaturesRejected,
    QueueUnavailable,
}

impl VirtioInput {
    pub fn new(pointer: bool) -> Result<Self, InitError> {
        let base = find_device(pointer).ok_or(InitError::MissingDevice)?;
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
        })
    }

    pub fn poll(&mut self) -> Option<InputEvent> {
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

            if event.is_some() {
                return event;
            }
        }
        None
    }

    pub fn acknowledge(&mut self) -> bool {
        let status = read_reg(self.base, INTERRUPT_STATUS);
        if status != 0 {
            write_reg(self.base, INTERRUPT_ACK, status);
        }
        status & 1 != 0
    }
    pub fn irq(&self) -> usize {
        VIRTIO_IRQ_BASE + ((self.base - MMIO_BASE) / MMIO_STRIDE) as usize
    }
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

fn find_device(pointer: bool) -> Option<u64> {
    (0..MMIO_SLOTS)
        .map(|slot| MMIO_BASE + slot * MMIO_STRIDE)
        .find(|&base| {
            if read_reg(base, MAGIC_VALUE) != VIRTIO_MAGIC
                || read_reg(base, DEVICE_ID) != INPUT_DEVICE_ID
            {
                return false;
            }
            if pointer {
                config_has_bit(base, 3, 0)
                    && config_has_bit(base, 3, 1)
                    && config_has_bit(base, 1, 0x110)
            } else {
                config_has_bit(base, 1, 30) && config_has_bit(base, 1, 28)
            }
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
