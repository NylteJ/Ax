#[cfg(debug_assertions)]
use core::mem::MaybeUninit;
use core::{
    alloc::{GlobalAlloc, Layout},
    arch::{asm, global_asm},
    ptr::{NonNull, null_mut},
};

use buddy_system_allocator::Heap;
#[cfg(debug_assertions)]
use hashbrown::{DefaultHashBuilder, HashMap};

use crate::basic::sync::Mutex;

const HEAP_SIZE_BYTES: usize = 32 * 1024 * 1024; // 32 MB

const BUDDY_SYSTEM_ALLOCATOR_ORDER: usize =
    HEAP_SIZE_BYTES.next_power_of_two().trailing_zeros() as usize + 1;

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

        unsafe {
            self.allocator
                .dealloc(unsafe { NonNull::new_unchecked(ptr) }, layout)
        };
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
        unsafe { self.0.lock().alloc(layout) }
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
}

pub unsafe fn force_unlock_heap() {
    unsafe { GLOBAL_ALLOCATOR.0.assume_unlocked() };
}
