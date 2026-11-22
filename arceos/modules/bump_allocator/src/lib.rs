#![no_std]

use allocator::{AllocError, AllocResult, BaseAllocator, ByteAllocator, PageAllocator};
use core::{alloc::Layout, ptr::NonNull};

#[derive(Clone, Copy)]
struct Region {
    start: usize,
    bytes: usize,
    pages: usize,
    end: usize,
}

const EMPTY_REGION: Region = Region {
    start: 0,
    bytes: 0,
    pages: 0,
    end: 0,
};

/// A double-ended bump allocator: bytes grow upwards and pages downwards.
/// Byte storage is reclaimed when all byte allocations have been freed.
/// Pages are permanent, as required by the early allocator.
pub struct EarlyAllocator<const SIZE: usize> {
    regions: [Region; 32],
    region_count: usize,
    allocations: usize,
}

impl<const SIZE: usize> EarlyAllocator<SIZE> {
    pub const fn new() -> Self {
        Self {
            regions: [EMPTY_REGION; 32],
            region_count: 0,
            allocations: 0,
        }
    }

    fn regions(&self) -> &[Region] {
        &self.regions[..self.region_count]
    }
}

impl<const SIZE: usize> BaseAllocator for EarlyAllocator<SIZE> {
    fn init(&mut self, start: usize, size: usize) {
        *self = Self::new();
        self.add_memory(start, size)
            .expect("invalid allocator memory region");
    }

    fn add_memory(&mut self, start: usize, size: usize) -> AllocResult {
        if !SIZE.is_power_of_two() || size == 0 || start == 0 {
            return Err(AllocError::InvalidParam);
        }
        let end = start.checked_add(size).ok_or(AllocError::InvalidParam)?;
        if self
            .regions()
            .iter()
            .any(|r| start < r.end && r.start < end)
        {
            return Err(AllocError::MemoryOverlap);
        }
        if self.region_count == self.regions.len() {
            return Err(AllocError::NoMemory);
        }
        self.regions[self.region_count] = Region {
            start,
            bytes: start,
            pages: end,
            end,
        };
        self.region_count += 1;
        Ok(())
    }
}

impl<const SIZE: usize> ByteAllocator for EarlyAllocator<SIZE> {
    fn alloc(&mut self, layout: Layout) -> AllocResult<NonNull<u8>> {
        for r in &mut self.regions[..self.region_count] {
            let Some(pos) = r
                .bytes
                .checked_add(layout.align() - 1)
                .map(|n| n & !(layout.align() - 1))
            else {
                continue;
            };
            let Some(end) = pos.checked_add(layout.size().max(1)) else {
                continue;
            };
            if end <= r.pages {
                r.bytes = end;
                self.allocations += 1;
                return NonNull::new(pos as *mut u8).ok_or(AllocError::InvalidParam);
            }
        }
        Err(AllocError::NoMemory)
    }

    fn dealloc(&mut self, _pos: NonNull<u8>, _layout: Layout) {
        assert!(self.allocations > 0, "unbalanced byte deallocation");
        self.allocations -= 1;
        if self.allocations == 0 {
            for r in &mut self.regions[..self.region_count] {
                r.bytes = r.start;
            }
        }
    }

    fn total_bytes(&self) -> usize {
        self.regions().iter().map(|r| r.end - r.start).sum()
    }

    fn used_bytes(&self) -> usize {
        self.regions().iter().map(|r| r.bytes - r.start).sum()
    }

    fn available_bytes(&self) -> usize {
        self.regions().iter().map(|r| r.pages - r.bytes).sum()
    }
}

impl<const SIZE: usize> PageAllocator for EarlyAllocator<SIZE> {
    const PAGE_SIZE: usize = SIZE;

    fn alloc_pages(&mut self, num_pages: usize, align_pow2: usize) -> AllocResult<usize> {
        if num_pages == 0 || !align_pow2.is_power_of_two() {
            return Err(AllocError::InvalidParam);
        }
        let size = num_pages
            .checked_mul(SIZE)
            .ok_or(AllocError::InvalidParam)?;
        let align = align_pow2.max(SIZE);
        for r in &mut self.regions[..self.region_count] {
            let Some(pos) = r.pages.checked_sub(size).map(|n| n & !(align - 1)) else {
                continue;
            };
            if pos >= r.bytes {
                r.pages = pos;
                return Ok(pos);
            }
        }
        Err(AllocError::NoMemory)
    }

    fn dealloc_pages(&mut self, _pos: usize, _num_pages: usize) {
        // Early page allocations remain reserved for the lifetime of the kernel.
    }

    fn total_pages(&self) -> usize {
        self.regions()
            .iter()
            .map(|r| (r.end / SIZE).saturating_sub(r.start.div_ceil(SIZE)))
            .sum()
    }

    fn used_pages(&self) -> usize {
        self.regions()
            .iter()
            .map(|r| r.end / SIZE - r.pages / SIZE)
            .sum()
    }

    fn available_pages(&self) -> usize {
        self.regions()
            .iter()
            .map(|r| (r.pages / SIZE).saturating_sub(r.bytes.div_ceil(SIZE)))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_and_page_allocations_do_not_overlap() {
        let mut allocator = EarlyAllocator::<4096>::new();
        allocator.init(0x1003, 0xaffd);
        let layout = Layout::from_size_align(100, 64).unwrap();
        let bytes = allocator.alloc(layout).unwrap();
        assert_eq!(bytes.as_ptr() as usize % 64, 0);
        let pages = allocator.alloc_pages(2, 8192).unwrap();
        assert_eq!(pages % 8192, 0);
        assert!(bytes.as_ptr() as usize + layout.size() <= pages);
        assert!(matches!(
            allocator.alloc_pages(100, 4096),
            Err(AllocError::NoMemory)
        ));
        assert!(matches!(
            allocator.alloc_pages(1, 3),
            Err(AllocError::InvalidParam)
        ));
        let used_pages = allocator.used_pages();
        allocator.dealloc(bytes, layout);
        assert_eq!(allocator.used_bytes(), 0);
        allocator.dealloc_pages(pages, 2);
        assert_eq!(allocator.used_pages(), used_pages);
    }

    #[test]
    fn byte_storage_resets_only_after_last_deallocation() {
        let mut allocator = EarlyAllocator::<4096>::new();
        allocator.init(0x1000, 0x2000);
        let layout = Layout::from_size_align(4096, 4096).unwrap();
        let first = allocator.alloc(layout).unwrap();
        let second = allocator.alloc(layout).unwrap();
        allocator.dealloc(first, layout);
        assert!(matches!(allocator.alloc(layout), Err(AllocError::NoMemory)));
        allocator.dealloc(second, layout);
        assert_eq!(allocator.alloc(layout).unwrap(), first);
    }

    #[test]
    fn additional_regions_and_overflow_are_checked() {
        let mut allocator = EarlyAllocator::<4096>::new();
        allocator.init(0x1000, 4096);
        assert!(matches!(
            allocator.add_memory(0x1800, 4096),
            Err(AllocError::MemoryOverlap)
        ));
        assert!(matches!(
            allocator.add_memory(usize::MAX - 1, 4096),
            Err(AllocError::InvalidParam)
        ));
        allocator.add_memory(0x4000, 4096).unwrap();
        assert_eq!(allocator.total_pages(), 2);
        assert_eq!(allocator.alloc_pages(1, 4096).unwrap(), 0x1000);
        assert_eq!(allocator.alloc_pages(1, 4096).unwrap(), 0x4000);
        assert_eq!(allocator.available_bytes(), 0);
    }
}
