pub mod address;
pub mod address_space;
pub mod frame_allocator;
pub mod page;
pub mod page_meta;
pub mod page_table;
pub mod uaccess;
pub mod virtual_memory_area;

pub mod tlb_refill_handler;

// 注意：以下常量的值均由硬件决定，不能在此处随意修改。

// 4KB, 2 MB, 1 GB
pub const PAGE_SHIFTS: [usize; 3] = [12, 12 + 9, 12 + 9 * 2];

pub const PAGE_SHIFT: usize = PAGE_SHIFTS[0];
pub const PAGE_BYTES: usize = 1 << PAGE_SHIFT;

// RISC-V 64 内存区域分段：
// =================== 用户态区域（低 256 GB）===================
// 0 ~ 1GB：保留，用于检测空指针异常
// 1 ~ ~256GB：用户态虚拟地址空间，不同进程间隔离（线程间共享）
// 在这 ~255 GB 中，用户栈被放在最高处，往下增长；用户堆则放在各数据段往上的最低处，往上增长。
// ~256GB ~ 256GB：用于信号跳板等特殊区域，此部分映射很小
// =================== 内核态区域（高 256 GB）===================
// （这部分内存全部共享，也就是最外层页表的这一部分都指向同一批页）
// 0 ~ 16GB：直接物理内存映射。注意这是高 256 GB，空指针不在这一区间。
// =============================================================
// 注意，和 Linux 一样，我们不启用 KPTI。

// LoongArch 64 内存区域分段：
// =================== 用户态区域（低 256 GB）===================
// 0 ~ 1GB：保留，用于检测空指针异常
// 1 ~ ~256GB：用户态虚拟地址空间，不同进程间隔离（线程间共享）
// 在这 ~255 GB 中，用户栈被放在最高处，往下增长；用户堆则放在各数据段往上的最低处，往上增长。
// ~256GB ~ 256GB：用于信号跳板等特殊区域，此部分映射很小
// =============================================================
// DMW 映射：偏移量 0x1000000000000000，映射整个物理内存，仅内核态（PLV = 0）可访问
//
// 此布局天然免疫 Meltdown 漏洞（因为根本没有内核态映射），也就不需要 KPTI 了。
// （貌似也有说法是龙芯硬件上就免疫 Meltdown？总之结果就是不需要 KPTI）

const MEMORY_START: usize = 0x80000000;

#[cfg(target_arch = "riscv64")]
const MEMORY_SIZE: usize = 0x40000000;
#[cfg(target_arch = "loongarch64")]
const MEMORY_SIZE: usize = 0x30000000; // LoongArch 64 会将 256 MB 内存放置在低地址区域（尽管那部分内存貌似不太好用）

const MEMORY_END: usize = MEMORY_START + MEMORY_SIZE;

pub const STACK_ALIGN: usize = 16;

#[repr(C)]
#[repr(align(4096))]
pub struct Page([u8; PAGE_BYTES]);
static_assertions::const_assert_eq!(align_of::<Page>(), PAGE_BYTES);
static_assertions::const_assert_eq!(size_of::<Page>(), PAGE_BYTES);

impl Page {
    pub fn as_bytes(&self) -> &[u8; PAGE_BYTES] {
        &self.0
    }
}

/// 全零只读物理页，用于零页在不写入时的优化
///
/// 例如，对于 .bss 段等自动置零的段，只要不发生写入，就总可以把所有虚拟页都映射到这个页
// 它不能为 const，因为 Rust 中的 const 很大程度上其实是 C/C++ 的宏（没有固定地址，总是内联展开），很坑……
pub static ZERO_PAGE: Page = Page([0; PAGE_BYTES]);
