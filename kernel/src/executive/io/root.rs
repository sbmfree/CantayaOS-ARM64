//! Bounded, read-only user file access to root and one 8.3 subdirectory.

use alloc::{sync::Arc, vec::Vec};
use spin::Mutex;

use crate::{
    drivers::{
        fat::{LiveFatVolume, RootFileEntry},
        virtio_blk::{BOOT_DISK, MAX_BOOT_VOLUME_SECTORS},
    },
    executive::ke::spinlock::IrqState,
};

const MAX_ROOT_FILE_SIZE: usize = 2 * 1024 * 1024;
pub const MAX_PATH_LENGTH: usize = 25;

pub struct ParsedPath {
    pub directory: Option<[u8; 11]>,
    pub file: [u8; 11],
}

pub struct OpenFile {
    bytes: Vec<u8>,
    offset: Mutex<usize>,
}

impl OpenFile {
    pub fn read(&self, output: &mut [u8]) -> usize {
        let irq_state = IrqState::disable();
        let mut offset = self.offset.lock();
        let count = output.len().min(self.bytes.len().saturating_sub(*offset));
        output[..count].copy_from_slice(&self.bytes[*offset..*offset + count]);
        *offset += count;
        drop(offset);
        irq_state.restore();
        count
    }
}

pub fn parse_name(name: &[u8]) -> Option<[u8; 11]> {
    if name.is_empty() || name.len() > 12 {
        return None;
    }
    let dot = name
        .iter()
        .position(|&byte| byte == b'.')
        .unwrap_or(name.len());
    let stem = &name[..dot];
    let extension = if dot == name.len() {
        &[][..]
    } else {
        &name[dot + 1..]
    };
    if stem.is_empty()
        || stem.len() > 8
        || extension.len() > 3
        || (dot != name.len() && extension.is_empty())
    {
        return None;
    }
    let mut name83 = [b' '; 11];
    let (stem_target, extension_target) = name83.split_at_mut(8);
    for (source, target) in stem
        .iter()
        .zip(stem_target.iter_mut())
        .chain(extension.iter().zip(extension_target.iter_mut()))
    {
        let upper = source.to_ascii_uppercase();
        if !upper.is_ascii_alphanumeric() && !matches!(upper, b'_' | b'-') {
            return None;
        }
        *target = upper;
    }
    Some(name83)
}

pub fn parse_path(path: &[u8]) -> Option<ParsedPath> {
    if path.is_empty() || path.len() > MAX_PATH_LENGTH {
        return None;
    }
    if let Some(slash) = path.iter().position(|&byte| byte == b'/') {
        Some(ParsedPath {
            directory: Some(parse_name(&path[..slash])?),
            file: parse_name(&path[slash + 1..])?,
        })
    } else {
        Some(ParsedPath {
            directory: None,
            file: parse_name(path)?,
        })
    }
}

pub fn open_path(path: &ParsedPath) -> Option<Arc<OpenFile>> {
    let bytes = read_full_path(path)?;
    Some(Arc::new(OpenFile {
        bytes,
        offset: Mutex::new(0),
    }))
}

pub fn read_full_path(path: &ParsedPath) -> Option<Vec<u8>> {
    let volume = LiveFatVolume::open_bounded(&BOOT_DISK, MAX_BOOT_VOLUME_SECTORS)?;
    let mut bytes = Vec::new();
    let loaded = if let Some(directory) = path.directory.as_ref() {
        let cluster = volume.find_directory(directory)?;
        volume.read_file_from_directory_to_buf_bounded(
            cluster,
            &path.file,
            &mut bytes,
            MAX_ROOT_FILE_SIZE,
        )
    } else {
        volume.read_file_to_buf_bounded(&path.file, &mut bytes, MAX_ROOT_FILE_SIZE)
    };
    loaded.then_some(bytes)
}

pub fn root_file_at(index: usize) -> Option<RootFileEntry> {
    let volume = LiveFatVolume::open_bounded(&BOOT_DISK, MAX_BOOT_VOLUME_SECTORS)?;
    volume.root_file_at(index)
}

pub fn directory_file_at(directory: &[u8; 11], index: usize) -> Option<RootFileEntry> {
    let volume = LiveFatVolume::open_bounded(&BOOT_DISK, MAX_BOOT_VOLUME_SECTORS)?;
    let cluster = volume.find_directory(directory)?;
    volume.directory_entry_at(cluster, index)
}

pub fn directory_exists(directory: &[u8; 11]) -> bool {
    LiveFatVolume::open_bounded(&BOOT_DISK, MAX_BOOT_VOLUME_SECTORS)
        .and_then(|volume| volume.find_directory(directory))
        .is_some()
}

pub fn encode_entry(entry: RootFileEntry) -> [u8; 16] {
    let mut encoded = [0u8; 16];
    let stem_end = entry.name83[..8]
        .iter()
        .position(|&byte| byte == b' ')
        .unwrap_or(8);
    encoded[..stem_end].copy_from_slice(&entry.name83[..stem_end]);
    let extension_end = entry.name83[8..]
        .iter()
        .position(|&byte| byte == b' ')
        .unwrap_or(3);
    let mut length = stem_end;
    if extension_end != 0 {
        encoded[length] = b'.';
        length += 1;
        encoded[length..length + extension_end]
            .copy_from_slice(&entry.name83[8..8 + extension_end]);
    }
    encoded[12..].copy_from_slice(
        &(if entry.is_directory {
            u32::MAX
        } else {
            entry.size
        })
        .to_le_bytes(),
    );
    encoded
}
