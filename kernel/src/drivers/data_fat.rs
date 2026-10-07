use alloc::vec::Vec;
use core::cell::Cell;

use super::fat::{BlockDevice, LiveFatVolume, SECTOR_SIZE};

pub const MAX_CREATE_FILE_BYTES: usize = SECTOR_SIZE;
const MAX_FAT_SECTORS: u64 = 1024;
const MAX_SECTORS_PER_CLUSTER: u64 = 8;
const MAX_ROOT_DIRECTORY_SLOTS: usize = 128;
const JOURNAL_SECTOR_COUNT: u64 = 2;
const MIN_RESERVED_SECTORS: u64 = 16;
const JOURNAL_CHECKSUM_OFFSET: usize = SECTOR_SIZE - 4;
const TRANSACTION_DATA_CHECKSUM_OFFSET: usize = 32;
const JOURNAL_MAGIC: [u8; 8] = *b"CANTTXN1";
const JOURNAL_VERSION: u8 = 2;
const FAT_EOC: u32 = 0x0FFF_FFFF;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_LONG_NAME: u8 = 0x0F;
const ATTR_VOLUME_ID: u8 = 0x08;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataVolumeError {
    Device,
    Geometry,
    FatMismatch,
    InvalidName,
    AlreadyExists,
    DirectoryFull,
    NoSpace,
    TooLarge,
    Journal,
}

pub struct DataFatVolume<'a> {
    device: &'a dyn BlockDevice,
    sectors_per_cluster: u64,
    fat_start_sector: u64,
    fat_sectors: u64,
    data_start_sector: u64,
    total_sectors: u64,
    root_cluster: u32,
    last_cluster: u32,
    journal_primary_sector: u64,
    journal_secondary_sector: u64,
}

impl<'a> DataFatVolume<'a> {
    pub fn open_bounded(
        device: &'a dyn BlockDevice,
        max_sectors: u64,
    ) -> Result<Self, DataVolumeError> {
        let mut boot_sector = [0u8; SECTOR_SIZE];
        if !device.read_sector(0, &mut boot_sector) {
            return Err(DataVolumeError::Device);
        }

        let bytes_per_sector =
            read_u16(&boot_sector, 11).ok_or(DataVolumeError::Geometry)? as usize;
        let sectors_per_cluster = boot_sector[13] as u64;
        let reserved_sectors =
            read_u16(&boot_sector, 14).ok_or(DataVolumeError::Geometry)? as u64;
        let fat_count = boot_sector[16] as u64;
        let total_sectors16 =
            read_u16(&boot_sector, 19).ok_or(DataVolumeError::Geometry)? as u64;
        let total_sectors32 =
            read_u32(&boot_sector, 32).ok_or(DataVolumeError::Geometry)? as u64;
        let fat_sectors =
            read_u32(&boot_sector, 36).ok_or(DataVolumeError::Geometry)? as u64;
        let root_cluster = read_u32(&boot_sector, 44).ok_or(DataVolumeError::Geometry)?;
        let fs_info_sector =
            read_u16(&boot_sector, 48).ok_or(DataVolumeError::Geometry)? as u64;
        let backup_boot_sector =
            read_u16(&boot_sector, 50).ok_or(DataVolumeError::Geometry)? as u64;
        let total_sectors = if total_sectors16 != 0 {
            total_sectors16
        } else {
            total_sectors32
        };

        if bytes_per_sector != SECTOR_SIZE
            || sectors_per_cluster == 0
            || !sectors_per_cluster.is_power_of_two()
            || sectors_per_cluster > MAX_SECTORS_PER_CLUSTER
            || reserved_sectors < MIN_RESERVED_SECTORS
            || fat_count != 2
            || fat_sectors == 0
            || fat_sectors > MAX_FAT_SECTORS
            || root_cluster < 2
            || total_sectors == 0
            || total_sectors > max_sectors
            || total_sectors > device.sector_count()
        {
            return Err(DataVolumeError::Geometry);
        }

        let journal_primary_sector = reserved_sectors - JOURNAL_SECTOR_COUNT;
        let journal_secondary_sector = reserved_sectors - 1;
        if fs_info_sector >= reserved_sectors
            || backup_boot_sector >= reserved_sectors
            || [fs_info_sector, backup_boot_sector, backup_boot_sector.saturating_add(1)]
                .contains(&journal_primary_sector)
            || [fs_info_sector, backup_boot_sector, backup_boot_sector.saturating_add(1)]
                .contains(&journal_secondary_sector)
        {
            return Err(DataVolumeError::Geometry);
        }

        let fat_start_sector = reserved_sectors;
        let data_start_sector = fat_start_sector
            .checked_add(fat_count.checked_mul(fat_sectors).ok_or(DataVolumeError::Geometry)?)
            .ok_or(DataVolumeError::Geometry)?;
        if data_start_sector >= total_sectors {
            return Err(DataVolumeError::Geometry);
        }
        let cluster_count = (total_sectors - data_start_sector) / sectors_per_cluster;
        let last_cluster = u32::try_from(
            cluster_count
                .checked_add(1)
                .ok_or(DataVolumeError::Geometry)?,
        )
        .map_err(|_| DataVolumeError::Geometry)?;
        let fat_entries = fat_sectors
            .checked_mul(SECTOR_SIZE as u64)
            .ok_or(DataVolumeError::Geometry)?
            / 4;
        if last_cluster < 2
            || root_cluster > last_cluster
            || last_cluster as u64 >= fat_entries
            || cluster_count == 0
        {
            return Err(DataVolumeError::Geometry);
        }

        let volume = Self {
            device,
            sectors_per_cluster,
            fat_start_sector,
            fat_sectors,
            data_start_sector,
            total_sectors,
            root_cluster,
            last_cluster,
            journal_primary_sector,
            journal_secondary_sector,
        };
        if let Some(transaction) = volume.read_transaction()? {
            volume.validate_transaction(&transaction)?;
        } else {
            volume.validate_fat_copies()?;
        }
        Ok(volume)
    }

    pub fn recover(&self) -> Result<(), DataVolumeError> {
        let Some(transaction) = self.read_transaction()? else {
            return self.validate_fat_copies();
        };
        self.validate_transaction(&transaction)?;

        let entry = self.read_root_slot(transaction.directory_slot)?;
        let published = root_entry_matches(&entry, &transaction);
        if transaction.phase == TransactionPhase::Committed && published {
            self.restore_transaction_allocation(transaction.cluster, FAT_EOC)?;
            self.validate_fat_copies()?;
            self.validate_transaction_data(&transaction)?;
            self.clear_transaction()?;
            return Ok(());
        }

        self.rollback_transaction(&transaction)
    }

    fn rollback_transaction(&self, transaction: &Transaction) -> Result<(), DataVolumeError> {
        let entry = self.read_root_slot(transaction.directory_slot)?;
        if root_entry_matches(&entry, transaction) {
            self.delete_root_slot(transaction.directory_slot)?;
            if !self.device.flush() {
                return Err(DataVolumeError::Device);
            }
        } else if entry[0] != 0x00 && entry[0] != 0xE5 {
            return Err(DataVolumeError::Journal);
        }

        self.restore_transaction_allocation(transaction.cluster, 0)?;
        self.validate_fat_copies()?;
        self.clear_transaction()
    }

    pub fn create_file(
        &self,
        name83: &[u8; 11],
        contents: &[u8],
    ) -> Result<(), DataVolumeError> {
        self.create_file_with_checkpoint(name83, contents, None)
    }

    pub fn create_file_interrupted(
        &self,
        name83: &[u8; 11],
        contents: &[u8],
        checkpoint: u8,
        handler: fn(u8),
    ) -> Result<(), DataVolumeError> {
        if !(1..=8).contains(&checkpoint) {
            return Err(DataVolumeError::Journal);
        }
        self.create_file_with_checkpoint(name83, contents, Some((checkpoint, handler)))
    }

    fn create_file_with_checkpoint(
        &self,
        name83: &[u8; 11],
        contents: &[u8],
        checkpoint: Option<(u8, fn(u8))>,
    ) -> Result<(), DataVolumeError> {
        self.recover()?;
        self.validate_fat_copies()?;
        if !valid_name83(name83) {
            return Err(DataVolumeError::InvalidName);
        }
        if contents.len() > self.max_create_file_bytes() {
            return Err(DataVolumeError::TooLarge);
        }

        let directory_slot = self.find_free_root_slot(name83)?;
        let cluster = self.find_free_cluster()?;
        let transaction = Transaction {
            phase: TransactionPhase::Intent,
            name83: *name83,
            size: u32::try_from(contents.len()).map_err(|_| DataVolumeError::TooLarge)?,
            directory_slot,
            cluster,
            data_checksum: checksum(contents),
        };

        if let Err(error) = self.write_transaction(transaction) {
            return self.abort_failed_create(&transaction, error);
        }
        invoke_checkpoint(checkpoint, 1);
        if let Err(error) = self.write_cluster(cluster, contents) {
            return self.abort_failed_create(&transaction, error);
        }
        if !self.device.flush() {
            return self.abort_failed_create(&transaction, DataVolumeError::Device);
        }
        invoke_checkpoint(checkpoint, 2);
        if let Err(error) = self.write_transaction(transaction.with_phase(TransactionPhase::Data)) {
            return self.abort_failed_create(&transaction, error);
        }
        invoke_checkpoint(checkpoint, 3);

        if let Err(error) = self.write_fat_entry(cluster, FAT_EOC) {
            return self.abort_failed_create(&transaction, error);
        }
        if !self.device.flush() {
            return self.abort_failed_create(&transaction, DataVolumeError::Device);
        }
        invoke_checkpoint(checkpoint, 4);
        if let Err(error) = self.write_transaction(transaction.with_phase(TransactionPhase::Fat)) {
            return self.abort_failed_create(&transaction, error);
        }
        invoke_checkpoint(checkpoint, 5);

        let entry = root_entry(&transaction);
        if let Err(error) = self.write_transaction(transaction.with_phase(TransactionPhase::Publishing)) {
            return self.abort_failed_create(&transaction, error);
        }
        invoke_checkpoint(checkpoint, 6);
        if let Err(error) = self.write_root_slot(directory_slot, &entry) {
            return self.abort_failed_create(&transaction, error);
        }
        if !self.device.flush() {
            return self.abort_failed_create(&transaction, DataVolumeError::Device);
        }
        invoke_checkpoint(checkpoint, 7);
        if let Err(error) = self.write_transaction(transaction.with_phase(TransactionPhase::Committed)) {
            return self.abort_failed_create(&transaction, error);
        }
        invoke_checkpoint(checkpoint, 8);

        let _ = self.clear_transaction();
        Ok(())
    }

    pub fn validate_fat_copies(&self) -> Result<(), DataVolumeError> {
        let mut first = [0u8; SECTOR_SIZE];
        let mut second = [0u8; SECTOR_SIZE];
        for sector in 0..self.fat_sectors {
            if !self
                .device
                .read_sector(self.fat_start_sector + sector, &mut first)
                || !self
                    .device
                    .read_sector(self.fat_start_sector + self.fat_sectors + sector, &mut second)
            {
                return Err(DataVolumeError::Device);
            }
            if first != second {
                return Err(DataVolumeError::FatMismatch);
            }
        }
        Ok(())
    }

    pub fn max_create_file_bytes(&self) -> usize {
        self.cluster_bytes().min(MAX_CREATE_FILE_BYTES)
    }

    pub fn probe_failure_recovery(&self) -> Result<(), DataVolumeError> {
        const CONTENTS: &[u8] = b"CantayaOS data-volume failure recovery probe\n";
        let mut probe_index = 0usize;

        for occurrence in 1..=10 {
            self.probe_failed_create(
                FailurePoint::JournalWrite(occurrence),
                probe_index,
                CONTENTS,
            )?;
            probe_index += 1;
        }
        for occurrence in 1..=2 {
            self.probe_failed_create(
                FailurePoint::FatWrite(occurrence),
                probe_index,
                CONTENTS,
            )?;
            probe_index += 1;
        }
        self.probe_failed_create(FailurePoint::DataWrite(1), probe_index, CONTENTS)?;
        probe_index += 1;
        self.probe_failed_create(FailurePoint::RootWrite(1), probe_index, CONTENTS)?;
        probe_index += 1;
        for occurrence in 1..=8 {
            self.probe_failed_create(FailurePoint::Flush(occurrence), probe_index, CONTENTS)?;
            probe_index += 1;
        }

        let name = *b"RECOVER TXT";
        let volume = Self::open_bounded(self.device, self.total_sectors)?;
        volume.create_file(&name, CONTENTS)?;
        volume.recover()?;
        let reader = LiveFatVolume::open_bounded(self.device, self.total_sectors)
            .ok_or(DataVolumeError::Journal)?;
        let mut readback = Vec::new();
        if !reader.read_file_to_buf_bounded(&name, &mut readback, CONTENTS.len())
            || readback.as_slice() != CONTENTS
        {
            return Err(DataVolumeError::Journal);
        }
        Ok(())
    }

    pub fn probe_corruption_rejection(&self) -> Result<(), DataVolumeError> {
        #[derive(Clone, Copy)]
        enum Corruption {
            FatCopy,
            Journal,
            Directory,
            Data,
        }

        struct CorruptingDevice<'a> {
            base: &'a dyn BlockDevice,
            corruption: Corruption,
            transaction: Option<Transaction>,
            fat_second_sector: u64,
            journal_primary_sector: u64,
            journal_secondary_sector: u64,
            root_sector: u64,
            root_offset: usize,
            data_sector: u64,
        }

        impl BlockDevice for CorruptingDevice<'_> {
            fn sector_count(&self) -> u64 {
                self.base.sector_count()
            }

            fn read_sector(&self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> bool {
                if !self.base.read_sector(sector, output) {
                    return false;
                }
                if sector == self.journal_primary_sector || sector == self.journal_secondary_sector {
                    if let Some(transaction) = self.transaction {
                        *output = transaction.encode();
                    }
                }
                match self.corruption {
                    Corruption::FatCopy if sector == self.fat_second_sector => output[8] ^= 1,
                    Corruption::Journal
                        if sector == self.journal_primary_sector
                            || sector == self.journal_secondary_sector =>
                    {
                        output[0] = 1;
                    }
                    Corruption::Directory if sector == self.root_sector => {
                        output[self.root_offset] = b'X';
                        output[self.root_offset + 11] = ATTR_ARCHIVE;
                    }
                    Corruption::Data if sector == self.data_sector => output[0] ^= 0xFF,
                    _ => {}
                }
                true
            }

            fn write_sector(&self, sector: u64, input: &[u8; SECTOR_SIZE]) -> bool {
                self.base.write_sector(sector, input)
            }

            fn flush(&self) -> bool {
                self.base.flush()
            }
        }

        let name = *b"CHECKSUMTXT";
        let contents = b"CantayaOS data-volume corruption rejection probe\n";
        let directory_slot = self.find_free_root_slot(&name)?;
        let cluster = self.find_free_cluster()?;
        let (root_sector, root_offset) = self.root_slot_location(directory_slot)?;
        let data_sector = self.cluster_to_sector(cluster).ok_or(DataVolumeError::Geometry)?;
        let transaction = Transaction {
            phase: TransactionPhase::Committed,
            name83: name,
            size: u32::try_from(contents.len()).map_err(|_| DataVolumeError::TooLarge)?,
            directory_slot,
            cluster,
            data_checksum: checksum(contents),
        };

        let fat_copy = CorruptingDevice {
            base: self.device,
            corruption: Corruption::FatCopy,
            transaction: None,
            fat_second_sector: self.fat_start_sector + self.fat_sectors,
            journal_primary_sector: self.journal_primary_sector,
            journal_secondary_sector: self.journal_secondary_sector,
            root_sector,
            root_offset,
            data_sector,
        };
        if !matches!(
            DataFatVolume::open_bounded(&fat_copy, self.total_sectors),
            Err(DataVolumeError::FatMismatch)
        ) {
            return Err(DataVolumeError::Journal);
        }

        let journal = CorruptingDevice {
            base: self.device,
            corruption: Corruption::Journal,
            transaction: None,
            fat_second_sector: self.fat_start_sector + self.fat_sectors,
            journal_primary_sector: self.journal_primary_sector,
            journal_secondary_sector: self.journal_secondary_sector,
            root_sector,
            root_offset,
            data_sector,
        };
        if !matches!(
            DataFatVolume::open_bounded(&journal, self.total_sectors),
            Err(DataVolumeError::Journal)
        ) {
            return Err(DataVolumeError::Journal);
        }

        let directory = CorruptingDevice {
            base: self.device,
            corruption: Corruption::Directory,
            transaction: Some(transaction),
            fat_second_sector: self.fat_start_sector + self.fat_sectors,
            journal_primary_sector: self.journal_primary_sector,
            journal_secondary_sector: self.journal_secondary_sector,
            root_sector,
            root_offset,
            data_sector,
        };
        if !matches!(
            DataFatVolume::open_bounded(&directory, self.total_sectors)
                .and_then(|volume| volume.recover()),
            Err(DataVolumeError::Journal)
        ) {
            return Err(DataVolumeError::Journal);
        }

        self.create_file(&name, contents)?;
        let data = CorruptingDevice {
            base: self.device,
            corruption: Corruption::Data,
            transaction: Some(transaction),
            fat_second_sector: self.fat_start_sector + self.fat_sectors,
            journal_primary_sector: self.journal_primary_sector,
            journal_secondary_sector: self.journal_secondary_sector,
            root_sector,
            root_offset,
            data_sector,
        };
        if !matches!(
            DataFatVolume::open_bounded(&data, self.total_sectors).and_then(|volume| volume.recover()),
            Err(DataVolumeError::Journal)
        ) {
            return Err(DataVolumeError::Journal);
        }
        Ok(())
    }

    pub fn probe_capacity_rejection(&self) -> Result<(), DataVolumeError> {
        struct LimitedCapacityDevice<'a> {
            base: &'a dyn BlockDevice,
            fat_start_sector: u64,
            fat_sectors: u64,
            allowed_cluster: u32,
            last_cluster: u32,
        }

        impl LimitedCapacityDevice<'_> {
            fn fat_sector_offset(&self, sector: u64) -> Option<u64> {
                if sector >= self.fat_start_sector && sector < self.fat_start_sector + self.fat_sectors {
                    Some(sector - self.fat_start_sector)
                } else if sector >= self.fat_start_sector + self.fat_sectors
                    && sector < self.fat_start_sector + 2 * self.fat_sectors
                {
                    Some(sector - self.fat_start_sector - self.fat_sectors)
                } else {
                    None
                }
            }
        }

        impl BlockDevice for LimitedCapacityDevice<'_> {
            fn sector_count(&self) -> u64 {
                self.base.sector_count()
            }

            fn read_sector(&self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> bool {
                if !self.base.read_sector(sector, output) {
                    return false;
                }
                let Some(fat_sector_offset) = self.fat_sector_offset(sector) else {
                    return true;
                };
                let first_cluster = fat_sector_offset * (SECTOR_SIZE as u64 / 4);
                let last_cluster = (first_cluster + SECTOR_SIZE as u64 / 4)
                    .min(u64::from(self.last_cluster) + 1);
                for cluster in first_cluster.max(2)..last_cluster {
                    if cluster as u32 == self.allowed_cluster {
                        continue;
                    }
                    let offset = (cluster - first_cluster) as usize * 4;
                    let existing = read_u32(output, offset).unwrap_or(0);
                    output[offset..offset + 4]
                        .copy_from_slice(&((existing & 0xF000_0000) | FAT_EOC).to_le_bytes());
                }
                true
            }

            fn write_sector(&self, sector: u64, input: &[u8; SECTOR_SIZE]) -> bool {
                let Some(fat_sector_offset) = self.fat_sector_offset(sector) else {
                    return self.base.write_sector(sector, input);
                };
                let mut persisted = [0u8; SECTOR_SIZE];
                if !self.base.read_sector(sector, &mut persisted) {
                    return false;
                }
                let first_cluster = fat_sector_offset * (SECTOR_SIZE as u64 / 4);
                let last_cluster = (first_cluster + SECTOR_SIZE as u64 / 4)
                    .min(u64::from(self.last_cluster) + 1);
                if (first_cluster.max(2)..last_cluster).contains(&u64::from(self.allowed_cluster)) {
                    let offset = (u64::from(self.allowed_cluster) - first_cluster) as usize * 4;
                    persisted[offset..offset + 4].copy_from_slice(&input[offset..offset + 4]);
                }
                self.base.write_sector(sector, &persisted)
            }

            fn flush(&self) -> bool {
                self.base.flush()
            }
        }

        const CONTENTS: &[u8] = b"CantayaOS data-volume capacity probe\n";
        let first_name = *b"NOSPACE1TXT";
        let rejected_name = *b"NOSPACE2TXT";
        let allowed_cluster = self.find_free_cluster()?;
        let limited = LimitedCapacityDevice {
            base: self.device,
            fat_start_sector: self.fat_start_sector,
            fat_sectors: self.fat_sectors,
            allowed_cluster,
            last_cluster: self.last_cluster,
        };
        let limited_volume = DataFatVolume::open_bounded(&limited, self.total_sectors)?;
        limited_volume.create_file(&first_name, CONTENTS)?;
        let limited_volume = DataFatVolume::open_bounded(&limited, self.total_sectors)?;
        if !matches!(
            limited_volume.create_file(&rejected_name, CONTENTS),
            Err(DataVolumeError::NoSpace)
        ) {
            return Err(DataVolumeError::Journal);
        }

        if self.root_slot_count() < 2 {
            return Err(DataVolumeError::Geometry);
        }
        let mut last_name = None;
        for index in 0..self.root_slot_count() {
            let name = directory_probe_name(index);
            match self.create_file(&name, CONTENTS) {
                Ok(()) => last_name = Some(name),
                Err(DataVolumeError::DirectoryFull) => break,
                Err(error) => return Err(error),
            }
        }
        let last_name = last_name.ok_or(DataVolumeError::Geometry)?;
        let full_name = *b"DIRFULL TXT";
        if !matches!(
            self.create_file(&full_name, CONTENTS),
            Err(DataVolumeError::DirectoryFull)
        ) {
            return Err(DataVolumeError::Journal);
        }

        self.recover()?;
        self.validate_fat_copies()?;
        let reader = LiveFatVolume::open_bounded(self.device, self.total_sectors)
            .ok_or(DataVolumeError::Journal)?;
        let mut readback = Vec::new();
        if !reader.read_file_to_buf_bounded(&first_name, &mut readback, CONTENTS.len())
            || readback.as_slice() != CONTENTS
        {
            return Err(DataVolumeError::Journal);
        }
        readback.clear();
        if !reader.read_file_to_buf_bounded(&last_name, &mut readback, CONTENTS.len())
            || readback.as_slice() != CONTENTS
            || reader.read_file_to_buf_bounded(&rejected_name, &mut readback, CONTENTS.len())
            || reader.read_file_to_buf_bounded(&full_name, &mut readback, CONTENTS.len())
        {
            return Err(DataVolumeError::Journal);
        }
        Ok(())
    }

    fn cluster_bytes(&self) -> usize {
        self.sectors_per_cluster as usize * SECTOR_SIZE
    }

    fn abort_failed_create(
        &self,
        transaction: &Transaction,
        error: DataVolumeError,
    ) -> Result<(), DataVolumeError> {
        match self.rollback_transaction(transaction) {
            Ok(()) => Err(error),
            Err(recovery_error) => Err(recovery_error),
        }
    }

    fn probe_failed_create(
        &self,
        point: FailurePoint,
        probe_index: usize,
        contents: &[u8],
    ) -> Result<(), DataVolumeError> {
        let root_start_sector = self
            .cluster_to_sector(self.root_cluster)
            .ok_or(DataVolumeError::Geometry)?;
        let faulted = FaultedDevice {
            base: self.device,
            point,
            hits: Cell::new(0),
            journal_primary_sector: self.journal_primary_sector,
            journal_secondary_sector: self.journal_secondary_sector,
            fat_start_sector: self.fat_start_sector,
            data_start_sector: self.data_start_sector,
            root_start_sector,
            root_end_sector: root_start_sector + self.sectors_per_cluster,
        };
        let name = failure_probe_name(probe_index);
        let volume = DataFatVolume::open_bounded(&faulted, self.total_sectors)?;
        if volume.create_file(&name, contents).is_ok() || !faulted.triggered() {
            return Err(DataVolumeError::Journal);
        }

        let recovered = Self::open_bounded(self.device, self.total_sectors)?;
        recovered.recover()?;
        if recovered.find_free_root_slot(&name).is_err() {
            return Err(DataVolumeError::Journal);
        }
        Ok(())
    }

    fn find_free_root_slot(&self, name83: &[u8; 11]) -> Result<u16, DataVolumeError> {
        let mut reusable = None;
        for slot in 0..self.root_slot_count() {
            let entry = self.read_root_slot(slot as u16)?;
            if &entry[..11] == name83
                && entry[0] != 0xE5
                && entry[11] != ATTR_LONG_NAME
                && entry[11] & ATTR_VOLUME_ID == 0
            {
                return Err(DataVolumeError::AlreadyExists);
            }
            if entry[0] == 0x00 {
                return Ok(reusable.unwrap_or(slot) as u16);
            }
            if entry[0] == 0xE5 && reusable.is_none() {
                reusable = Some(slot);
            }
        }
        reusable
            .map(|slot| slot as u16)
            .ok_or(DataVolumeError::DirectoryFull)
    }

    fn find_free_cluster(&self) -> Result<u32, DataVolumeError> {
        for cluster in 2..=self.last_cluster {
            if self.read_fat_entry(cluster)? == 0 {
                return Ok(cluster);
            }
        }
        Err(DataVolumeError::NoSpace)
    }

    fn write_cluster(&self, cluster: u32, contents: &[u8]) -> Result<(), DataVolumeError> {
        let start_sector = self.cluster_to_sector(cluster).ok_or(DataVolumeError::Geometry)?;
        let mut copied = 0usize;
        for sector_offset in 0..self.sectors_per_cluster {
            let mut sector = [0u8; SECTOR_SIZE];
            let remaining = contents.len().saturating_sub(copied);
            let take = remaining.min(SECTOR_SIZE);
            sector[..take].copy_from_slice(&contents[copied..copied + take]);
            if !self.device.write_sector(start_sector + sector_offset, &sector) {
                return Err(DataVolumeError::Device);
            }
            copied += take;
        }
        Ok(())
    }

    fn read_fat_entry(&self, cluster: u32) -> Result<u32, DataVolumeError> {
        let (sector, offset) = self.fat_entry_location(cluster, 0)?;
        let mut bytes = [0u8; SECTOR_SIZE];
        if !self.device.read_sector(sector, &mut bytes) {
            return Err(DataVolumeError::Device);
        }
        read_u32(&bytes, offset)
            .map(|value| value & 0x0FFF_FFFF)
            .ok_or(DataVolumeError::Geometry)
    }

    fn read_fat_entries(&self, cluster: u32) -> Result<[u32; 2], DataVolumeError> {
        let mut entries = [0u32; 2];
        for (copy, entry) in entries.iter_mut().enumerate() {
            let (sector, offset) = self.fat_entry_location(cluster, copy as u64)?;
            let mut bytes = [0u8; SECTOR_SIZE];
            if !self.device.read_sector(sector, &mut bytes) {
                return Err(DataVolumeError::Device);
            }
            *entry = read_u32(&bytes, offset)
                .map(|value| value & 0x0FFF_FFFF)
                .ok_or(DataVolumeError::Geometry)?;
        }
        Ok(entries)
    }

    fn restore_transaction_allocation(
        &self,
        cluster: u32,
        value: u32,
    ) -> Result<(), DataVolumeError> {
        let entries = self.read_fat_entries(cluster)?;
        if entries
            .iter()
            .any(|entry| *entry != 0 && *entry != FAT_EOC)
        {
            return Err(DataVolumeError::Journal);
        }
        if entries != [value; 2] {
            self.write_fat_entry(cluster, value)?;
            if !self.device.flush() {
                return Err(DataVolumeError::Device);
            }
        }
        Ok(())
    }

    fn write_fat_entry(&self, cluster: u32, value: u32) -> Result<(), DataVolumeError> {
        for copy in 0..2 {
            let (sector, offset) = self.fat_entry_location(cluster, copy)?;
            let mut bytes = [0u8; SECTOR_SIZE];
            if !self.device.read_sector(sector, &mut bytes) {
                return Err(DataVolumeError::Device);
            }
            let existing = read_u32(&bytes, offset).ok_or(DataVolumeError::Geometry)?;
            bytes[offset..offset + 4]
                .copy_from_slice(&((existing & 0xF000_0000) | value).to_le_bytes());
            if !self.device.write_sector(sector, &bytes) {
                return Err(DataVolumeError::Device);
            }
        }
        Ok(())
    }

    fn fat_entry_location(
        &self,
        cluster: u32,
        copy: u64,
    ) -> Result<(u64, usize), DataVolumeError> {
        if cluster < 2 || cluster > self.last_cluster || copy >= 2 {
            return Err(DataVolumeError::Geometry);
        }
        let entry_offset = cluster as u64 * 4;
        let sector = self
            .fat_start_sector
            .checked_add(copy * self.fat_sectors)
            .and_then(|base| base.checked_add(entry_offset / SECTOR_SIZE as u64))
            .ok_or(DataVolumeError::Geometry)?;
        if sector >= self.data_start_sector {
            return Err(DataVolumeError::Geometry);
        }
        Ok((sector, (entry_offset % SECTOR_SIZE as u64) as usize))
    }

    fn read_root_slot(&self, slot: u16) -> Result<[u8; 32], DataVolumeError> {
        let (sector, offset) = self.root_slot_location(slot)?;
        let mut bytes = [0u8; SECTOR_SIZE];
        if !self.device.read_sector(sector, &mut bytes) {
            return Err(DataVolumeError::Device);
        }
        let mut entry = [0u8; 32];
        entry.copy_from_slice(&bytes[offset..offset + 32]);
        Ok(entry)
    }

    fn write_root_slot(&self, slot: u16, entry: &[u8; 32]) -> Result<(), DataVolumeError> {
        let (sector, offset) = self.root_slot_location(slot)?;
        let mut bytes = [0u8; SECTOR_SIZE];
        if !self.device.read_sector(sector, &mut bytes) {
            return Err(DataVolumeError::Device);
        }
        bytes[offset..offset + 32].copy_from_slice(entry);
        if !self.device.write_sector(sector, &bytes) {
            return Err(DataVolumeError::Device);
        }
        Ok(())
    }

    fn delete_root_slot(&self, slot: u16) -> Result<(), DataVolumeError> {
        let mut entry = self.read_root_slot(slot)?;
        entry[0] = 0xE5;
        self.write_root_slot(slot, &entry)
    }

    fn root_slot_location(&self, slot: u16) -> Result<(u64, usize), DataVolumeError> {
        let slot = slot as usize;
        if slot >= self.root_slot_count() {
            return Err(DataVolumeError::Journal);
        }
        let byte_offset = slot * 32;
        let root_start = self
            .cluster_to_sector(self.root_cluster)
            .ok_or(DataVolumeError::Geometry)?;
        let sector = root_start
            .checked_add((byte_offset / SECTOR_SIZE) as u64)
            .ok_or(DataVolumeError::Geometry)?;
        if sector >= self.total_sectors {
            return Err(DataVolumeError::Geometry);
        }
        Ok((sector, byte_offset % SECTOR_SIZE))
    }

    fn root_slot_count(&self) -> usize {
        (self.cluster_bytes() / 32).min(MAX_ROOT_DIRECTORY_SLOTS)
    }

    fn write_transaction(&self, transaction: Transaction) -> Result<(), DataVolumeError> {
        let bytes = transaction.encode();
        if !self.device.write_sector(self.journal_primary_sector, &bytes)
            || !self.device.write_sector(self.journal_secondary_sector, &bytes)
            || !self.device.flush()
        {
            return Err(DataVolumeError::Device);
        }
        Ok(())
    }

    fn clear_transaction(&self) -> Result<(), DataVolumeError> {
        let zero = [0u8; SECTOR_SIZE];
        if !self.device.write_sector(self.journal_primary_sector, &zero)
            || !self.device.write_sector(self.journal_secondary_sector, &zero)
            || !self.device.flush()
        {
            return Err(DataVolumeError::Device);
        }
        Ok(())
    }

    fn read_transaction(&self) -> Result<Option<Transaction>, DataVolumeError> {
        let mut primary = [0u8; SECTOR_SIZE];
        let mut secondary = [0u8; SECTOR_SIZE];
        if !self.device.read_sector(self.journal_primary_sector, &mut primary)
            || !self.device.read_sector(self.journal_secondary_sector, &mut secondary)
        {
            return Err(DataVolumeError::Device);
        }
        let primary = Transaction::decode(&primary);
        let secondary = Transaction::decode(&secondary);
        match (primary, secondary) {
            (Ok(None), Ok(None)) => Ok(None),
            (Ok(Some(first)), Ok(Some(second))) => {
                if !first.same_identity(&second) {
                    return Err(DataVolumeError::Journal);
                }
                Ok(Some(if second.phase > first.phase { second } else { first }))
            }
            (Ok(Some(transaction)), _) | (_, Ok(Some(transaction))) => Ok(Some(transaction)),
            _ => Err(DataVolumeError::Journal),
        }
    }

    fn validate_transaction(&self, transaction: &Transaction) -> Result<(), DataVolumeError> {
        if !valid_name83(&transaction.name83)
            || transaction.size as usize > self.max_create_file_bytes()
            || transaction.directory_slot as usize >= self.root_slot_count()
            || transaction.cluster < 2
            || transaction.cluster > self.last_cluster
        {
            return Err(DataVolumeError::Journal);
        }
        Ok(())
    }

    fn validate_transaction_data(&self, transaction: &Transaction) -> Result<(), DataVolumeError> {
        let start_sector = self
            .cluster_to_sector(transaction.cluster)
            .ok_or(DataVolumeError::Journal)?;
        let mut remaining = transaction.size as usize;
        let mut data_checksum = 0x811C_9DC5;
        for sector_offset in 0..self.sectors_per_cluster {
            let mut sector = [0u8; SECTOR_SIZE];
            if !self.device.read_sector(start_sector + sector_offset, &mut sector) {
                return Err(DataVolumeError::Device);
            }
            let take = remaining.min(SECTOR_SIZE);
            data_checksum = checksum_update(data_checksum, &sector[..take]);
            remaining -= take;
            if remaining == 0 {
                break;
            }
        }
        if remaining != 0 || data_checksum != transaction.data_checksum {
            return Err(DataVolumeError::Journal);
        }
        Ok(())
    }

    fn cluster_to_sector(&self, cluster: u32) -> Option<u64> {
        if cluster < 2 || cluster > self.last_cluster {
            return None;
        }
        self.data_start_sector
            .checked_add((cluster as u64 - 2).checked_mul(self.sectors_per_cluster)?)
            .filter(|&sector| sector < self.total_sectors)
    }
}

fn invoke_checkpoint(checkpoint: Option<(u8, fn(u8))>, point: u8) {
    if let Some((target, handler)) = checkpoint {
        if target == point {
            handler(point);
        }
    }
}

#[derive(Clone, Copy)]
enum FailurePoint {
    JournalWrite(usize),
    DataWrite(usize),
    FatWrite(usize),
    RootWrite(usize),
    Flush(usize),
}

impl FailurePoint {
    fn occurrence(self) -> usize {
        match self {
            Self::JournalWrite(occurrence)
            | Self::DataWrite(occurrence)
            | Self::FatWrite(occurrence)
            | Self::RootWrite(occurrence)
            | Self::Flush(occurrence) => occurrence,
        }
    }
}

struct FaultedDevice<'a> {
    base: &'a dyn BlockDevice,
    point: FailurePoint,
    hits: Cell<usize>,
    journal_primary_sector: u64,
    journal_secondary_sector: u64,
    fat_start_sector: u64,
    data_start_sector: u64,
    root_start_sector: u64,
    root_end_sector: u64,
}

impl FaultedDevice<'_> {
    fn triggered(&self) -> bool {
        self.hits.get() >= self.point.occurrence()
    }

    fn should_fail(&self, matches_point: bool) -> bool {
        if !matches_point {
            return false;
        }
        let occurrence = self.hits.get() + 1;
        self.hits.set(occurrence);
        occurrence == self.point.occurrence()
    }

    fn matches_write(&self, sector: u64) -> bool {
        match self.point {
            FailurePoint::JournalWrite(_) => {
                sector == self.journal_primary_sector || sector == self.journal_secondary_sector
            }
            FailurePoint::DataWrite(_) => {
                sector >= self.data_start_sector
                    && (sector < self.root_start_sector || sector >= self.root_end_sector)
            }
            FailurePoint::FatWrite(_) => {
                sector >= self.fat_start_sector && sector < self.data_start_sector
            }
            FailurePoint::RootWrite(_) => {
                sector >= self.root_start_sector && sector < self.root_end_sector
            }
            FailurePoint::Flush(_) => false,
        }
    }
}

impl BlockDevice for FaultedDevice<'_> {
    fn sector_count(&self) -> u64 {
        self.base.sector_count()
    }

    fn read_sector(&self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> bool {
        self.base.read_sector(sector, output)
    }

    fn write_sector(&self, sector: u64, input: &[u8; SECTOR_SIZE]) -> bool {
        if self.should_fail(self.matches_write(sector)) {
            return false;
        }
        self.base.write_sector(sector, input)
    }

    fn flush(&self) -> bool {
        if self.should_fail(matches!(self.point, FailurePoint::Flush(_))) {
            return false;
        }
        self.base.flush()
    }
}

fn failure_probe_name(index: usize) -> [u8; 11] {
    let mut name = *b"FAIL0000TXT";
    let mut value = index;
    for position in (4..8).rev() {
        name[position] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    name
}

fn directory_probe_name(index: usize) -> [u8; 11] {
    let mut name = *b"DIR00000TXT";
    let mut value = index;
    for position in (3..8).rev() {
        name[position] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    name
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum TransactionPhase {
    Intent = 1,
    Data = 2,
    Fat = 3,
    Publishing = 4,
    Committed = 5,
}

impl TransactionPhase {
    fn from_byte(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Intent),
            2 => Some(Self::Data),
            3 => Some(Self::Fat),
            4 => Some(Self::Publishing),
            5 => Some(Self::Committed),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct Transaction {
    phase: TransactionPhase,
    name83: [u8; 11],
    size: u32,
    directory_slot: u16,
    cluster: u32,
    data_checksum: u32,
}

impl Transaction {
    fn with_phase(mut self, phase: TransactionPhase) -> Self {
        self.phase = phase;
        self
    }

    fn encode(self) -> [u8; SECTOR_SIZE] {
        let mut bytes = [0u8; SECTOR_SIZE];
        bytes[..8].copy_from_slice(&JOURNAL_MAGIC);
        bytes[8] = JOURNAL_VERSION;
        bytes[9] = self.phase as u8;
        bytes[10..12].copy_from_slice(&self.directory_slot.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.size.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.cluster.to_le_bytes());
        bytes[20..31].copy_from_slice(&self.name83);
        bytes[TRANSACTION_DATA_CHECKSUM_OFFSET..TRANSACTION_DATA_CHECKSUM_OFFSET + 4]
            .copy_from_slice(&self.data_checksum.to_le_bytes());
        let checksum = journal_checksum(&bytes[..JOURNAL_CHECKSUM_OFFSET]);
        bytes[JOURNAL_CHECKSUM_OFFSET..]
            .copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8; SECTOR_SIZE]) -> Result<Option<Self>, DataVolumeError> {
        if bytes.iter().all(|byte| *byte == 0) {
            return Ok(None);
        }
        if bytes[..8] != JOURNAL_MAGIC[..]
            || bytes[8] != JOURNAL_VERSION
            || read_u32(bytes, JOURNAL_CHECKSUM_OFFSET)
                != Some(journal_checksum(&bytes[..JOURNAL_CHECKSUM_OFFSET]))
        {
            return Err(DataVolumeError::Journal);
        }
        let mut name83 = [0u8; 11];
        name83.copy_from_slice(&bytes[20..31]);
        Ok(Some(Self {
            phase: TransactionPhase::from_byte(bytes[9]).ok_or(DataVolumeError::Journal)?,
            directory_slot: read_u16(bytes, 10).ok_or(DataVolumeError::Journal)?,
            size: read_u32(bytes, 12).ok_or(DataVolumeError::Journal)?,
            cluster: read_u32(bytes, 16).ok_or(DataVolumeError::Journal)?,
            data_checksum: read_u32(bytes, TRANSACTION_DATA_CHECKSUM_OFFSET)
                .ok_or(DataVolumeError::Journal)?,
            name83,
        }))
    }

    fn same_identity(&self, other: &Self) -> bool {
        self.name83 == other.name83
            && self.size == other.size
            && self.directory_slot == other.directory_slot
            && self.cluster == other.cluster
            && self.data_checksum == other.data_checksum
    }
}

fn root_entry(transaction: &Transaction) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[..11].copy_from_slice(&transaction.name83);
    entry[11] = ATTR_ARCHIVE;
    entry[20..22].copy_from_slice(&((transaction.cluster >> 16) as u16).to_le_bytes());
    entry[26..28].copy_from_slice(&(transaction.cluster as u16).to_le_bytes());
    entry[28..32].copy_from_slice(&transaction.size.to_le_bytes());
    entry
}

fn root_entry_matches(entry: &[u8; 32], transaction: &Transaction) -> bool {
    entry[..11] == transaction.name83
        && entry[11] == ATTR_ARCHIVE
        && read_u16(entry, 20).map(u32::from).zip(read_u16(entry, 26).map(u32::from))
            == Some(((transaction.cluster >> 16) & 0xFFFF, transaction.cluster & 0xFFFF))
        && read_u32(entry, 28) == Some(transaction.size)
}

fn valid_name83(name83: &[u8; 11]) -> bool {
    valid_name_component(&name83[..8], true) && valid_name_component(&name83[8..], false)
}

fn valid_name_component(component: &[u8], required: bool) -> bool {
    let mut saw_character = false;
    let mut padding = false;
    for byte in component {
        if *byte == b' ' {
            if saw_character {
                padding = true;
            }
            continue;
        }
        if padding
            || !matches!(*byte, b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-')
        {
            return false;
        }
        saw_character = true;
    }
    !required || saw_character
}

fn checksum(bytes: &[u8]) -> u32 {
    checksum_update(0x811C_9DC5, bytes)
}

fn checksum_update(checksum: u32, bytes: &[u8]) -> u32 {
    bytes.iter().fold(checksum, |checksum, byte| {
        (checksum ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

fn journal_checksum(bytes: &[u8]) -> u32 {
    checksum(bytes)
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