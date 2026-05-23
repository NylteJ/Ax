use crate::memory::{PAGE_BYTES, PAGE_SHIFTS, page::PageTableEntry};

#[repr(transparent)]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Debug)]
pub struct PhysAddr(pub usize);

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Debug)]
pub struct VirtAddr(pub usize);

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Debug)]
pub struct PhysPageNum<const LEVEL: usize>(pub usize);

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Debug)]
pub struct VirtPageNum<const LEVEL: usize>(pub usize);

impl<const LEVEL: usize> From<PhysAddr> for PhysPageNum<LEVEL> {
    fn from(value: PhysAddr) -> Self {
        Self(value.0 >> PAGE_SHIFTS[LEVEL])
    }
}

impl<const LEVEL: usize> From<PhysPageNum<LEVEL>> for PhysAddr {
    fn from(value: PhysPageNum<LEVEL>) -> Self {
        Self(value.0 << PAGE_SHIFTS[LEVEL])
    }
}

#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
compile_error!("Unsupported architecture");

const RISC_V: bool = cfg!(target_arch = "riscv64");

impl<const LEVEL: usize> PhysPageNum<LEVEL> {
    pub const fn mask(&self) -> usize {
        if RISC_V {
            self.0 << (PAGE_SHIFTS[LEVEL] - 2)
        } else {
            self.0 << PAGE_SHIFTS[LEVEL]
        }
    }

    pub fn from_pte(pte: &PageTableEntry<LEVEL>) -> Self {
        if RISC_V {
            Self((pte.bits >> (PAGE_SHIFTS[LEVEL] - 2)) & ((1 << 44) - 1))
        } else {
            Self((pte.bits >> PAGE_SHIFTS[LEVEL]) & ((1 << 48) - 1))
        }
    }
}

impl<const LEVEL: usize> From<VirtAddr> for VirtPageNum<LEVEL> {
    fn from(value: VirtAddr) -> Self {
        Self(value.0 >> PAGE_SHIFTS[LEVEL])
    }
}

impl<const LEVEL: usize> From<VirtPageNum<LEVEL>> for VirtAddr {
    fn from(value: VirtPageNum<LEVEL>) -> Self {
        Self(value.0 << PAGE_SHIFTS[LEVEL])
    }
}

// #[cfg(target_arch = "riscv64")] // FIXME
pub const VIRTUAL_ADDRESS_LOW_HIGHEST: usize = 0x3f_ffff_ffff;

pub const VIRTUAL_ADDRESS_HIGH_BASE: usize = if RISC_V {
    0xffff_ffc0_0000_0000
} else {
    0x1000_0000_0000_0000 // 此值要改就要连着 ld 脚本一起改
};
pub const LINEAR_MAPPING_OFFSET: usize = VIRTUAL_ADDRESS_HIGH_BASE;

impl PhysAddr {
    pub const fn to_virtual_address(&self) -> VirtAddr {
        VirtAddr(LINEAR_MAPPING_OFFSET + self.0)
    }
}
impl VirtAddr {
    pub const fn to_physical_address(&self) -> PhysAddr {
        PhysAddr(self.0 - LINEAR_MAPPING_OFFSET)
    }
}

impl VirtAddr {
    pub fn from_ptr<T>(ptr: *const T) -> Self {
        Self(ptr as usize)
    }

    pub fn as_ptr<T>(&self) -> *const T {
        self.0 as *const T
    }
    pub fn as_mut_ptr<T>(&self) -> *mut T {
        self.0 as *mut T
    }

    pub fn floor_to_page(&self) -> Self {
        Self(self.0 / PAGE_BYTES * PAGE_BYTES)
    }
    pub fn ceil_to_page(&self) -> Self {
        Self((self.0 + PAGE_BYTES - 1) / PAGE_BYTES * PAGE_BYTES)
    }
}
