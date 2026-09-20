//! Mm — Memory Manager.
//!
//! Sub-modules:
//!   phys — physical page frame allocator (bitmap)
//!   heap — kernel heap (`linked_list_allocator`)
//!   virt — early virtual-address range allocator (VAD-tree precursor)

pub mod heap;
pub mod phys;
pub mod virt;

use cantaya_shared::BootInfo;

/// Initialise all memory management subsystems.
///
/// Must be called **after** the MMU is on so the heap virtual mapping exists.
pub fn init(boot_info: &BootInfo) {
    phys::init(&boot_info.memory_map);
    heap::init();
    log::info!(
        "Mm: {} MiB free physical RAM",
        phys::free_pages() * 4 / 1024
    );
}
