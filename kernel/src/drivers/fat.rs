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

    fn write_sector(&self, _sector: u64, _input: &[u8; SECTOR_SIZE]) -> bool {
        false
    }

    fn flush(&self) -> bool {
        false
    }
}

const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_LONG_NAME: u8 = 0x0F;
const ATTR_VOLUME_ID: u8 = 0x08;
const MAX_DIRECTORY_CLUSTERS: usize = 256;

#[derive(Clone, Copy)]
pub struct RootFileEntry {
    pub name83: [u8; 11],
    pub size: u32,
    pub cluster: u32,
    pub is_directory: bool,
}

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
    /// Return one root entry by ordinal. `None` means end of directory.
    pub fn root_file_at(&self, index: usize) -> Option<RootFileEntry> {
        self.directory_entry_at(self.root_cluster, index)
    }

    pub fn directory_entry_at(&self, start: u32, index: usize) -> Option<RootFileEntry> {
        let mut cluster = start;
        let mut sector = [0u8; SECTOR_SIZE];
        let mut ordinal = 0usize;
        let mut seen = [0u32; MAX_DIRECTORY_CLUSTERS];
        for visited in 0..MAX_DIRECTORY_CLUSTERS.min(self.last_cluster.saturating_sub(1) as usize) {
            if seen[..visited].contains(&cluster) {
                return None;
            }
            seen[visited] = cluster;
            let cluster_sector = self.cluster_to_sector(cluster)?;
            for sector_index in 0..self.sectors_per_cluster {
                self.read_sector(cluster_sector + sector_index, &mut sector)
                    .then_some(())?;
                for entry in sector.chunks_exact(32) {
                    if entry[0] == 0x00 {
                        return None;
                    }
                    if entry[0] == 0xE5
                        || entry[11] == ATTR_LONG_NAME
                        || entry[11] & ATTR_VOLUME_ID != 0
                        || entry[0] == b'.'
                    {
                        continue;
                    }
                    if ordinal == index {
                        let mut name83 = [0u8; 11];
                        name83.copy_from_slice(&entry[..11]);
                        return Some(RootFileEntry {
                            name83,
                            size: read_u32(entry, 28)?,
                            cluster: Self::entry_cluster(entry)?,
                            is_directory: entry[11] & ATTR_DIRECTORY != 0,
                        });
                    }
                    ordinal += 1;
                }
            }
            cluster = self.next_cluster(cluster)?;
        }
        None
    }
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
        self.read_file_from_directory_to_buf_bounded(self.root_cluster, name83, buffer, max_size)
    }

    pub fn read_file_from_directory_to_buf_bounded(
        &self,
        directory: u32,
        name83: &[u8; 11],
        buffer: &mut alloc::vec::Vec<u8>,
        max_size: usize,
    ) -> bool {
        let Some(entry) = self.find_in_directory(directory, name83) else {
            return false;
        };
        if entry.is_directory {
            return false;
        }
        let mut cluster = entry.cluster;
        let Ok(size) = usize::try_from(entry.size) else {
            return false;
        };
        if size > max_size {
            return false;
        }

        let mut contents = alloc::vec::Vec::new();
        if contents.try_reserve_exact(size).is_err() {
            return false;
        }
        if size == 0 {
            *buffer = contents;
            return true;
        }
        let mut sector = [0u8; SECTOR_SIZE];
        let mut remaining = size;
        let mut seen = alloc::vec::Vec::new();
        let cluster_bytes = self.sectors_per_cluster as usize * SECTOR_SIZE;
        if seen
            .try_reserve_exact((size + cluster_bytes - 1) / cluster_bytes)
            .is_err()
        {
            return false;
        }
        for _ in 0..self.last_cluster.saturating_sub(1) {
            if seen.contains(&cluster) {
                return false;
            }
            seen.push(cluster);
            let Some(cluster_sector) = self.cluster_to_sector(cluster) else {
                return false;
            };
            for sector_index in 0..self.sectors_per_cluster {
                if !self.read_sector(cluster_sector + sector_index, &mut sector) {
                    return false;
                }
                let take = remaining.min(SECTOR_SIZE);
                contents.extend_from_slice(&sector[..take]);
                remaining -= take;
                if remaining == 0 {
                    *buffer = contents;
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

    pub fn find_directory(&self, name83: &[u8; 11]) -> Option<u32> {
        let entry = self.find_in_directory(self.root_cluster, name83)?;
        entry.is_directory.then_some(entry.cluster)
    }

    fn find_in_directory(&self, start: u32, name83: &[u8; 11]) -> Option<RootFileEntry> {
        let mut cluster = start;
        let mut sector = [0u8; SECTOR_SIZE];
        let mut seen = [0u32; MAX_DIRECTORY_CLUSTERS];
        for visited in 0..MAX_DIRECTORY_CLUSTERS.min(self.last_cluster.saturating_sub(1) as usize) {
            if seen[..visited].contains(&cluster) {
                return None;
            }
            seen[visited] = cluster;
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
                        || entry[11] & ATTR_VOLUME_ID != 0
                    {
                        continue;
                    }
                    if &entry[..11] != name83 {
                        continue;
                    }
                    return Some(RootFileEntry {
                        name83: *name83,
                        size: read_u32(entry, 28)?,
                        cluster: Self::entry_cluster(entry)?,
                        is_directory: entry[11] & ATTR_DIRECTORY != 0,
                    });
                }
            }
            cluster = self.next_cluster(cluster)?;
        }
        None
    }

    fn entry_cluster(entry: &[u8]) -> Option<u32> {
        let high = read_u16(entry, 20)? as u32;
        let low = read_u16(entry, 26)? as u32;
        Some((high << 16) | low)
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

/// Private QEMU smoke probe using faulted views of the live boot disk.
pub fn probe_malformed_media(device: &dyn BlockDevice, max_sectors: u64) -> bool {
    struct FaultedDevice<'a> {
        base: &'a dyn BlockDevice,
        corrupt_boot: u8,
        unreadable_sector: Option<u64>,
    }

    impl BlockDevice for FaultedDevice<'_> {
        fn sector_count(&self) -> u64 {
            self.base.sector_count()
        }

        fn read_sector(&self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> bool {
            if self.unreadable_sector == Some(sector) || !self.base.read_sector(sector, output) {
                return false;
            }
            if sector == 0 {
                match self.corrupt_boot {
                    1 => output[11..13].fill(0),
                    2 => output[44..48].fill(0xFF),
                    _ => {}
                }
            }
            true
        }
    }

    for corrupt_boot in [1, 2] {
        if LiveFatVolume::open_bounded(
            &FaultedDevice {
                base: device,
                corrupt_boot,
                unreadable_sector: None,
            },
            max_sectors,
        )
        .is_some()
        {
            return false;
        }
    }

    let mut boot = [0u8; SECTOR_SIZE];
    if !device.read_sector(0, &mut boot) {
        return false;
    }
    let Some(reserved) = read_u16(&boot, 14).map(u64::from) else {
        return false;
    };
    let Some(fat_sectors) = read_u32(&boot, 36).map(u64::from) else {
        return false;
    };
    let Some(root_cluster) = read_u32(&boot, 44).map(u64::from) else {
        return false;
    };
    let sectors_per_cluster = u64::from(boot[13]);
    let Some(root_sector) = reserved
        .checked_add(u64::from(boot[16]).saturating_mul(fat_sectors))
        .and_then(|start| {
            root_cluster
                .checked_sub(2)
                .and_then(|cluster| cluster.checked_mul(sectors_per_cluster))
                .and_then(|offset| start.checked_add(offset))
        })
    else {
        return false;
    };
    let faulted = FaultedDevice {
        base: device,
        corrupt_boot: 0,
        unreadable_sector: Some(root_sector),
    };
    LiveFatVolume::open_bounded(&faulted, max_sectors)
        .is_some_and(|volume| volume.root_file_at(0).is_none())
}
