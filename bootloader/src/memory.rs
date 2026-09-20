//! Convert a UEFI memory map into our `BootInfo` memory map.

use uefi::boot::MemoryType as UefiTy;
use uefi::mem::memory_map::{MemoryMap as UefiMap, MemoryMapOwned};

use cantaya_shared::{MemoryDescriptor, MemoryMap, MemoryType, MAX_MEMORY_DESCRIPTORS};

pub fn convert_map(uefi_map: &MemoryMapOwned) -> MemoryMap {
    let mut out = MemoryMap::new();

    for desc in uefi_map.entries() {
        if out.count >= MAX_MEMORY_DESCRIPTORS {
            break;
        }
        let ty = match desc.ty {
            UefiTy::RESERVED => MemoryType::Reserved,
            UefiTy::LOADER_CODE => MemoryType::LoaderCode,
            UefiTy::LOADER_DATA => MemoryType::LoaderData,
            UefiTy::BOOT_SERVICES_CODE => MemoryType::BootServicesCode,
            UefiTy::BOOT_SERVICES_DATA => MemoryType::BootServicesData,
            UefiTy::RUNTIME_SERVICES_CODE => MemoryType::RuntimeServicesCode,
            UefiTy::RUNTIME_SERVICES_DATA => MemoryType::RuntimeServicesData,
            UefiTy::CONVENTIONAL => MemoryType::Conventional,
            UefiTy::UNUSABLE => MemoryType::Unusual,
            UefiTy::ACPI_RECLAIM => MemoryType::AcpiReclaim,
            UefiTy::ACPI_NON_VOLATILE => MemoryType::AcpiNvs,
            UefiTy::MMIO => MemoryType::MemoryMappedIo,
            UefiTy::MMIO_PORT_SPACE => MemoryType::MemoryMappedIoPortSpace,
            UefiTy::PAL_CODE => MemoryType::PalCode,
            UefiTy::PERSISTENT_MEMORY => MemoryType::PersistentMemory,
            _ => MemoryType::Reserved,
        };

        out.entries[out.count] = MemoryDescriptor {
            ty,
            phys_start: desc.phys_start,
            page_count: desc.page_count,
            attributes: desc.att.bits(),
        };
        out.count += 1;
    }

    out
}
