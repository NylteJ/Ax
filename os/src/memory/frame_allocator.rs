use buddy_system_allocator::LockedFrameAllocator;
use lazy_static::lazy_static;

use crate::memory::{MEMORY_END, PAGE_BYTES, PAGE_SHIFT, address::VirtAddr};

lazy_static! {
    pub static ref FRAME_ALLOCATOR: LockedFrameAllocator<11> = LockedFrameAllocator::new();
}

pub fn init() {
    let ekernel: usize;
    unsafe { core::arch::asm!("la {0}, ekernel", out(reg) ekernel) };

    let start = VirtAddr(ekernel).ceil_to_page().to_physical_address().0 >> PAGE_SHIFT;
    let end = MEMORY_END >> PAGE_SHIFT;

    log::info!(
        "物理页帧分配器可用范围：{:#x} ~ {:#x} (共 {} 页, {} KB)",
        start,
        end,
        end - start,
        (end - start) * PAGE_BYTES / 1024
    );

    FRAME_ALLOCATOR.lock().add_frame(start, end);
}
