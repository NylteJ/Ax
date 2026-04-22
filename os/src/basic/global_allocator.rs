use core::arch::{asm, global_asm};

use buddy_system_allocator::LockedHeap;

const HEAP_SIZE_BYTES: usize = 16 * 1024 * 1024; // 16 MB

const BUDDY_SYSTEM_ALLOCATOR_ORDER: usize =
    HEAP_SIZE_BYTES.next_power_of_two().trailing_zeros() as usize + 1;

#[global_allocator]
static GLOBAL_ALLOCATOR: LockedHeap<BUDDY_SYSTEM_ALLOCATOR_ORDER> = LockedHeap::empty();

// 和启动栈数组一样，内核堆也不需要初始置零，因此我们用相同的方式定义，以避免置零开销。
global_asm!(
    ".section .bss.kheap",
    ".globl kheap_start",
    "kheap_start:",
    "    .space {HEAP_SIZE}",
    HEAP_SIZE = const HEAP_SIZE_BYTES,
);

pub fn init_heap() {
    let kheap_start: usize;
    unsafe {
        asm!("la {0}, kheap_start", out(reg) kheap_start);
        GLOBAL_ALLOCATOR.lock().init(kheap_start, HEAP_SIZE_BYTES);
    }
}
