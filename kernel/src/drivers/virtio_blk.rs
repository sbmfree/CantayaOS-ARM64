//! Minimal synchronous VirtIO 1.0 block reader for QEMU `virt` MMIO slot zero.

use crate::drivers::fat::{BlockDevice, SECTOR_SIZE};
use aarch64_cpu::asm::barrier;
use spin::Mutex;

const VIRTIO_MMIO_BASE: u64 = 0x0A00_0000;
const VIRTIO_MMIO_SLOT_STRIDE: u64 = 0x200;
const VIRTIO_MMIO_SLOT_COUNT: u64 = 32;
const VIRTIO_MMIO_MAGIC_VALUE: usize = 0x000;
const VIRTIO_MMIO_VERSION: usize = 0x004;
const VIRTIO_MMIO_DEVICE_ID: usize = 0x008;
const VIRTIO_MMIO_DEVICE_FEATURES: usize = 0x010;
const VIRTIO_MMIO_DEVICE_FEATURES_SEL: usize = 0x014;
const VIRTIO_MMIO_DRIVER_FEATURES: usize = 0x020;
const VIRTIO_MMIO_DRIVER_FEATURES_SEL: usize = 0x024;
const VIRTIO_MMIO_QUEUE_SEL: usize = 0x030;
const VIRTIO_MMIO_QUEUE_NUM_MAX: usize = 0x034;
const VIRTIO_MMIO_QUEUE_NUM: usize = 0x038;
const VIRTIO_MMIO_QUEUE_READY: usize = 0x044;
const VIRTIO_MMIO_QUEUE_NOTIFY: usize = 0x050;
const VIRTIO_MMIO_INTERRUPT_STATUS: usize = 0x060;
const VIRTIO_MMIO_INTERRUPT_ACK: usize = 0x064;
const VIRTIO_MMIO_STATUS: usize = 0x070;
const VIRTIO_MMIO_QUEUE_DESC_LOW: usize = 0x080;
const VIRTIO_MMIO_QUEUE_DRIVER_LOW: usize = 0x090;
const VIRTIO_MMIO_QUEUE_DEVICE_LOW: usize = 0x0A0;
const VIRTIO_BLK_CONFIG_CAPACITY: usize = 0x100;
pub const MAX_BOOT_VOLUME_SECTORS: u64 = 128 * 1024 * 1024 / SECTOR_SIZE as u64;

const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_MMIO_VERSION_1: u32 = 2;
const VIRTIO_DEVICE_BLOCK: u32 = 2;
const VIRTIO_F_VERSION_1: u32 = 1;
const VIRTIO_STATUS_ACKNOWLEDGE: u32 = 1;
const VIRTIO_STATUS_DRIVER: u32 = 2;
const VIRTIO_STATUS_DRIVER_OK: u32 = 4;
const VIRTIO_STATUS_FEATURES_OK: u32 = 8;

const QUEUE_SIZE: u16 = 4;
const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2;
const QUEUE_DESC_OFFSET: usize = 0;
const QUEUE_AVAIL_OFFSET: usize = 64;
const QUEUE_USED_OFFSET: usize = 80;
const BUFFER_HEADER_OFFSET: usize = 0;
const BUFFER_STATUS_OFFSET: usize = 16;
const BUFFER_DATA_OFFSET: usize = 512;
const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_S_OK: u8 = 0;
const POLL_LIMIT: usize = 1_000_000;

#[repr(C)]
struct VirtqDesc {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct VirtioBlkReqHeader {
    request_type: u32,
    reserved: u32,
    sector: u64,
}

#[repr(C)]
struct VirtqUsedElem {
    id: u32,
    length: u32,
}

struct VirtioBlock {
    mmio_base: u64,
    queue_phys: u64,
    buffer_phys: u64,
    sector_count: u64,
    last_used: u16,
}

static BOOT_BLOCK: Mutex<Option<VirtioBlock>> = Mutex::new(None);

/// Fixed block device containing the FAT boot volume on QEMU `virt`.
pub struct BootDisk;

pub static BOOT_DISK: BootDisk = BootDisk;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InitError {
    MissingDevice,
    UnsupportedVersion,
    MissingVersionFeature,
    FeaturesRejected,
    QueueUnavailable,
}

/// Initialise the fixed QEMU VirtIO-MMIO block device before Ps can load
/// runtime program sources.
pub fn init() {
    let mut device = BOOT_BLOCK.lock();
    if device.is_some() {
        return;
    }
    let block = VirtioBlock::new().expect("failed to initialize QEMU VirtIO block device");
    log::info!(
        "VirtIO block: live read-only boot disk ready at {:#x} ({} sectors)",
        block.mmio_base,
        block.sector_count,
    );
    *device = Some(block);
}

impl BlockDevice for BootDisk {
    fn sector_count(&self) -> u64 {
        BOOT_BLOCK
            .lock()
            .as_ref()
            .map_or(0, |block| block.sector_count.min(MAX_BOOT_VOLUME_SECTORS))
    }

    fn read_sector(&self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> bool {
        if sector >= self.sector_count() {
            return false;
        }
        let irq_state = crate::executive::ke::spinlock::IrqState::disable();
        let result = BOOT_BLOCK
            .lock()
            .as_mut()
            .is_some_and(|block| block.read_sector(sector, output));
        irq_state.restore();
        result
    }
}

impl VirtioBlock {
    fn new() -> Result<Self, InitError> {
        let mmio_base = find_block_device().ok_or(InitError::MissingDevice)?;
        if read_reg(mmio_base, VIRTIO_MMIO_VERSION) != VIRTIO_MMIO_VERSION_1 {
            return Err(InitError::UnsupportedVersion);
        }

        write_reg(mmio_base, VIRTIO_MMIO_STATUS, 0);
        write_reg(mmio_base, VIRTIO_MMIO_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
        write_reg(
            mmio_base,
            VIRTIO_MMIO_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER,
        );

        write_reg(mmio_base, VIRTIO_MMIO_DEVICE_FEATURES_SEL, 1);
        if read_reg(mmio_base, VIRTIO_MMIO_DEVICE_FEATURES) & VIRTIO_F_VERSION_1 == 0 {
            return Err(InitError::MissingVersionFeature);
        }
        write_reg(mmio_base, VIRTIO_MMIO_DRIVER_FEATURES_SEL, 0);
        write_reg(mmio_base, VIRTIO_MMIO_DRIVER_FEATURES, 0);
        write_reg(mmio_base, VIRTIO_MMIO_DRIVER_FEATURES_SEL, 1);
        write_reg(mmio_base, VIRTIO_MMIO_DRIVER_FEATURES, VIRTIO_F_VERSION_1);

        write_reg(
            mmio_base,
            VIRTIO_MMIO_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
        );
        if read_reg(mmio_base, VIRTIO_MMIO_STATUS) & VIRTIO_STATUS_FEATURES_OK == 0 {
            return Err(InitError::FeaturesRejected);
        }

        write_reg(mmio_base, VIRTIO_MMIO_QUEUE_SEL, 0);
        if read_reg(mmio_base, VIRTIO_MMIO_QUEUE_READY) != 0
            || read_reg(mmio_base, VIRTIO_MMIO_QUEUE_NUM_MAX) < QUEUE_SIZE as u32
        {
            return Err(InitError::QueueUnavailable);
        }
        let queue_phys = crate::executive::mm::phys::alloc_page();
        let buffer_phys = crate::executive::mm::phys::alloc_page();
        unsafe {
            core::ptr::write_bytes(
                crate::arch::mmu::phys_to_direct_map(queue_phys) as *mut u8,
                0,
                4096,
            );
            core::ptr::write_bytes(
                crate::arch::mmu::phys_to_direct_map(buffer_phys) as *mut u8,
                0,
                4096,
            );
        }

        write_reg(mmio_base, VIRTIO_MMIO_QUEUE_NUM, QUEUE_SIZE as u32);
        write_address(
            mmio_base,
            VIRTIO_MMIO_QUEUE_DESC_LOW,
            queue_phys + QUEUE_DESC_OFFSET as u64,
        );
        write_address(
            mmio_base,
            VIRTIO_MMIO_QUEUE_DRIVER_LOW,
            queue_phys + QUEUE_AVAIL_OFFSET as u64,
        );
        write_address(
            mmio_base,
            VIRTIO_MMIO_QUEUE_DEVICE_LOW,
            queue_phys + QUEUE_USED_OFFSET as u64,
        );
        write_reg(mmio_base, VIRTIO_MMIO_QUEUE_READY, 1);

        let sector_count = read_config_u64(mmio_base, VIRTIO_BLK_CONFIG_CAPACITY);
        if sector_count == 0 {
            return Err(InitError::MissingDevice);
        }
        write_reg(
            mmio_base,
            VIRTIO_MMIO_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE
                | VIRTIO_STATUS_DRIVER
                | VIRTIO_STATUS_FEATURES_OK
                | VIRTIO_STATUS_DRIVER_OK,
        );
        Ok(Self {
            mmio_base,
            queue_phys,
            buffer_phys,
            sector_count,
            last_used: 0,
        })
    }

    fn read_sector(&mut self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> bool {
        if sector >= self.sector_count {
            return false;
        }
        let queue = crate::arch::mmu::phys_to_direct_map(self.queue_phys) as usize;
        let buffer = crate::arch::mmu::phys_to_direct_map(self.buffer_phys) as usize;
        unsafe {
            ((buffer + BUFFER_HEADER_OFFSET) as *mut VirtioBlkReqHeader).write(
                VirtioBlkReqHeader {
                    request_type: VIRTIO_BLK_T_IN,
                    reserved: 0,
                    sector,
                },
            );
            core::ptr::write_volatile((buffer + BUFFER_STATUS_OFFSET) as *mut u8, u8::MAX);
            write_desc(
                queue,
                0,
                self.buffer_phys + BUFFER_HEADER_OFFSET as u64,
                core::mem::size_of::<VirtioBlkReqHeader>() as u32,
                DESC_NEXT,
                1,
            );
            write_desc(
                queue,
                1,
                self.buffer_phys + BUFFER_DATA_OFFSET as u64,
                SECTOR_SIZE as u32,
                DESC_NEXT | DESC_WRITE,
                2,
            );
            write_desc(
                queue,
                2,
                self.buffer_phys + BUFFER_STATUS_OFFSET as u64,
                1,
                DESC_WRITE,
                0,
            );

            let avail_index =
                core::ptr::read_volatile((queue + QUEUE_AVAIL_OFFSET + 2) as *const u16);
            let ring_offset =
                queue + QUEUE_AVAIL_OFFSET + 4 + (avail_index as usize % QUEUE_SIZE as usize) * 2;
            core::ptr::write_volatile(ring_offset as *mut u16, 0);
            barrier::dsb(barrier::ISHST);
            core::ptr::write_volatile(
                (queue + QUEUE_AVAIL_OFFSET + 2) as *mut u16,
                avail_index.wrapping_add(1),
            );
            barrier::dsb(barrier::ISHST);
            write_reg(self.mmio_base, VIRTIO_MMIO_QUEUE_NOTIFY, 0);

            for _ in 0..POLL_LIMIT {
                barrier::dsb(barrier::ISH);
                let used_index =
                    core::ptr::read_volatile((queue + QUEUE_USED_OFFSET + 2) as *const u16);
                if used_index == self.last_used {
                    continue;
                }
                let used_offset = queue
                    + QUEUE_USED_OFFSET
                    + 4
                    + (self.last_used as usize % QUEUE_SIZE as usize)
                        * core::mem::size_of::<VirtqUsedElem>();
                let used = core::ptr::read_volatile(used_offset as *const VirtqUsedElem);
                self.last_used = self.last_used.wrapping_add(1);
                let interrupt_status = read_reg(self.mmio_base, VIRTIO_MMIO_INTERRUPT_STATUS);
                if interrupt_status != 0 {
                    write_reg(self.mmio_base, VIRTIO_MMIO_INTERRUPT_ACK, interrupt_status);
                }
                if used.id != 0
                    || core::ptr::read_volatile((buffer + BUFFER_STATUS_OFFSET) as *const u8)
                        != VIRTIO_BLK_S_OK
                {
                    return false;
                }
                core::ptr::copy_nonoverlapping(
                    (buffer + BUFFER_DATA_OFFSET) as *const u8,
                    output.as_mut_ptr(),
                    SECTOR_SIZE,
                );
                return true;
            }
        }
        false
    }
}

#[inline]
unsafe fn write_desc(queue: usize, index: usize, address: u64, length: u32, flags: u16, next: u16) {
    unsafe {
        ((queue + QUEUE_DESC_OFFSET + index * core::mem::size_of::<VirtqDesc>()) as *mut VirtqDesc)
            .write(VirtqDesc {
                address,
                length,
                flags,
                next,
            });
    }
}

#[inline]
fn find_block_device() -> Option<u64> {
    (0..VIRTIO_MMIO_SLOT_COUNT)
        .map(|slot| VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_SLOT_STRIDE)
        .find(|&base| {
            read_reg(base, VIRTIO_MMIO_MAGIC_VALUE) == VIRTIO_MAGIC
                && read_reg(base, VIRTIO_MMIO_DEVICE_ID) == VIRTIO_DEVICE_BLOCK
        })
}

#[inline]
fn read_reg(base: u64, offset: usize) -> u32 {
    unsafe {
        core::ptr::read_volatile(
            (crate::arch::mmu::phys_to_direct_map(base) as usize + offset) as *const u32,
        )
    }
}

#[inline]
fn write_reg(base: u64, offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile(
            (crate::arch::mmu::phys_to_direct_map(base) as usize + offset) as *mut u32,
            value,
        );
    }
}

#[inline]
fn write_address(base: u64, low_offset: usize, address: u64) {
    write_reg(base, low_offset, address as u32);
    write_reg(base, low_offset + 4, (address >> 32) as u32);
}

#[inline]
fn read_config_u64(base: u64, offset: usize) -> u64 {
    read_reg(base, offset) as u64 | ((read_reg(base, offset + 4) as u64) << 32)
}
