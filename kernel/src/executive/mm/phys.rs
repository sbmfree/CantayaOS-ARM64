//! Physical page frame allocator.
//!
//! Uses a bitmap where each bit represents one 4 KiB page.
//! 1 = free, 0 = allocated / reserved.
//!
//! Supports up to 16 GiB of physical RAM (4M pages × 4 KiB).

use cantaya_shared::{MemoryMap, MemoryType};
use spin::Mutex;

const PAGE_SIZE: usize = 4096;
const MAX_PAGES: usize = 4 * 1024 * 1024; // 16 GiB

/// Bitmap: 1 bit per page; stored as u64 words.
const WORDS: usize = MAX_PAGES / 64;
static mut BITMAP: [u64; WORDS] = [0u64; WORDS]; // all reserved initially

struct PhysAllocator {
    total_pages: usize,
    free_pages: usize,
    /// Next word index to search for free pages (roving pointer).
    next_word: usize,
}

static PHYS: Mutex<PhysAllocator> = Mutex::new(PhysAllocator {
    total_pages: 0,
    free_pages: 0,
    next_word: 0,
});

/// Initialise the allocator from the BootInfo memory map.
pub fn init(map: &MemoryMap) {
    let mut alloc = PHYS.lock();

    for desc in map.as_slice() {
        if desc.ty == MemoryType::Conventional {
            let start_page = (desc.phys_start / PAGE_SIZE as u64) as usize;
            let count = desc.page_count as usize;

            for page in start_page..start_page + count {
                if page < MAX_PAGES {
                    unsafe { set_free(page) };
                    alloc.free_pages += 1;
                    alloc.total_pages += 1;
                }
            }
        }
    }
}

/// Allocate one physical page.  Returns its physical address, or panics.
pub fn alloc_page() -> u64 {
    alloc_contiguous_pages(1)
}

/// Allocate `page_count` physically contiguous 4 KiB pages.
///
/// The returned address is the base of the range.  This is required for
/// consumers such as kernel stacks which use one linear address range while
/// paging is disabled.  The allocator searches from its roving cursor to
/// avoid repeatedly scanning the same low-memory pages.
pub fn alloc_contiguous_pages(page_count: usize) -> u64 {
    assert!(page_count > 0, "alloc_contiguous_pages: zero-page request");

    let mut alloc = PHYS.lock();
    assert!(
        page_count <= alloc.free_pages,
        "Out of physical memory: requested {page_count} pages, {} free",
        alloc.free_pages
    );

    let start_page = alloc.next_word * 64;
    for offset in 0..MAX_PAGES {
        let page = (start_page + offset) % MAX_PAGES;
        if page + page_count > MAX_PAGES || !unsafe { range_is_free(page, page_count) } {
            continue;
        }

        for allocated_page in page..page + page_count {
            unsafe { set_used(allocated_page) };
        }
        alloc.free_pages -= page_count;
        alloc.next_word = (page + page_count) / 64 % WORDS;
        return (page * PAGE_SIZE) as u64;
    }

    panic!(
        "Phys alloc: no contiguous run of {page_count} pages despite {} free pages",
        alloc.free_pages
    );
}

/// Free a physical page previously returned by [`alloc_page`].
pub fn free_page(phys: u64) {
    free_contiguous_pages(phys, 1);
}

/// Return a physically contiguous range previously allocated by
/// [`alloc_contiguous_pages`].
pub fn free_contiguous_pages(phys: u64, page_count: usize) {
    assert!(page_count > 0, "free_contiguous_pages: zero-page request");
    assert_eq!(
        phys % PAGE_SIZE as u64,
        0,
        "free_page: address is not page aligned"
    );

    let page = (phys / PAGE_SIZE as u64) as usize;
    assert!(
        page <= MAX_PAGES.saturating_sub(page_count),
        "free_page: address range out of range"
    );

    let mut alloc = PHYS.lock();
    for freed_page in page..page + page_count {
        assert!(!unsafe { is_free(freed_page) }, "free_page: double free");
    }
    for freed_page in page..page + page_count {
        unsafe { set_free(freed_page) };
    }
    alloc.free_pages += page_count;
}

/// Number of currently free pages.
pub fn free_pages() -> usize {
    PHYS.lock().free_pages
}

// ─────────────────────────────────────────────────────────────────────────────
// Bitmap helpers
// ─────────────────────────────────────────────────────────────────────────────

unsafe fn set_free(page: usize) {
    let word = unsafe { bitmap_word_mut(page / 64) };
    unsafe { *word |= 1 << (page % 64) };
}

unsafe fn set_used(page: usize) {
    let word = unsafe { bitmap_word_mut(page / 64) };
    unsafe { *word &= !(1 << (page % 64)) };
}

unsafe fn range_is_free(start_page: usize, page_count: usize) -> bool {
    (start_page..start_page + page_count).all(|page| unsafe { is_free(page) })
}

unsafe fn is_free(page: usize) -> bool {
    let word = unsafe { bitmap_word(page / 64) };
    unsafe { *word & (1 << (page % 64)) != 0 }
}

/// Get a raw pointer to a bitmap word without creating a reference to the
/// mutable static.  Callers hold `PHYS`, which serialises all bitmap access.
unsafe fn bitmap_word_mut(index: usize) -> *mut u64 {
    unsafe { core::ptr::addr_of_mut!(BITMAP).cast::<u64>().add(index) }
}

unsafe fn bitmap_word(index: usize) -> *const u64 {
    unsafe { core::ptr::addr_of!(BITMAP).cast::<u64>().add(index) }
}
