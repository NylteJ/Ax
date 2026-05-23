use bitflags::bitflags;

use crate::memory::address::PhysPageNum;

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry<const LEVEL: usize> {
    pub bits: usize,
}

impl<const LEVEL: usize> PageTableEntry<LEVEL> {
    pub fn new(ppn: PhysPageNum<LEVEL>, flags: PTEFlags) -> Self {
        let flags: PTEFlagsNative = flags.into();
        Self {
            bits: ppn.mask() | flags.bits(),
        }
    }

    pub fn new_secondary_table(ppn: PhysPageNum<0>, flags: PTEFlags) -> Self {
        let flags: PTEFlagsNative = flags.into();
        Self {
            bits: ppn.mask() | flags.bits(),
        }
    }

    pub const fn new_invalid() -> Self {
        static_assertions::const_assert!(!PTEFlagsNative::Valid.is_empty());
        Self { bits: 0 }
    }

    pub fn flags(&self) -> PTEFlags {
        // bits 原本是使用按位或设定的，因此这里直接转换就能提取出 flags
        let flags_native = PTEFlagsNative::from_bits_truncate(self.bits);
        flags_native.into()
    }
}

#[cfg(target_arch = "riscv64")]
bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct PTEFlagsNative: usize {
        const Valid = 1 << 0;
        const Read = 1 << 1;
        const Write = 1 << 2;
        const Execute = 1 << 3;
        const User = 1 << 4;
        const Global = 1 << 5;
        const Accessed = 1 << 6;
        const Dirty = 1 << 7;
    }
}

#[cfg(target_arch = "riscv64")]
pub type PTEFlags = PTEFlagsNative;

#[cfg(target_arch = "loongarch64")]
bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct PTEFlagsNative: usize {
        const Valid = 1 << 0;
        const Dirty = 1 << 1;
        const User = 0b11 << 2;
        const Global = 1 << 6;

        const NRead = 1 << (usize::BITS - 3);
        const NExecute = 1 << (usize::BITS - 2);
    }

    #[derive(Debug, Clone, Copy)]
    pub struct PTEFlags: usize {
        const Valid = 1 << 0;
        const Dirty = 1 << 1;
        const User = 0b11 << 2;
        const Global = 1 << 6;

        const Read = 1 << (usize::BITS - 3);
        const Execute = 1 << (usize::BITS - 2);
        const Write = PTEFlags::Dirty.bits(); // LoongArch 64 没有单独的 Write 位，但可以用 Dirty 位在语义上替代
    }
}

#[cfg(target_arch = "loongarch64")]
impl From<PTEFlagsNative> for PTEFlags {
    fn from(value: PTEFlagsNative) -> Self {
        const MASK: usize = PTEFlagsNative::NRead.bits() | PTEFlagsNative::NExecute.bits();
        Self::from_bits_retain(value.bits() ^ MASK)
    }
}

#[cfg(target_arch = "loongarch64")]
impl From<PTEFlags> for PTEFlagsNative {
    fn from(value: PTEFlags) -> Self {
        const MASK: usize = PTEFlagsNative::NRead.bits() | PTEFlagsNative::NExecute.bits();
        Self::from_bits_retain(value.bits() ^ MASK)
    }
}
