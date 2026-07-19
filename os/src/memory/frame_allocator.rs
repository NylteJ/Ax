use core::{intrinsics::likely, num::NonZeroUsize};

use crate::{
    basic::{UnsafeInited, sync::SpinMutex},
    filesystem::filesystems::foreach_filesystem,
    memory::{
        MEMORY_END, PAGE_BYTES, PAGE_SHIFT,
        address::{PhysPageNum, VirtAddr},
        page_meta::{PAGE_METADATAS, PageMetaGuard, PageType},
    },
};

const ORDER: usize = 2;

pub struct FrameAllocator {
    allocator: SpinMutex<buddy_system_allocator::FrameAllocator<ORDER>>,
}

impl FrameAllocator {
    #[track_caller]
    pub fn try_alloc(&self, frame_count: usize) -> Option<PhysPageNum<0>> {
        likely(frame_count == 1);

        let try_alloc = || self.allocator.lock().alloc(frame_count);

        try_alloc()
            .or_else(|| {
                log::warn!("未能分配 {frame_count} 个物理页帧，尝试执行回收");

                foreach_filesystem(|fs| {
                    fs.writeback(NonZeroUsize::MAX);
                    fs.gc_inode();
                });

                log::info!("回收完毕，再度尝试分配");

                try_alloc()
            })
            .map(|idx| PhysPageNum(idx))
    }

    #[track_caller]
    pub fn alloc(&self, frame_count: usize) -> PhysPageNum<0> {
        self.try_alloc(frame_count)
            .unwrap_or_else(|| panic!("无法分配 {frame_count} 个物理页帧"))
    }

    #[track_caller]
    pub fn alloc_one_as(&self, page_type: PageType) -> PageMetaGuard {
        let page_num = self.alloc(1);

        // SAFETY: 分配器保证了这点
        unsafe { PAGE_METADATAS.mark_new_page(page_num, page_type) }
    }

    #[track_caller]
    pub fn dealloc(&self, frame_index: PhysPageNum<0>, frame_count: usize) {
        likely(frame_count == 1);

        self.allocator.lock().dealloc(frame_index.0, frame_count);
    }
}

pub static FRAME_ALLOCATOR: UnsafeInited<FrameAllocator> = unsafe { UnsafeInited::uninit() };

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

    let mut allocator = buddy_system_allocator::FrameAllocator::<ORDER>::new();
    allocator.add_frame(start, end);

    unsafe {
        UnsafeInited::init_unsafe(&FRAME_ALLOCATOR, FrameAllocator {
            allocator: SpinMutex::new(allocator),
        })
    };
}
