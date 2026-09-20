//! Minimal ELF64 loader.
//!
//! Manual ELF64 parser — avoids the goblin high-level `Elf` struct which
//! requires `std`.  We parse only what we need: ELF header + PT_LOAD phdrs.

use uefi::boot::{AllocateType, MemoryType as UefiTy};

// ELF64 magic + type constants
const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const PT_LOAD: u32 = 1;

/// ELF64 file header (64 bytes).
#[repr(C, packed)]
struct Elf64Hdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

/// ELF64 program header (56 bytes).
#[repr(C, packed)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

/// Load the ELF `bytes` into memory.  Returns `(entry_point_vaddr, lowest_phys_addr)`.
pub fn load_elf(bytes: &[u8]) -> Option<(u64, u64)> {
    if bytes.len() < core::mem::size_of::<Elf64Hdr>() {
        return None;
    }

    // SAFETY: checked length; 1-byte aligned pointer cast.
    let hdr = unsafe { &*(bytes.as_ptr() as *const Elf64Hdr) };

    if hdr.e_ident[..4] != ELFMAG {
        return None;
    }
    if hdr.e_ident[4] != ELFCLASS64 {
        return None;
    }

    let phoff = { hdr.e_phoff } as usize;
    let phentsize = { hdr.e_phentsize } as usize;
    let phnum = { hdr.e_phnum } as usize;
    let entry = { hdr.e_entry };

    let mut lowest_phys: u64 = u64::MAX;

    for i in 0..phnum {
        let off = phoff + i * phentsize;
        if off + core::mem::size_of::<Elf64Phdr>() > bytes.len() {
            break;
        }
        let ph = unsafe { &*(bytes[off..].as_ptr() as *const Elf64Phdr) };

        let p_type = { ph.p_type };
        let p_paddr = { ph.p_paddr };
        let p_filesz = { ph.p_filesz } as usize;
        let p_memsz = { ph.p_memsz } as usize;
        let p_offset = { ph.p_offset } as usize;

        if p_type != PT_LOAD || p_memsz == 0 {
            continue;
        }

        let pages = (p_memsz + 4095) / 4096;

        uefi::boot::allocate_pages(AllocateType::Address(p_paddr), UefiTy::LOADER_DATA, pages)
            .expect("Failed to allocate pages for ELF segment");

        // SAFETY: pages just allocated by UEFI firmware.
        let dest = unsafe { core::slice::from_raw_parts_mut(p_paddr as *mut u8, p_memsz) };
        let copy_len = p_filesz.min(p_memsz);
        if p_offset + copy_len <= bytes.len() {
            dest[..copy_len].copy_from_slice(&bytes[p_offset..p_offset + copy_len]);
        }
        dest[copy_len..].fill(0);

        if p_paddr < lowest_phys {
            lowest_phys = p_paddr;
        }
    }

    if lowest_phys == u64::MAX {
        return None;
    }
    Some((entry, lowest_phys))
}
