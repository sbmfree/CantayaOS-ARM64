//! Mm::Virt — kernel virtual address allocator.
//!
//! Plain reservations remain available for incomplete user-mode syscall stubs.
//! The dedicated kernel API reserves a higher-half range, allocates physical
//! backing, and installs TTBR1 mappings through `arch::mmu`.

use crate::arch::mmu::{self, PagePermissions};
use crate::executive::mm::phys;
use spin::Mutex;

const PAGE_SIZE: u64 = 4096;
const KERNEL_VIRT_START: u64 = mmu::KERNEL_DYNAMIC_BASE;

/// Fixed tables avoid requiring a heap while memory management is initialised.
/// They can be replaced by a VAD tree when process address spaces are added.
const MAX_FREE_RANGES: usize = 64;
const MAX_ALLOCATIONS: usize = 128;

#[derive(Clone, Copy)]
struct Range {
    start: u64,
    size: u64,
}

impl Range {
    const EMPTY: Self = Self { start: 0, size: 0 };

    fn end(self) -> u64 {
        self.start + self.size
    }
}

struct VirtAllocator {
    next: u64,
    free_ranges: [Range; MAX_FREE_RANGES],
    free_count: usize,
    allocations: [Range; MAX_ALLOCATIONS],
}

impl VirtAllocator {
    const fn new() -> Self {
        Self {
            next: KERNEL_VIRT_START,
            free_ranges: [Range::EMPTY; MAX_FREE_RANGES],
            free_count: 0,
            allocations: [Range::EMPTY; MAX_ALLOCATIONS],
        }
    }

    fn reserve(&mut self, size: u64) -> Option<u64> {
        let allocation_slot = self.allocations.iter().position(|range| range.size == 0)?;

        let start = if let Some(index) = self.free_ranges[..self.free_count]
            .iter()
            .position(|range| range.size >= size)
        {
            let range = &mut self.free_ranges[index];
            let start = range.start;
            if range.size == size {
                self.free_ranges
                    .copy_within(index + 1..self.free_count, index);
                self.free_count -= 1;
                self.free_ranges[self.free_count] = Range::EMPTY;
            } else {
                range.start += size;
                range.size -= size;
            }
            start
        } else {
            let start = self.next;
            self.next = self.next.checked_add(size)?;
            start
        };

        self.allocations[allocation_slot] = Range { start, size };
        Some(start)
    }

    fn release(&mut self, start: u64, size: u64) -> bool {
        let Some(allocation_slot) = self
            .allocations
            .iter()
            .position(|range| range.start == start && range.size == size)
        else {
            return false;
        };

        if !self.insert_free_range(Range { start, size }) {
            return false;
        }
        self.allocations[allocation_slot] = Range::EMPTY;
        true
    }

    fn contains_allocation(&self, start: u64, size: u64) -> bool {
        self.allocations
            .iter()
            .any(|range| range.start == start && range.size == size)
    }

    /// Insert a range into the sorted free list and coalesce adjacent ranges.
    fn insert_free_range(&mut self, range: Range) -> bool {
        let insert_at = self.free_ranges[..self.free_count]
            .iter()
            .position(|existing| existing.start > range.start)
            .unwrap_or(self.free_count);

        if insert_at > 0 {
            let previous = self.free_ranges[insert_at - 1];
            if previous.end() == range.start {
                self.free_ranges[insert_at - 1].size += range.size;
                self.merge_next(insert_at - 1);
                return true;
            }
        }

        if insert_at < self.free_count && range.end() == self.free_ranges[insert_at].start {
            self.free_ranges[insert_at].start = range.start;
            self.free_ranges[insert_at].size += range.size;
            return true;
        }

        if self.free_count == MAX_FREE_RANGES {
            return false;
        }
        self.free_ranges
            .copy_within(insert_at..self.free_count, insert_at + 1);
        self.free_ranges[insert_at] = range;
        self.free_count += 1;
        true
    }

    fn merge_next(&mut self, index: usize) {
        if index + 1 >= self.free_count
            || self.free_ranges[index].end() != self.free_ranges[index + 1].start
        {
            return;
        }

        self.free_ranges[index].size += self.free_ranges[index + 1].size;
        self.free_ranges
            .copy_within(index + 2..self.free_count, index + 1);
        self.free_count -= 1;
        self.free_ranges[self.free_count] = Range::EMPTY;
    }
}

static ALLOCATOR: Mutex<VirtAllocator> = Mutex::new(VirtAllocator::new());

fn page_aligned_size(size: usize) -> Option<u64> {
    let size = size as u64;
    if size == 0 {
        return None;
    }
    size.checked_add(PAGE_SIZE - 1)
        .map(|value| value / PAGE_SIZE * PAGE_SIZE)
}

/// Reserve `size` bytes of kernel virtual address space, rounded up to pages.
///
/// The returned range has no physical backing until page-table support maps
/// it.  `None` indicates an invalid size or exhausted bookkeeping capacity.
pub fn mm_allocate_virtual(size: usize) -> Option<u64> {
    let size = page_aligned_size(size)?;
    ALLOCATOR.lock().reserve(size)
}

/// Reserve, physically back, and map kernel virtual memory.
///
/// This is intentionally distinct from [`mm_allocate_virtual`], whose current
/// callers include incomplete user-mode syscall stubs.  User mappings will be
/// backed by their owning process's TTBR0 in the next milestone.
pub fn mm_allocate_kernel_virtual(size: usize) -> Option<u64> {
    let mapped_size = page_aligned_size(size)?;
    let virt = mm_allocate_virtual(size)?;
    let mut offset = 0;

    while offset < mapped_size {
        let phys = phys::alloc_page();
        let result = unsafe {
            mmu::map_kernel_page(
                virt + offset,
                phys,
                PagePermissions::WRITE,
                phys::alloc_page,
            )
        };
        if result.is_err() {
            phys::free_page(phys);
            while offset > 0 {
                offset -= PAGE_SIZE;
                if let Ok(mapped_phys) = unsafe { mmu::unmap_kernel_page(virt + offset) } {
                    phys::free_page(mapped_phys);
                }
            }
            let _ = mm_free_virtual(virt, size);
            return None;
        }

        unsafe { core::ptr::write_bytes((virt + offset) as *mut u8, 0, PAGE_SIZE as usize) };
        offset += PAGE_SIZE;
    }

    Some(virt)
}

/// Release a range previously returned by [`mm_allocate_virtual`].
///
/// The address and rounded size must exactly match the original allocation.
/// Returning `false` prevents accidental double frees and overlapping ranges.
pub fn mm_free_virtual(virt: u64, size: usize) -> bool {
    if virt % PAGE_SIZE != 0 {
        return false;
    }
    let Some(size) = page_aligned_size(size) else {
        return false;
    };
    ALLOCATOR.lock().release(virt, size)
}

/// Unmap and release memory allocated by [`mm_allocate_kernel_virtual`].
pub fn mm_free_kernel_virtual(virt: u64, size: usize) -> bool {
    if virt % PAGE_SIZE != 0 {
        return false;
    }
    let Some(mapped_size) = page_aligned_size(size) else {
        return false;
    };
    if !ALLOCATOR.lock().contains_allocation(virt, mapped_size) {
        return false;
    }

    let mut offset = 0;
    while offset < mapped_size {
        let Ok(phys) = (unsafe { mmu::unmap_kernel_page(virt + offset) }) else {
            return false;
        };
        phys::free_page(phys);
        offset += PAGE_SIZE;
    }

    ALLOCATOR.lock().release(virt, mapped_size)
}
