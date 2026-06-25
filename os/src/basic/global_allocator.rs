#[cfg(debug_assertions)]
use core::mem::MaybeUninit;
use core::{
    alloc::{GlobalAlloc, Layout},
    arch::{asm, global_asm},
    num::NonZeroUsize,
    ptr::{NonNull, null_mut},
};

use buddy_system_allocator::Heap;
#[cfg(debug_assertions)]
use hashbrown::{DefaultHashBuilder, HashMap};

use crate::{basic::sync::Mutex, filesystem::filesystems::foreach_filesystem};

const HEAP_SIZE_BYTES: usize = if cfg!(debug_assertions) {
    256 * 1024 * 1024 // 256 MB（因为此时的内核栈非常非常大）
} else {
    // FIXME：其实这也有点大了，未来加栈溢出检测时可以将内核栈独立出去
    if cfg!(target_arch = "riscv64") {
        64 * 1024 * 1024 // 64 MB
    } else {
        0 * 1024 * 1024 // 0 MB（我们下面会添加低地址区段的 256 MB）
    }
};

const MAX_BYTES_PER_ALLOCATION: usize = 8 * 1024 * 1024;

const BUDDY_SYSTEM_ALLOCATOR_ORDER: usize = MAX_BYTES_PER_ALLOCATION
    .next_power_of_two()
    .trailing_zeros() as usize
    + 1;

struct GlobalAllocatorInner {
    allocator: Heap<BUDDY_SYSTEM_ALLOCATOR_ORDER>,

    #[cfg(debug_assertions)]
    allocated:
        MaybeUninit<HashMap<usize, Layout, DefaultHashBuilder, &'static debug::DebugAllocator>>,
}

impl GlobalAllocatorInner {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let alloc_result = self.allocator.alloc(layout);

        #[cfg(debug_assertions)]
        if let Ok(ptr) = alloc_result {
            use uninit::extension_traits::MaybeUninitExt;

            let allocated = unsafe { self.allocated.assume_init_by_mut() };

            unsafe { allocated.insert_unique_unchecked(ptr.as_ptr().addr(), layout) };
        }

        alloc_result.map_or(null_mut(), |ptr| ptr.as_ptr())
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        #[cfg(debug_assertions)]
        {
            use uninit::extension_traits::MaybeUninitExt;

            let allocated = unsafe { self.allocated.assume_init_by_mut() };

            let old_layout = allocated
                .remove(&ptr.addr())
                .expect("解分配时使用的指针必须和分配时返回的指针完全一致（或许发生了 UAF？）");
            assert_eq!(
                layout, old_layout,
                "解分配时使用的布局必须和分配时使用的布局完全一致"
            );
        }

        unsafe { self.allocator.dealloc(NonNull::new_unchecked(ptr), layout) };
    }

    const fn new() -> Self {
        Self {
            allocator: Heap::empty(),
            #[cfg(debug_assertions)]
            allocated: MaybeUninit::uninit(),
        }
    }
}

#[cfg(debug_assertions)]
mod debug {
    use core::{alloc::Layout, cell::SyncUnsafeCell, mem::MaybeUninit, ptr::NonNull};

    use allocator_api2::alloc::{AllocError, Allocator};
    use buddy_system_allocator::Heap;

    use crate::basic::sync::Mutex;

    pub struct DebugAllocator {
        pub buffer: SyncUnsafeCell<MaybeUninit<[u8; 4 * 1024 * 1024]>>,
        pub heap: Mutex<Heap<32>>,
    }

    unsafe impl Allocator for &'static DebugAllocator {
        fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            self.heap
                .lock()
                .alloc(layout)
                .map(|ptr| NonNull::slice_from_raw_parts(ptr, layout.size()))
                .map_err(|_| AllocError)
        }

        unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
            unsafe { self.heap.lock().dealloc(ptr, layout) };
        }
    }

    impl DebugAllocator {
        pub const fn new() -> Self {
            Self {
                buffer: SyncUnsafeCell::new(MaybeUninit::uninit()),
                heap: Mutex::new(Heap::empty()),
            }
        }
    }

    pub static DEBUG_ALLOCATOR: DebugAllocator = DebugAllocator::new();
}

struct GlobalAllocator(Mutex<GlobalAllocatorInner>);

unsafe impl GlobalAlloc for GlobalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let try_alloc = || NonNull::new(unsafe { self.0.lock().alloc(layout) });

        let get_rest_bytes = || {
            let guard = self.0.lock();
            guard.allocator.stats_total_bytes() - guard.allocator.stats_alloc_actual()
        };

        try_alloc().or_else(||{
            log::warn!(
                "内核堆内存不足，无法完成 {layout:?} 的分配请求（当前堆内存剩余 {} 字节），正在尝试释放内存...",
                get_rest_bytes()
            );

            foreach_filesystem(|fs| {
                let gced = fs.full_gc_dentry();
                log::info!("从 FileSystem {fs:p} 中回收了 {gced} 个 DEntry");
            });

            log::info!("回收完毕，当前堆内存剩余 {} 字节", get_rest_bytes());

            try_alloc()
        }).or_else(||{
            log::warn!("分配请求仍然失败！尝试更激进的回收策略...");

            foreach_filesystem(|fs| {
                fs.writeback(NonZeroUsize::MAX);
                fs.gc_inode();
                fs.shrink_cache();
            });

            log::info!("激进回收完毕，当前堆内存剩余 {} 字节", get_rest_bytes());

            try_alloc()
        }).map_or(null_mut(), |ptr| ptr.as_ptr())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.0.lock().dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: GlobalAllocator = GlobalAllocator(Mutex::new(GlobalAllocatorInner::new()));

// 和启动栈数组一样，内核堆也不需要初始置零，因此我们用相同的方式定义，以避免置零开销。
global_asm!(
    ".section .bss.kheap",
    ".globl kheap_start",
    "kheap_start:",
    "    .space {HEAP_SIZE}",
    HEAP_SIZE = const HEAP_SIZE_BYTES,
);

pub fn init_heap() {
    let kheap_start;

    unsafe { asm!("la {0}, kheap_start", out(reg) kheap_start) };

    let mut global_allocator = GLOBAL_ALLOCATOR.0.lock();
    unsafe {
        global_allocator
            .allocator
            .init(kheap_start, HEAP_SIZE_BYTES);

        #[cfg(debug_assertions)]
        debug::DEBUG_ALLOCATOR.heap.lock().init(
            debug::DEBUG_ALLOCATOR.buffer.get().addr(),
            size_of_val(&debug::DEBUG_ALLOCATOR.buffer),
        );
    }

    #[cfg(debug_assertions)]
    global_allocator
        .allocated
        .write(HashMap::new_in(&debug::DEBUG_ALLOCATOR));

    log::info!(
        "已初始化内核堆，对应物理地址：{kheap_start:#x} ~ {:#x} ({HEAP_SIZE_BYTES} 字节)",
        kheap_start + HEAP_SIZE_BYTES
    );

    #[cfg(target_arch = "loongarch64")]
    {
        use crate::memory::address::PhysAddr;

        const START: usize = PhysAddr(0x00000000).to_virtual_address().0;
        const END: usize = PhysAddr(0x10000000).to_virtual_address().0;

        unsafe { global_allocator.allocator.add_to_heap(START, END) };

        log::info!(
            "已将低地址区段内存添加到内核堆，地址范围：{START:#x} ~ {END:#x} ({} 字节)",
            END - START
        );
    }
}

pub unsafe fn force_unlock_heap() {
    unsafe { GLOBAL_ALLOCATOR.0.assume_unlocked() };
}

#[cfg_attr(not(debug_assertions), inline(always))]
pub fn debug_assert_heap_mem_valid<T: ?Sized>(ptr: *const T) {
    #[cfg(debug_assertions)]
    {
        let mut allocator = GLOBAL_ALLOCATOR.0.lock();
        let allocated = unsafe { allocator.allocated.assume_init_ref() };

        use core::any::type_name;

        let ptr_addr = ptr.addr();
        let Some(layout) = allocated.get(&ptr_addr) else {
            panic!(
                "{} 类型的指针 {ptr_addr:#x} 不在已分配的堆内存中，或未指向分配的内存的起始位置",
                type_name::<T>()
            );
        };
        assert_eq!(
            unsafe { Layout::for_value_raw(ptr) },
            *layout,
            "{} 类型的指针 {ptr_addr:#x} 指向的堆内存布局不匹配",
            type_name::<T>()
        );
    }
}
