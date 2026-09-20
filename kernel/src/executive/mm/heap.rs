//! Kernel heap — `linked_list_allocator` over a statically-reserved region.
//!
//! We carve out 4 MiB of BSS space and hand it to the allocator at init time.
//! The global allocator (`LockedHeap`) is declared in `main.rs`.

const HEAP_SIZE: usize = 4 * 1024 * 1024; // 4 MiB

#[repr(align(16))]
struct HeapStorage([u8; HEAP_SIZE]);

static mut HEAP_STORAGE: HeapStorage = HeapStorage([0u8; HEAP_SIZE]);

/// Hand the heap region to the global allocator.
pub fn init() {
    unsafe {
        let heap_start = HEAP_STORAGE.0.as_mut_ptr();
        crate::ALLOCATOR.0.lock().init(heap_start, HEAP_SIZE);
    }
    log::debug!("Kernel heap: {} KiB", HEAP_SIZE / 1024);
}
