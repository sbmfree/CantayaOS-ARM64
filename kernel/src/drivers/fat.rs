//! FAT32 read-only driver.
//!
//! Reads bounded FAT32 root files through a kernel-owned sector source.
//!
//! Supports:
//!   - FAT32 BPB parsing
//   - Root-directory traversal (8.3 names)
//!   - File read into a caller-supplied buffer

pub const SECTOR_SIZE: usize = 512;

/// Read-only source of 512-byte disk sectors.
pub trait BlockDevice {
    fn sector_count(&self) -> u64;
    fn read_sector(&self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> bool;
}

const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_LONG_NAME: u8 = 0x0F;

/// FAT32 reader backed by a live block device instead of retained boot bytes.
pub struct LiveFatVolume<'a> {
    device: &'a dyn BlockDevice,
    sectors_per_cluster: u64,
    fat_start_sector: u64,
    data_start_sector: u64,
    total_sectors: u64,
    root_cluster: u32,
    last_cluster: u32,
}

impl<'a> LiveFatVolume<'a> {
    /// Open a FAT32 volume without reading more than `max_sectors` from the
    /// kernel-controlled block device.
    pub fn open_bounded(device: &'a dyn BlockDevice, max_sectors: u64) -> Option<Self> {
        let mut boot_sector = [0u8; SECTOR_SIZE];
        if !device.read_sector(0, &mut boot_sector) {
            return None;
        }

        let bytes_per_sector = read_u16(&boot_sector, 11)? as usize;
        let sectors_per_cluster = boot_sector[13] as u64;
        let reserved_sectors = read_u16(&boot_sector, 14)? as u64;
        let fat_count = boot_sector[16] as u64;
        let total_sectors16 = read_u16(&boot_sector, 19)? as u64;
        let total_sectors32 = read_u32(&boot_sector, 32)? as u64;
        let fat_sectors = read_u32(&boot_sector, 36)? as u64;
        let root_cluster = read_u32(&boot_sector, 44)?;
        let total_sectors = if total_sectors16 != 0 {
            total_sectors16
        } else {
            total_sectors32
        };

        if bytes_per_sector != SECTOR_SIZE
            || sectors_per_cluster == 0
            || !sectors_per_cluster.is_power_of_two()
            || fat_count == 0
            || fat_sectors == 0
            || root_cluster < 2
            || total_sectors == 0
            || total_sectors > max_sectors
            || total_sectors > device.sector_count()
        {
            return None;
        }

        let fat_start_sector = reserved_sectors;
        let data_start_sector =
            fat_start_sector.checked_add(fat_count.checked_mul(fat_sectors)?)?;
        if data_start_sector >= total_sectors {
            return None;
        }
        let cluster_count = (total_sectors - data_start_sector) / sectors_per_cluster;
        let last_cluster = u32::try_from(cluster_count.checked_add(1)?).ok()?;
        let fat_entries = fat_sectors.checked_mul(SECTOR_SIZE as u64)? / 4;
        if last_cluster < 2 || root_cluster > last_cluster || last_cluster as u64 >= fat_entries {
            return None;
        }

        Some(Self {
            device,
            sectors_per_cluster,
            fat_start_sector,
            data_start_sector,
            total_sectors,
            root_cluster,
            last_cluster,
        })
    }

    /// Read a root-level 8.3 file into `buffer`, rejecting files beyond the
    /// caller-provided image size cap.
    pub fn read_file_to_buf_bounded(
        &self,
        name83: &[u8; 11],
        buffer: &mut alloc::vec::Vec<u8>,
        max_size: usize,
    ) -> bool {
        let Some((mut cluster, size)) = self.find_in_root(name83) else {
            return false;
        };
        let Ok(size) = usize::try_from(size) else {
            return false;
        };
        if size > max_size {
            return false;
        }

        buffer.clear();
        buffer.reserve(size);
        if size == 0 {
            return true;
        }
        let mut sector = [0u8; SECTOR_SIZE];
        let mut remaining = size;
        for _ in 0..self.last_cluster.saturating_sub(1) {
            let Some(cluster_sector) = self.cluster_to_sector(cluster) else {
                return false;
            };
            for sector_index in 0..self.sectors_per_cluster {
                if !self.read_sector(cluster_sector + sector_index, &mut sector) {
                    return false;
                }
                let take = remaining.min(SECTOR_SIZE);
                buffer.extend_from_slice(&sector[..take]);
                remaining -= take;
                if remaining == 0 {
                    return true;
                }
            }
            let Some(next) = self.next_cluster(cluster) else {
                return false;
            };
            cluster = next;
        }
        false
    }

    fn find_in_root(&self, name83: &[u8; 11]) -> Option<(u32, u32)> {
        let mut cluster = self.root_cluster;
        let mut sector = [0u8; SECTOR_SIZE];
        for _ in 0..self.last_cluster.saturating_sub(1) {
            let cluster_sector = self.cluster_to_sector(cluster)?;
            for sector_index in 0..self.sectors_per_cluster {
                if !self.read_sector(cluster_sector + sector_index, &mut sector) {
                    return None;
                }
                for entry in sector.chunks_exact(32) {
                    if entry[0] == 0x00 {
                        return None;
                    }
                    if entry[0] == 0xE5
                        || entry[11] == ATTR_LONG_NAME
                        || entry[11] & ATTR_DIRECTORY != 0
                    {
                        continue;
                    }
                    if &entry[..11] != name83 {
                        continue;
                    }
                    let cluster_hi = read_u16(entry, 20)? as u32;
                    let cluster_lo = read_u16(entry, 26)? as u32;
                    return Some(((cluster_hi << 16) | cluster_lo, read_u32(entry, 28)?));
                }
            }
            cluster = self.next_cluster(cluster)?;
        }
        None
    }

    fn cluster_to_sector(&self, cluster: u32) -> Option<u64> {
        if cluster < 2 || cluster > self.last_cluster {
            return None;
        }
        self.data_start_sector
            .checked_add((cluster as u64 - 2).checked_mul(self.sectors_per_cluster)?)
    }

    fn next_cluster(&self, cluster: u32) -> Option<u32> {
        if cluster < 2 || cluster > self.last_cluster {
            return None;
        }
        let entry_offset = cluster as u64 * 4;
        let sector = self
            .fat_start_sector
            .checked_add(entry_offset / SECTOR_SIZE as u64)?;
        let offset = (entry_offset % SECTOR_SIZE as u64) as usize;
        let mut fat_sector = [0u8; SECTOR_SIZE];
        if !self.read_sector(sector, &mut fat_sector) {
            return None;
        }
        let next = read_u32(&fat_sector, offset)? & 0x0FFF_FFFF;
        if next >= 0x0FFF_FFF8 {
            None
        } else if next < 2 || next > self.last_cluster {
            None
        } else {
            Some(next)
        }
    }

    fn read_sector(&self, sector: u64, buffer: &mut [u8; SECTOR_SIZE]) -> bool {
        sector < self.total_sectors && self.device.read_sector(sector, buffer)
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}
