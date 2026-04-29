use bitflags::bitflags;

use crate::memory::address::VirtAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualMemoryArea {
    pub start: VirtAddr,
    pub end: VirtAddr,

    pub flags: VMAFlags,

    pub vma_type: VMAType,

    pub growth: VMAGrowth,
}

impl VirtualMemoryArea {
    /// 判断两个 VMA 是否兼容，即是否除区间外的属性完全相同。注意它不检查区间。
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.flags == other.flags && self.vma_type == other.vma_type && self.growth == other.growth
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct VMAFlags: u8 {
        /// 权限位，表示可读
        const Read = 1 << 0;
        /// 权限位，表示可写
        const Write = 1 << 1;
        /// 权限位，表示可执行
        const Execute = 1 << 2;
        /// 位于用户态，用户态可访问，内核态不应随意访问
        ///
        /// 无此标志则用户不可访问，相应的内核可以随意访问
        const User = 1 << 3;
        /// 标记此区域在进程间共享，任一进程的修改都对其它观察者（进程、文件，等等）可见
        ///
        /// 无此标记则表明区域是私有的，修改会导致进程复制一份自己的副本并在其上修改，其它观察者不会察觉
        const Shared = 1 << 4;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VMAGrowth {
    /// 区域大小固定，无法增长
    Fixed,
    /// 区域可以向低地址增长，至多增长到指定地址
    GrowDown(VirtAddr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VMAType {
    /// 匿名映射，缺页时分配零页。堆、栈、.bss 段等都属于此类别。
    ///
    /// 注意清零内存哪怕对堆和栈来讲也是必须的，一是为了安全，二是 POSIX 要求 mmap 分配的匿名映射“contents are initialized to zero”。
    /// （TODO：尽管如此目前貌似并没有清零）
    Anonymous,
    /// 文件映射，缺页时从文件中读取数据。
    File(!), // TODO
}
