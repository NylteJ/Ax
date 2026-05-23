// TODO: 重构此部分
use core::{arch::asm, fmt::Debug, marker::PhantomPinned, pin::Pin};

use alloc::boxed::Box;

use crate::{
    arch::{self, interfaces::Basic},
    basic::shared_with_asm::SharedWithAsm,
    memory::{
        PAGE_BYTES, PAGE_SHIFTS,
        address::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum},
        page::{PTEFlags, PTEFlagsNative, PageTableEntry},
        page_meta::PageMetaGuard,
    },
};

const PAGE_TABLE_SIZE: usize = PAGE_BYTES / size_of::<usize>();
static_assertions::const_assert_eq!(PAGE_BYTES % size_of::<usize>(), 0);

/// 页表
///
/// 其布局和实际内存中的页表保证完全一致。这同时意味着可以以 `[usize; PAGE_TABLE_SIZE]` 的形式访问它。
///
/// 不具所有权
#[repr(C)]
#[repr(align(4096))]
pub struct PageTable<const LEVEL: usize> {
    frames: [PageTableEntry<LEVEL>; PAGE_TABLE_SIZE],

    _pinned: PhantomPinned,
}

seq_macro::seq!(N in 0..3 {
    static_assertions::const_assert!(size_of::<PageTable<N>>() == PAGE_BYTES);
    static_assertions::const_assert!(align_of::<PageTable<N>>() == PAGE_BYTES);
});

impl<const LEVEL: usize> PageTable<LEVEL> {
    pub fn new() -> Pin<Box<Self>> {
        debug_assert_eq!(PageTableEntry::<LEVEL>::new_invalid().bits, 0);
        unsafe { Pin::new_unchecked(Box::new_zeroed().assume_init()) }
    }

    pub fn map_local(self: &mut Pin<Box<Self>>, index: usize, entry: PageTableEntry<LEVEL>) {
        unsafe { self.as_mut().get_unchecked_mut() }.frames[index] = entry;
    }

    pub fn to_entry(&self, flags: PTEFlags) -> PageTableEntry<{ LEVEL + 1 }> {
        debug_assert!(flags.contains(PTEFlags::Valid));
        #[cfg(target_arch = "riscv64")]
        debug_assert!(!flags.contains(PTEFlags::Write | PTEFlags::Read | PTEFlags::Execute));

        PageTableEntry::new_secondary_table(
            VirtAddr::from_ptr(self).to_physical_address().into(),
            flags,
        )
    }

    fn as_usize_array(&self) -> &[usize; PAGE_TABLE_SIZE] {
        let ptr = self as *const Self as *const [usize; PAGE_TABLE_SIZE];
        unsafe { ptr.as_ref_unchecked() }
    }
    /// # Safety:
    /// 所有修改必须保持类不变量
    unsafe fn as_mut_usize_array(&mut self) -> &mut [usize; PAGE_TABLE_SIZE] {
        let ptr = self as *mut Self as *mut [usize; PAGE_TABLE_SIZE];

        #[expect(dropping_references)] // 这个 drop 并非 “does nothing”，在后面使用 self 会报错
        drop(self); // 保证不会同时存在两个可变借用（虽然我有点好奇不同类型的借用会不会产生问题）

        unsafe { ptr.as_mut_unchecked() }
    }
}

impl PageTable<2> {
    pub fn new_table() -> Pin<Box<Self>> {
        let mut ret = Self::new();

        // 布局详见 mod.rs

        let flags = PTEFlags::Valid | PTEFlags::Read | PTEFlags::Write | PTEFlags::Execute;

        // 1. 内核态的直接内存映射区（LoongArch 64 使用 DMW 所以不需要）
        #[cfg(target_arch = "riscv64")]
        for i in 0..16 {
            ret.map_local(256 + i, PageTableEntry::new(PhysPageNum(i), flags));
        }

        ret
    }

    pub fn to_native_register(&self) -> usize {
        let my_paddr = VirtAddr::from_ptr(self).to_physical_address();
        if cfg!(target_arch = "riscv64") {
            (8 << 60) | PhysPageNum::<0>::from(my_paddr).0
        } else {
            my_paddr.0
        }
    }

    pub unsafe fn switch_to(native_register: usize) {
        unsafe {
            #[cfg(target_arch = "riscv64")]
            asm!("csrw satp, {reg}", reg = in(reg) native_register);
            #[cfg(target_arch = "loongarch64")]
            asm!("csrwr {reg}, 0x19", reg = in(reg) native_register);
        }

        arch::NativeBasic::tlb_invalidate_all();
    }
    pub fn activitate(&self) {
        unsafe { PageTable::switch_to(self.to_native_register()) }
    }

    /// 将 `from` 映射到 `to`，标志为 `flags`（自动添加 Valid 和 User），支持大页
    ///
    /// 如果已经存在同地址的映射，则返回 Err，包含该映射的页表项的可变引用
    ///
    /// 不刷新 TLB，请手动刷新
    pub fn map<'a, const LEVEL: usize>(
        self: &'a mut Pin<Box<Self>>,
        from: VirtPageNum<LEVEL>,
        to: PhysPageNum<LEVEL>,
        flags: PTEFlags,
    ) -> Result<(), &'a mut PageTableEntry<LEVEL>> {
        let flags = PTEFlagsNative::from(flags | PTEFlags::Valid | PTEFlags::User);
        let vaddr: VirtAddr = from.into();

        let mut page_table = unsafe { self.as_mut().get_unchecked_mut().as_mut_usize_array() };
        let mut level = 2;
        while level > LEVEL {
            let index = (vaddr.0 >> PAGE_SHIFTS[level]) % PAGE_TABLE_SIZE;
            let entry = page_table[index];
            let entry_flags: PTEFlags = PTEFlagsNative::from_bits_truncate(entry).into();

            let new_table_ptr: *mut [usize; 512];
            if !entry_flags.contains(PTEFlags::Valid) {
                // 未分配下一级页表，分配之（这里分配的是哪一级并不重要，反正布局都一样）
                let new_table = PageTable::<0>::new();

                let new_table = unsafe { new_table.into_asm() };

                page_table[index] =
                    PhysPageNum::<0>::from(VirtAddr::from_ptr(new_table).to_physical_address())
                        .mask()
                        | PTEFlagsNative::from(PTEFlags::Valid).bits();
                new_table_ptr = new_table as _;
            } else {
                // 已经分配了下一级页表，直接使用
                debug_assert!(
                    !entry_flags.contains(PTEFlags::Read | PTEFlags::Write | PTEFlags::Execute)
                );
                let new_paddr: PhysAddr = PhysPageNum::<0>::from_pte(&PageTableEntry {
                    bits: page_table[index],
                })
                .into();
                let new_vaddr = new_paddr.to_virtual_address().0;
                new_table_ptr = new_vaddr as _;
            }

            page_table = unsafe { new_table_ptr.as_mut_unchecked() };
            level -= 1;
        }

        // 已经到了最末级页表，映射
        let index = (vaddr.0 >> PAGE_SHIFTS[level]) % PAGE_TABLE_SIZE;
        let old_flags: PTEFlags = PTEFlagsNative::from_bits_truncate(page_table[index]).into();

        if old_flags.contains(PTEFlags::Valid) {
            let ptr = &raw mut page_table[index] as *mut PageTableEntry<LEVEL>;
            Err(unsafe { ptr.as_mut_unchecked() })
        } else {
            page_table[index] = to.mask() | flags.bits();
            // log::trace!("mapped: {:#x} => {:#x}", VirtAddr::from(from).0, PhysAddr::from(to).0);
            Ok(())
        }
    }

    pub fn at<'a, const LEVEL: usize>(
        &'a self,
        index: VirtPageNum<LEVEL>,
    ) -> Option<&'a PageTableEntry<LEVEL>> {
        Self::get_ptr(self.as_usize_array(), index).map(|ptr| unsafe { ptr.as_ref_unchecked() })
    }
    pub fn at_mut<'a, const LEVEL: usize>(
        self: &'a mut Pin<Box<Self>>,
        index: VirtPageNum<LEVEL>,
    ) -> Option<&'a mut PageTableEntry<LEVEL>> {
        Self::get_ptr(self.as_usize_array(), index)
            .map(|ptr| unsafe { (ptr as *mut PageTableEntry<LEVEL>).as_mut_unchecked() })
    }

    fn get_ptr<const LEVEL: usize>(
        mut page_table: &[usize; 512],
        index: VirtPageNum<LEVEL>,
    ) -> Option<*const PageTableEntry<LEVEL>> {
        let vaddr: VirtAddr = index.into();

        let mut level = 2;
        while level >= LEVEL {
            let index = (vaddr.0 >> PAGE_SHIFTS[level]) % PAGE_TABLE_SIZE;
            let entry = page_table[index];
            let entry_flags: PTEFlags = PTEFlagsNative::from_bits_truncate(entry).into();

            if !entry_flags.contains(PTEFlags::Valid) {
                return None;
            }

            if level == LEVEL {
                return Some((&raw const page_table[index]).cast());
            }

            debug_assert!(
                !entry_flags.contains(PTEFlags::Read | PTEFlags::Write | PTEFlags::Execute)
            );
            let new_paddr: PhysAddr = PhysPageNum::<0>::from_pte(&PageTableEntry {
                bits: page_table[index],
            })
            .into();
            let new_vaddr = new_paddr.to_virtual_address().0;

            page_table = unsafe { (new_vaddr as *const [usize; 512]).as_ref_unchecked() };
            level -= 1;
        }

        None
    }

    /// 将所有用户态次级页表递归性地深拷贝，浅拷贝所有页表项（会增加底层 PageMetaGuard 的引用计数），同时将所有非只读页表项改为只读
    /// 支持大页，自动忽略无效项
    ///
    /// 对于内核态映射，双方的都不会改变
    pub fn clone_cow(self: &mut Pin<Box<Self>>) -> Pin<Box<Self>> {
        let this = unsafe { self.as_mut().get_unchecked_mut() };

        let mut ret = Self::new_table();
        let table = unsafe { ret.as_mut().get_unchecked_mut() };

        let no_write = !PTEFlagsNative::from(PTEFlags::Write).bits();

        for (idx, entry) in this.frames[..256].iter_mut().enumerate() {
            let flags_native = PTEFlagsNative::from_bits_truncate(entry.bits);
            let flags: PTEFlags = flags_native.into();
            if !flags.contains(PTEFlags::Valid) {
                continue;
            }

            if flags.intersects(PTEFlags::Read | PTEFlags::Write | PTEFlags::Execute) {
                // 大页
                entry.bits &= no_write;
                table.frames[idx] = *entry;
                todo!();
            } else {
                // 下一级页表
                let next_table = PageTable::<1>::new();

                let pte = next_table.to_entry(flags);

                table.frames[idx] = pte;

                let ret_next_table = unsafe { next_table.into_asm().as_mut_unchecked() };

                let phys_page = PhysPageNum::<0>::from_pte(&PageTableEntry { bits: entry.bits });
                let phys_addr = PhysAddr::from(phys_page);
                let vaddr = phys_addr.to_virtual_address();
                let my_next_table =
                    unsafe { vaddr.as_mut_ptr::<PageTable<1>>().as_mut_unchecked() };

                for (idx, entry) in my_next_table.frames.iter_mut().enumerate() {
                    let flags_native = PTEFlagsNative::from_bits_truncate(entry.bits);
                    let flags: PTEFlags = flags_native.into();
                    if !flags.contains(PTEFlags::Valid) {
                        continue;
                    }

                    if flags.intersects(PTEFlags::Read | PTEFlags::Write | PTEFlags::Execute) {
                        // 大页
                        entry.bits &= no_write;
                        ret_next_table.frames[idx] = *entry;
                        todo!();
                    } else {
                        // 下一级（也是最后一级）页表
                        let next_table = PageTable::<0>::new();

                        let pte = next_table.to_entry(flags);

                        ret_next_table.frames[idx] = pte;

                        let ret_next_table = unsafe { next_table.into_asm().as_mut_unchecked() };

                        let phys_page =
                            PhysPageNum::<0>::from_pte(&PageTableEntry { bits: entry.bits });
                        let phys_addr = PhysAddr::from(phys_page);
                        let vaddr = phys_addr.to_virtual_address();
                        let my_next_table =
                            unsafe { vaddr.as_mut_ptr::<PageTable<0>>().as_mut_unchecked() };

                        for (idx, entry) in my_next_table.frames.iter_mut().enumerate() {
                            let flags_native = PTEFlagsNative::from_bits_truncate(entry.bits);
                            let flags: PTEFlags = flags_native.into();
                            if !flags.contains(PTEFlags::Valid) {
                                continue;
                            }

                            entry.bits &= no_write;
                            ret_next_table.frames[idx] = *entry;
                            unsafe { PageMetaGuard::clone_asm(PhysPageNum::<0>::from_pte(entry)) };
                        }
                    }
                }
            }
        }

        ret
    }

    /// 取消映射 `[start, end)` 范围内的页表项，支持大页，自动忽略无效项
    ///
    /// 不刷新 TLB，请手动刷新
    pub fn unmap(&mut self, start: VirtPageNum<0>, end: VirtPageNum<0>) {
        if start == end {
            return;
        }

        let start_addr: VirtAddr = start.into();
        let end_addr: VirtAddr = VirtPageNum::<0>(end.0 - 1).into();

        // 现在取消映射 `[start_addr, end_addr]` 内的页表项即可

        unsafe { unmap_page_table_range(self.as_mut_usize_array(), start_addr.0, end_addr.0, 2) };
    }
}

pub type PageTableBox<const LEVEL: usize> = Pin<Box<PageTable<LEVEL>>>;

impl<const LEVEL: usize> Debug for PageTable<LEVEL> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!(
            "PAGE TABLE (LEVEL {}, {} KB) PHY {:#x}: {{",
            LEVEL + 1,
            (1 << PAGE_SHIFTS[LEVEL]) / 1024,
            VirtAddr::from_ptr(self).to_physical_address().0
        ))?;

        for (i, entry) in self
            .frames
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.bits != 0)
        {
            let vaddr: VirtAddr = VirtPageNum::<LEVEL>(i).into();
            let paddr: PhysAddr = PhysPageNum::from_pte(entry).into();

            let flags: PTEFlags = PTEFlagsNative::from_bits_truncate(entry.bits).into();

            f.write_fmt(format_args!(
                "\n    {i:3} : [{:#x}, {:#x}) => ",
                vaddr.0,
                vaddr.0 + (1 << PAGE_SHIFTS[LEVEL])
            ))?;

            if LEVEL > 0
                && flags.contains(PTEFlags::Valid)
                && !flags.intersects(PTEFlags::Read | PTEFlags::Write | PTEFlags::Execute)
            {
                let addr: PhysAddr =
                    PhysPageNum::<0>::from_pte(&PageTableEntry { bits: entry.bits }).into();
                let vaddr = addr.to_virtual_address().0;
                f.write_fmt(format_args!(
                    "NEXT TABLE, VADDR: {vaddr:#x}, PADDR: {:#x}",
                    addr.0
                ))?;
                seq_macro::seq!(N in 1..3 {
                    if N == LEVEL {
                        f.write_fmt(format_args!(
                            " {:?}", unsafe{(vaddr as *const PageTable<{N - 1}>).as_ref_unchecked()}
                        ))?;
                    }
                });
            } else {
                f.write_fmt(format_args!(
                    "[{:#x}, {:#x})",
                    paddr.0,
                    paddr.0 + (1 << PAGE_SHIFTS[LEVEL]),
                ))?;
            }

            f.write_fmt(format_args!(" (raw: {:#x})", entry.bits))?;
        }

        f.write_str("\n}")?;

        Ok(())
    }
}

unsafe fn drop_page_table_entry(entry: usize, level: usize) {
    // 只能说 Rust 真的不适合当 C++ 写，试了很久都没法像 C++ 那样递归析构，拿黑魔法先糊着了

    let flags: PTEFlags = PTEFlagsNative::from_bits_truncate(entry).into();

    if !flags.contains(PTEFlags::Valid) {
        return;
    }

    let phys_page = PhysPageNum::<0>::from_pte(&PageTableEntry { bits: entry });

    if level == 0 {
        unsafe { PageMetaGuard::drop_asm(phys_page) };
        return;
    }

    if flags.intersects(PTEFlags::Read | PTEFlags::Write | PTEFlags::Execute) {
        todo!("大页回收");
        return;
    }

    let addr: PhysAddr = phys_page.into();
    let vaddr = addr.to_virtual_address().0;

    let next_table = vaddr as *mut [usize; PAGE_TABLE_SIZE];

    for entry in unsafe { next_table.as_ref_unchecked() } {
        unsafe { drop_page_table_entry(*entry, level - 1) }
    }

    unsafe { PageTableBox::<0>::drop_asm(next_table as *mut _) }
}

unsafe fn unmap_page_table_range(
    table: &mut [usize; PAGE_TABLE_SIZE],
    start: usize,
    end: usize,
    level: usize,
) {
    debug_assert!(start <= end);
    debug_assert!(level == 2 || start >> PAGE_SHIFTS[level + 1] == end >> PAGE_SHIFTS[level + 1]);

    let start_head = (start >> PAGE_SHIFTS[level]) % PAGE_TABLE_SIZE;
    let end_head = (end >> PAGE_SHIFTS[level]) % PAGE_TABLE_SIZE;

    let start_tail = start % (1 << PAGE_SHIFTS[level]);
    let end_tail = end % (1 << PAGE_SHIFTS[level]);

    let tail_max = 0x3ffff000_usize % (1 << PAGE_SHIFTS[level]);

    for index in start_head..=end_head {
        let entry = &mut table[index];
        let flags: PTEFlags = PTEFlagsNative::from_bits_truncate(*entry).into();

        if !flags.contains(PTEFlags::Valid) {
            continue;
        }

        if (index != start_head || start_tail == 0) && (index != end_head || end_tail == 0) {
            unsafe { drop_page_table_entry(*entry, level) };
            *entry = 0;
        } else {
            debug_assert!(level > 0);
            debug_assert!(!flags.intersects(PTEFlags::Read | PTEFlags::Write | PTEFlags::Execute)); // 不为大页

            let phys_page = PhysPageNum::<0>::from_pte(&PageTableEntry { bits: *entry });
            let addr: PhysAddr = phys_page.into();
            let vaddr = addr.to_virtual_address().0;

            let next_table = vaddr as *mut [usize; PAGE_TABLE_SIZE];
            let next_table = unsafe { next_table.as_mut_unchecked() };

            let start =
                (index << PAGE_SHIFTS[level]) + if index == start_head { start_tail } else { 0 };
            let end = (index << PAGE_SHIFTS[level])
                + if index == end_head {
                    end_tail
                } else {
                    tail_max
                };

            unsafe { unmap_page_table_range(next_table, start, end, level - 1) };

            if next_table.iter().all(|entry| *entry == 0) {
                unsafe { PageTableBox::<0>::drop_asm(next_table as *mut _ as _) };
                *entry = 0;
            }
        }
    }
}
