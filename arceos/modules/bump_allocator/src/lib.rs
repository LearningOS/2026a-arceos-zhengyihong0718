#![no_std]

use allocator::{BaseAllocator, ByteAllocator, PageAllocator};

/// Early memory allocator
/// Use it before formal bytes-allocator and pages-allocator can work!
/// This is a double-end memory range:
/// - Alloc bytes forward
/// - Alloc pages backward
///
/// [ bytes-used | avail-area | pages-used ]
/// |            | -->    <-- |            |
/// start       b_pos        p_pos       end
///
/// For bytes area, 'count' records number of allocations.
/// When it goes down to ZERO, free bytes-used area.
/// For pages area, it will never be freed!
///
pub struct EarlyAllocator<const SIZE: usize> {
    start: usize,
    end: usize,
    /// Next free address in the byte-allocation area.
    b_pos: usize,
    /// Start of the page-allocation area, growing downwards.
    p_pos: usize,
    /// Number of live byte allocations.
    count: usize,
}

impl<const SIZE: usize> EarlyAllocator<SIZE> {
    pub const fn new() -> Self {
        Self {
            start: 0,
            end: 0,
            b_pos: 0,
            p_pos: 0,
            count: 0,
        }
    }
}

impl<const SIZE: usize> BaseAllocator for EarlyAllocator<SIZE> {
    fn init(&mut self, start: usize, size: usize) {
        let end = start.saturating_add(size) & !(SIZE - 1);
        let start = start.saturating_add(SIZE - 1) & !(SIZE - 1);
        self.start = start;
        self.end = end.max(start);
        self.b_pos = self.start;
        self.p_pos = self.end;
        self.count = 0;
    }

    fn add_memory(&mut self, _start: usize, _size: usize) -> allocator::AllocResult {
        // The early allocator manages exactly one contiguous region.
        Err(allocator::AllocError::NoMemory)
    }
}

impl<const SIZE: usize> ByteAllocator for EarlyAllocator<SIZE> {
    fn alloc(
        &mut self,
        layout: core::alloc::Layout,
    ) -> allocator::AllocResult<core::ptr::NonNull<u8>> {
        let align = layout.align();
        if !align.is_power_of_two() {
            return Err(allocator::AllocError::InvalidParam);
        }

        // Zero-sized allocations still have to return a valid, aligned pointer.
        let size = layout.size().max(1);
        let start = self
            .b_pos
            .checked_add(align - 1)
            .ok_or(allocator::AllocError::NoMemory)?
            & !(align - 1);
        let end = start
            .checked_add(size)
            .ok_or(allocator::AllocError::NoMemory)?;
        if end > self.p_pos {
            return Err(allocator::AllocError::NoMemory);
        }

        self.b_pos = end;
        self.count += 1;
        core::ptr::NonNull::new(start as *mut u8).ok_or(allocator::AllocError::NoMemory)
    }

    fn dealloc(&mut self, pos: core::ptr::NonNull<u8>, _layout: core::alloc::Layout) {
        let addr = pos.as_ptr() as usize;
        if addr >= self.start && addr < self.b_pos && self.count > 0 {
            self.count -= 1;
            // Deallocations may be out of order, so reclaim the byte area only
            // after every allocation has been released.
            if self.count == 0 {
                self.b_pos = self.start;
            }
        }
    }

    fn total_bytes(&self) -> usize {
        self.end - self.start
    }

    fn used_bytes(&self) -> usize {
        self.b_pos - self.start
    }

    fn available_bytes(&self) -> usize {
        self.p_pos.saturating_sub(self.b_pos)
    }
}

impl<const SIZE: usize> PageAllocator for EarlyAllocator<SIZE> {
    const PAGE_SIZE: usize = SIZE;

    fn alloc_pages(
        &mut self,
        num_pages: usize,
        align_pow2: usize,
    ) -> allocator::AllocResult<usize> {
        if num_pages == 0
            || align_pow2 < SIZE
            || !align_pow2.is_power_of_two()
            || align_pow2 % SIZE != 0
        {
            return Err(allocator::AllocError::InvalidParam);
        }

        let size = num_pages
            .checked_mul(SIZE)
            .ok_or(allocator::AllocError::NoMemory)?;
        let pos = self
            .p_pos
            .checked_sub(size)
            .ok_or(allocator::AllocError::NoMemory)?
            & !(align_pow2 - 1);
        if pos < self.b_pos {
            return Err(allocator::AllocError::NoMemory);
        }

        self.p_pos = pos;
        Ok(pos)
    }

    fn dealloc_pages(&mut self, _pos: usize, _num_pages: usize) {
        // The page area is never reclaimed by the early allocator.
    }

    fn total_pages(&self) -> usize {
        (self.end - self.start) / SIZE
    }

    fn used_pages(&self) -> usize {
        (self.end - self.p_pos) / SIZE
    }

    fn available_pages(&self) -> usize {
        (self.p_pos - self.b_pos) / SIZE
    }
}
