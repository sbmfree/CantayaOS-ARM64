//! ELF64 loader for the initial AArch64 user-mode process.

use super::{process::EProcess, thread::EThread};
use crate::arch::mmu::{PageMapError, PagePermissions, UserAddressSpace, USER_ADDRESS_LIMIT};
use alloc::{sync::Arc, vec::Vec};
use cantaya_shared::BootInfo;
use spin::Mutex;

const PAGE_SIZE: u64 = 4096;
const CACHE_LINE_SIZE: u64 = 64;
const MAX_INIT_ELF_SIZE: usize = 2 * 1024 * 1024;
const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const MAX_PROGRAM_HEADERS: usize = 32;
const EM_AARCH64: u16 = 183;
const ET_EXEC: u16 = 2;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

const INITIAL_USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
const SECOND_INITIAL_USER_STACK_TOP: u64 = INITIAL_USER_STACK_TOP - 0x10_0000;
const INITIAL_USER_STACK_PAGES: usize = 4;
pub const INITIAL_IMAGE_SOURCE: u64 = 0;
pub const FAT_CHILD_IMAGE_SOURCE: u64 = 1;

/// Canonical byte-for-byte copy of the UEFI-provided initialization image.
/// `NtCreateProcess` revalidates this source for every new address space;
/// user mode never receives a direct mapping of retained loader memory.
static INITIAL_USER_IMAGE: Mutex<Option<Vec<u8>>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    MissingImage,
    InvalidBootVolume,
    ImageTooLarge,
    InvalidHeader,
    InvalidProgramHeaders,
    InvalidSegment,
    MissingExecutableEntry,
    Mapping(PageMapError),
    MissingAddressSpace,
}

struct LoadSegment {
    virtual_address: u64,
    memory_size: u64,
    file_offset: usize,
    file_size: u64,
    mapped_start: u64,
    mapped_end: u64,
    permissions: PagePermissions,
}

struct ParsedImage {
    entry: u64,
    image_base: u64,
    segments: Vec<LoadSegment>,
}

/// Parse the UEFI-provided `init.elf` and create two independently mapped
/// processes. Their distinct user stacks make timer-driven EL0 context
/// restoration observable in the smoke workload.
pub fn load_initial_processes(
    boot_info: &BootInfo,
) -> Result<[(Arc<EProcess>, *mut EThread); 2], LoadError> {
    let bytes = unsafe { init_elf_bytes(boot_info)? };
    let image = parse_image(bytes)?;
    *INITIAL_USER_IMAGE.lock() = Some(bytes.to_vec());

    Ok([
        create_user_process(bytes, &image, INITIAL_USER_STACK_TOP, 0, "init ELF")?,
        create_user_process(bytes, &image, SECOND_INITIAL_USER_STACK_TOP, 0, "init ELF")?,
    ])
}

/// Create a fresh process from a bounded kernel-owned image source.
///
/// The caller supplies an initial EL0 `x1` value. Sources are restricted to
/// boot-owned bytes and are revalidated on every address-space creation.
pub fn create_process_from_source(
    source: u64,
    initial_argument: u64,
) -> Result<(Arc<EProcess>, *mut EThread), LoadError> {
    let (bytes, label) = match source {
        INITIAL_IMAGE_SOURCE => (
            INITIAL_USER_IMAGE
                .lock()
                .as_ref()
                .cloned()
                .ok_or(LoadError::MissingImage)?,
            "cached init ELF",
        ),
        FAT_CHILD_IMAGE_SOURCE => (load_fat_child_image()?, "FAT CHILD.ELF"),
        _ => return Err(LoadError::MissingImage),
    };
    let image = parse_image(&bytes)?;
    create_user_process(
        &bytes,
        &image,
        INITIAL_USER_STACK_TOP,
        initial_argument,
        label,
    )
}

fn create_user_process(
    bytes: &[u8],
    image: &ParsedImage,
    stack_top: u64,
    initial_argument: u64,
    label: &str,
) -> Result<(Arc<EProcess>, *mut EThread), LoadError> {
    let process = EProcess::new_user_process(image.image_base);

    let stack = process
        .with_user_address_space(|address_space| {
            map_image(address_space, bytes, &image)?;
            address_space
                .map_user_stack(stack_top, INITIAL_USER_STACK_PAGES)
                .map_err(LoadError::Mapping)
        })
        .ok_or(LoadError::MissingAddressSpace)??;

    let thread = EThread::new_user_with_argument(
        process.clone(),
        image.entry,
        stack.top,
        4,
        initial_argument,
    );
    log::info!(
        "Ps: {} mapped pid={} at {:#x}, entry={:#x}, stack={:#x}, arg={:#x}",
        label,
        process.pid.0,
        process.image_base,
        image.entry,
        stack.top,
        initial_argument,
    );
    Ok((process, thread))
}

fn load_fat_child_image() -> Result<Vec<u8>, LoadError> {
    crate::executive::io::image::FAT_CHILD_FILE
        .read_image()
        .map_err(|error| match error {
            crate::executive::io::image::ImageReadError::InvalidVolume => {
                LoadError::InvalidBootVolume
            }
            crate::executive::io::image::ImageReadError::MissingImage
            | crate::executive::io::image::ImageReadError::DispatchFailed(_)
            | crate::executive::io::image::ImageReadError::InvalidCompletion => {
                LoadError::MissingImage
            }
        })
}

unsafe fn init_elf_bytes(boot_info: &BootInfo) -> Result<&[u8], LoadError> {
    if boot_info.init_elf_phys == 0 || boot_info.init_elf_size == 0 {
        return Err(LoadError::MissingImage);
    }
    let length = usize::try_from(boot_info.init_elf_size).map_err(|_| LoadError::ImageTooLarge)?;
    if length > MAX_INIT_ELF_SIZE {
        return Err(LoadError::ImageTooLarge);
    }
    unsafe {
        Ok(core::slice::from_raw_parts(
            crate::arch::mmu::phys_to_direct_map(boot_info.init_elf_phys) as *const u8,
            length,
        ))
    }
}

fn parse_image(bytes: &[u8]) -> Result<ParsedImage, LoadError> {
    if bytes.len() < ELF_HEADER_SIZE
        || bytes[..4] != [0x7F, b'E', b'L', b'F']
        || bytes[4] != 2
        || bytes[5] != 1
        || bytes[6] != 1
        || read_u16(bytes, 16)? != ET_EXEC
        || read_u16(bytes, 18)? != EM_AARCH64
        || read_u32(bytes, 20)? != 1
        || read_u16(bytes, 52)? as usize != ELF_HEADER_SIZE
        || read_u16(bytes, 54)? as usize != PROGRAM_HEADER_SIZE
    {
        return Err(LoadError::InvalidHeader);
    }

    let entry = read_u64(bytes, 24)?;
    let program_headers_offset =
        usize::try_from(read_u64(bytes, 32)?).map_err(|_| LoadError::InvalidProgramHeaders)?;
    let program_header_count = read_u16(bytes, 56)? as usize;
    if program_header_count == 0 || program_header_count > MAX_PROGRAM_HEADERS {
        return Err(LoadError::InvalidProgramHeaders);
    }
    let program_headers_size = program_header_count
        .checked_mul(PROGRAM_HEADER_SIZE)
        .ok_or(LoadError::InvalidProgramHeaders)?;
    if program_headers_offset
        .checked_add(program_headers_size)
        .filter(|&end| end <= bytes.len())
        .is_none()
    {
        return Err(LoadError::InvalidProgramHeaders);
    }

    let mut segments = Vec::new();
    for index in 0..program_header_count {
        let offset = program_headers_offset + index * PROGRAM_HEADER_SIZE;
        if read_u32(bytes, offset)? != PT_LOAD {
            continue;
        }

        let flags = read_u32(bytes, offset + 4)?;
        let file_offset =
            usize::try_from(read_u64(bytes, offset + 8)?).map_err(|_| LoadError::InvalidSegment)?;
        let virtual_address = read_u64(bytes, offset + 16)?;
        let file_size = read_u64(bytes, offset + 32)?;
        let memory_size = read_u64(bytes, offset + 40)?;
        if memory_size == 0 {
            continue;
        }
        if file_size > memory_size
            || file_offset
                .checked_add(usize::try_from(file_size).map_err(|_| LoadError::InvalidSegment)?)
                .filter(|&end| end <= bytes.len())
                .is_none()
            || virtual_address == 0
            || virtual_address & (PAGE_SIZE - 1) != file_offset as u64 & (PAGE_SIZE - 1)
        {
            return Err(LoadError::InvalidSegment);
        }

        let segment_end = virtual_address
            .checked_add(memory_size)
            .ok_or(LoadError::InvalidSegment)?;
        let mapped_start = virtual_address & !(PAGE_SIZE - 1);
        let mapped_end = page_align_up(segment_end).ok_or(LoadError::InvalidSegment)?;
        if segment_end > USER_ADDRESS_LIMIT || mapped_end > USER_ADDRESS_LIMIT {
            return Err(LoadError::InvalidSegment);
        }
        if segments.iter().any(|segment: &LoadSegment| {
            mapped_start < segment.mapped_end && segment.mapped_start < mapped_end
        }) {
            return Err(LoadError::InvalidSegment);
        }

        let mut permissions = PagePermissions::empty();
        if flags & PF_W != 0 {
            permissions |= PagePermissions::WRITE;
        }
        if flags & PF_X != 0 {
            permissions |= PagePermissions::EXECUTE;
        }
        if flags & PF_W != 0 && flags & PF_X != 0 || flags & PF_R == 0 {
            return Err(LoadError::InvalidSegment);
        }

        segments.push(LoadSegment {
            virtual_address,
            memory_size,
            file_offset,
            file_size,
            mapped_start,
            mapped_end,
            permissions,
        });
    }

    let image_base = segments
        .iter()
        .map(|segment| segment.virtual_address)
        .min()
        .ok_or(LoadError::InvalidProgramHeaders)?;
    let entry_is_executable = segments.iter().any(|segment| {
        segment.permissions.contains(PagePermissions::EXECUTE)
            && entry >= segment.virtual_address
            && entry < segment.virtual_address + segment.memory_size
    });
    if !entry_is_executable {
        return Err(LoadError::MissingExecutableEntry);
    }

    Ok(ParsedImage {
        entry,
        image_base,
        segments,
    })
}

fn map_image(
    address_space: &mut UserAddressSpace,
    bytes: &[u8],
    image: &ParsedImage,
) -> Result<(), LoadError> {
    for segment in &image.segments {
        let mut page = segment.mapped_start;
        while page < segment.mapped_end {
            let phys = address_space
                .allocate_user_page(page, segment.permissions)
                .map_err(LoadError::Mapping)?;
            let mapped_page = crate::arch::mmu::phys_to_direct_map(phys);
            unsafe { core::ptr::write_bytes(mapped_page as *mut u8, 0, PAGE_SIZE as usize) };

            let copy_start = core::cmp::max(page, segment.virtual_address);
            let copy_end = core::cmp::min(
                page + PAGE_SIZE,
                segment.virtual_address + segment.file_size,
            );
            if copy_start < copy_end {
                let source_offset =
                    segment.file_offset + (copy_start - segment.virtual_address) as usize;
                let destination_offset = (copy_start - page) as usize;
                let copy_len = (copy_end - copy_start) as usize;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr().add(source_offset),
                        (mapped_page as *mut u8).add(destination_offset),
                        copy_len,
                    );
                }
            }
            if segment.permissions.contains(PagePermissions::EXECUTE) {
                synchronize_instruction_cache(mapped_page, PAGE_SIZE);
            }
            page += PAGE_SIZE;
        }
    }
    Ok(())
}

fn page_align_up(value: u64) -> Option<u64> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|address| address & !(PAGE_SIZE - 1))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, LoadError> {
    let bytes = bytes
        .get(offset..offset + 2)
        .ok_or(LoadError::InvalidHeader)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, LoadError> {
    let bytes = bytes
        .get(offset..offset + 4)
        .ok_or(LoadError::InvalidHeader)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, LoadError> {
    let bytes = bytes
        .get(offset..offset + 8)
        .ok_or(LoadError::InvalidHeader)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn synchronize_instruction_cache(start: u64, length: u64) {
    let end = start + length;
    let mut address = start & !(CACHE_LINE_SIZE - 1);
    while address < end {
        unsafe {
            core::arch::asm!("dc cvau, {address}", address = in(reg) address, options(nostack));
        }
        address += CACHE_LINE_SIZE;
    }
    unsafe {
        core::arch::asm!("dsb ish", options(nostack));
        core::arch::asm!("ic iallu", options(nostack));
        core::arch::asm!("dsb ish", options(nostack));
        core::arch::asm!("isb", options(nostack));
    }
}
