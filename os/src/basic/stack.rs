use core::{any::type_name, cell::SyncUnsafeCell, fmt::Debug};

/// 内核栈指针
///
/// 对应栈的对齐和大小都是 BYTES 字节。
///
/// 假设栈地址为 [base, base + BYTES)，则栈指针的取值也必须在 [base, base + BYTES) 内。\
/// 这意味着其无法指向栈的高地址边界。由于此位置存放了指向 Task 结构体的指针等其它信息，实战中这不会产生问题。
///
/// 其值在 Rust 端只能读取，但在汇编端可以修改，即便它在 Rust 端声明为不可变。
pub struct KernelStackPtr<const BYTES: usize>(SyncUnsafeCell<usize>);

impl<const BYTES: usize> KernelStackPtr<BYTES> {
    /// 从地址创建栈指针
    ///
    /// # SAFETY
    /// 地址必须满足类的不变量
    pub unsafe fn from_addr(addr: usize) -> Self {
        Self(SyncUnsafeCell::new(addr))
    }

    /// 获取栈的低地址边界（内核中常说的栈底，平时常说的栈顶）地址
    pub fn get_low_addr(&self) -> usize {
        self.get_addr() & !(BYTES - 1)
    }

    /// 获取栈的高地址边界（内核中常说的栈顶，平时常说的栈底）地址
    pub fn get_high_addr(&self) -> usize {
        self.get_low_addr() + BYTES
    }

    /// 获取栈指针地址
    pub fn get_addr(&self) -> usize {
        // SAFETY: Rust 端此值不可修改，因此不可能存在可变借用；汇编端乱改值就不是这里的问题了
        unsafe { *(self.0.get() as *const _) }
    }

    pub const fn size() -> usize {
        BYTES
    }
    pub const fn align() -> usize {
        BYTES
    }
}

impl<const BYTES: usize> Debug for KernelStackPtr<BYTES> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("addr", &self.get_addr())
            .field("range", &(&self.get_low_addr()..&self.get_high_addr()))
            .finish()
    }
}

// Rust 没有 emplace 系的支持（或者说支持得很烂），因此调试模式下不得不调大栈大小来防止各种东西把栈压爆
const KERNEL_STACK_BYTES: usize = if cfg!(debug_assertions) { 65536 } else { 8192 };

/// 每个任务独占的内核栈的指针
pub type TaskKernelStackPtr = KernelStackPtr<KERNEL_STACK_BYTES>;
