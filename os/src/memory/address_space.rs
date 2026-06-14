use core::{intrinsics::volatile_set_memory, mem::replace, pin::Pin, sync::atomic::Ordering};

use alloc::{boxed::Box, vec::Vec};

use crate::{
    arch::{self, interfaces::Basic},
    basic::shared_with_asm::SharedWithAsm,
    memory::{
        ZERO_PAGE,
        address::{MMAP_END, MMAP_START, PhysPageNum, VirtAddr, VirtPageNum},
        frame_allocator::FRAME_ALLOCATOR,
        page::{PTEFlags, PTEFlagsNative, PageTableEntry},
        page_meta::{self, PAGE_METADATAS, PageMetaGuard},
        page_table::{PageTable, PageTableBox},
        virtual_memory_area::{VMAFlags, VMAGrowth, VMAType, VirtualMemoryArea},
    },
    trap::page_fault_type::PageFaultType,
};

#[derive(Debug)]
pub struct AddressSpace {
    /// VMA 列表，不重叠、无零大小段且升序排列
    ///
    /// （为什么这里用 vector？因为我们根本不会有那么多 VMA 插入/删除，论内存占用、查找速度和实现简单性，有序数组甩其它数据结构十条街）
    vmas: Vec<VirtualMemoryArea>,

    /// 页表，包含用户态和内核态的映射（我们不使用 KPTI）
    ///
    /// 注意内核态映射不体现在 VMA 列表中，且 VMA 中存在的映射不一定落实到了页表中（但反过来，用户态映射一定存在对应的 VMA）
    page_table: PageTableBox<2>,

    /// 当前堆顶
    pub brk: VirtAddr,
}

impl AddressSpace {
    pub fn new() -> Self {
        Self {
            vmas: Vec::new(),
            page_table: PageTable::new_table(),
            brk: VirtAddr(0),
        }
    }

    /// 将给定 VMA 添加到 VMA 列表中。会自动合并与之相邻的其它 VMA。
    ///
    /// # Panics
    /// 如果待插入 VMA 的 start >= end，或与现有任意 VMA 发生重叠，则会 panic。
    pub fn add_vma(&mut self, vma: VirtualMemoryArea) {
        log::trace!("add_vma: {vma:x?}");
        assert!(
            vma.start < vma.end,
            "VMA 大小必须大于 0（{:#x} >= {:#x}）",
            vma.start.0,
            vma.end.0
        );

        let lower_bound = self.find_vma_index_lower_bound(vma.start);

        let upper_bound = lower_bound.map_or(0, |x| x + 1);
        assert!(
            self.vmas
                .get(upper_bound)
                .map_or(true, |upper| upper.start >= vma.end),
            "VMA 发生重叠"
        );

        if let Some(lower_bound) = lower_bound
            && self.vmas[lower_bound].end == vma.start
            && self.vmas[lower_bound].is_compatible_with(&vma)
        {
            // 与前一个 VMA 相邻，合并
            self.vmas[lower_bound].end = vma.end;

            if let Some(upper_bnd) = self.vmas.get_mut(upper_bound)
                && upper_bnd.start == vma.end
                && upper_bnd.is_compatible_with(&vma)
            {
                // 与后一个 VMA 也相邻，继续合并
                self.vmas[lower_bound].end = upper_bnd.end;
                self.vmas.remove(upper_bound);
            }

            return;
        }

        if let Some(upper_bound) = self.vmas.get_mut(upper_bound)
            && upper_bound.start == vma.end
            && upper_bound.is_compatible_with(&vma)
        {
            // 与后一个 VMA 相邻，合并
            upper_bound.start = vma.start;
            return;
        }

        // 与前后 VMA 都不相邻，直接插入
        self.vmas.insert(upper_bound, vma);
    }

    /// 寻找包含给定地址的 VMA。
    pub fn find_vma(&self, addr: VirtAddr) -> Option<&VirtualMemoryArea> {
        let upper_bound = self.find_vma_index_upper_bound(addr);

        if upper_bound == 0 {
            None
        } else {
            let vma = &self.vmas[upper_bound - 1];
            if addr < vma.end { Some(vma) } else { None }
        }
    }

    /// 设置 brk，会同步修改 VMA。如果因 VMA 冲突或传入 brk 过小而无法修改则返回旧值，否则返回新值。
    ///
    /// 如果有意不想修改 VMA，请直接修改 [`brk`][`Self::brk`] 字段。
    ///
    /// 除此之外，使用它意味着你需要先创立一个至少一页的堆 VMA，并对应直接设置 `brk` 字段。
    pub fn set_brk(&mut self, new_brk: VirtAddr) -> VirtAddr {
        let old_brk_ceiled = self.brk.ceil_to_page();
        let new_brk_ceiled = new_brk.ceil_to_page();

        // FIXME: 现在的实现忽略了很多边界情况，例如：
        // - 用户 unmap 了堆区
        // - 用户在堆区后执行了 mmap，两个 VMA 合并了

        if new_brk_ceiled == old_brk_ceiled {
            // 仍在同一页内，无需改动 VMA，修改 brk 字段即可
            // 注意堆区 VMA 至少有一页，因此这里是没问题的（用户看来可能还是有问题就是了）
            self.brk = new_brk;
            return new_brk;
        }

        let index = self
            .vmas
            .binary_search_by(|vma| vma.end.cmp(&old_brk_ceiled))
            .expect("应当始终存在一个以 brk 结尾的 VMA");
        let heap_vma = &mut self.vmas[index];

        if new_brk < heap_vma.start {
            // 新的 brk 过小，无法修改
            return self.brk;
        }

        if new_brk_ceiled > old_brk_ceiled {
            let has_sufficient_space = self
                .vmas
                .get(index + 1)
                .map_or(true, |vma| vma.start >= new_brk_ceiled);

            if has_sufficient_space {
                self.vmas[index].end = new_brk_ceiled;
                self.brk = new_brk;
            }
        } else {
            // 已经验证过了可以修改
            heap_vma.end = new_brk_ceiled;
            self.brk = new_brk;

            // 除此之外，和扩张不同，收缩堆区意味着 unmap 现有内存
            self.page_table
                .unmap(new_brk_ceiled.into(), old_brk_ceiled.into());
        }

        return self.brk;
    }

    /// 删除所有位于 [start, end) 内的 VMA。对于部分位于其中的 VMA，会删除位于此范围内的部分。
    ///
    /// # Panics
    /// 如果 start > end，则会 panic。
    pub fn remove_vmas_between(&mut self, start: VirtAddr, end: VirtAddr) {
        assert!(start <= end, "范围无效");

        if start == end {
            return;
        }

        let lower_bound = self.find_vma_index_lower_bound(start);

        let begin = lower_bound.map_or(0, |x| x + 1);

        if begin == self.vmas.len() {
            return;
        }

        let full_remove;

        let begin_vma = &mut self.vmas[begin];
        if begin_vma.start < start {
            // 第一个 VMA 部分位于范围外

            if begin_vma.end > end {
                // |==============|
                //    |========|
                let ed = replace(&mut begin_vma.end, start);
                let new_vma = VirtualMemoryArea {
                    start: end,
                    end: ed,
                    flags: begin_vma.flags,
                    vma_type: begin_vma.vma_type.splitted(end.0 - begin_vma.start.0),
                    growth: begin_vma.growth,
                };
                self.vmas.insert(begin + 1, new_vma);

                return;
            } else {
                // |=======|
                //    |========|
                begin_vma.end = start;
                full_remove = begin + 1;
            }
        } else {
            //  |=====|
            // |========|
            // 或
            //  |=========|
            // |========|
            full_remove = begin;
        }

        let last_index =
            self.vmas[full_remove..].partition_point(|vma| vma.end <= end) + full_remove;

        if let Some(last) = self.vmas.get_mut(last_index)
            && last.start < end
        {
            //  |=========|
            // |========|
            last.vma_type = last.vma_type.splitted(end.0 - last.start.0);
            last.start = end;
        }

        self.vmas.drain(full_remove..last_index);
    }

    /// 将 [start, end) 内的所有 VMA 的标志用给定函数修改。对于部分位于其中的 VMA，会自动拆分。如果 `func` 返回的标志与原来相同，则不进行修改。
    ///
    /// `func` 应当是纯函数。
    ///
    /// # Panics
    /// 如果 start > end，则会 panic。
    pub fn modify_vma_flags_between(
        &mut self,
        start: VirtAddr,
        end: VirtAddr,
        func: impl Fn(VMAFlags) -> VMAFlags,
    ) {
        assert!(start <= end, "范围无效");

        if start == end {
            return;
        }

        let lower_bound = self.find_vma_index_lower_bound(start);

        let begin = lower_bound.map_or(0, |x| x + 1);

        if begin == self.vmas.len() {
            return;
        }

        let full;

        let begin_vma = &mut self.vmas[begin];
        if begin_vma.start < start {
            // 第一个 VMA 部分位于范围外

            let old_flags = begin_vma.flags;
            let new_flags: VMAFlags = func(old_flags);
            if new_flags != old_flags {
                if begin_vma.end > end {
                    // |==============|
                    //    |========|
                    let ed = replace(&mut begin_vma.end, start);
                    let new_vma_1 = VirtualMemoryArea {
                        start,
                        end,
                        flags: new_flags,
                        vma_type: begin_vma.vma_type.splitted(start.0 - begin_vma.start.0),
                        growth: begin_vma.growth,
                    };
                    let new_vma_2 = VirtualMemoryArea {
                        start: end,
                        end: ed,
                        flags: old_flags,
                        vma_type: begin_vma.vma_type.splitted(end.0 - begin_vma.start.0),
                        growth: begin_vma.growth,
                    };
                    self.vmas.insert(begin + 1, new_vma_1);
                    self.vmas.insert(begin + 2, new_vma_2);

                    return;
                } else {
                    // |=======|
                    //    |========|
                    let ed = replace(&mut begin_vma.end, start);
                    let new_vma = VirtualMemoryArea {
                        start,
                        end: ed,
                        flags: new_flags,
                        vma_type: begin_vma.vma_type.splitted(start.0 - begin_vma.start.0),
                        growth: begin_vma.growth,
                    };
                    self.vmas.insert(begin + 1, new_vma);
                    full = begin + 2;
                }
            } else {
                full = begin + 1;
            }
        } else {
            //  |=====|
            // |========|
            // 或
            //  |=========|
            // |========|
            full = begin;
        }

        let last_index = self.vmas[full..].partition_point(|vma| vma.end <= end) + full;

        if let Some(last) = self.vmas.get_mut(last_index)
            && last.start < end
            && let new_flags = func(last.flags)
            && new_flags != last.flags
        {
            //  |=========|
            // |========|
            let new_vma = VirtualMemoryArea {
                end,
                ..last.clone()
            };
            last.vma_type = last.vma_type.splitted(end.0 - last.start.0);
            last.start = end;
            self.vmas.insert(last_index, new_vma);
        }

        for vma in &mut self.vmas[full..last_index] {
            let old_flags = vma.flags;
            let new_flags = func(old_flags);
            if new_flags != old_flags {
                vma.flags = new_flags;
            }
        }
    }

    pub fn get_page_table(&self) -> &PageTable<2> {
        self.page_table.as_ref().get_ref()
    }

    pub fn get_page_table_mut(&mut self) -> &mut Pin<Box<PageTable<2>>> {
        &mut self.page_table
    }

    pub fn handle_page_fault(
        &mut self,
        addr: VirtAddr,
        fault_type: PageFaultType,
    ) -> Result<(), ()> {
        let vma = {
            let upper_bound = self.find_vma_index_upper_bound(addr);

            if upper_bound != 0 && addr < self.vmas[upper_bound - 1].end {
                // 找到了直接的 VMA
                &self.vmas[upper_bound - 1]
            } else if let Some(vma_may_growth) = self.vmas.get_mut(upper_bound)
                && let VMAGrowth::GrowDown(min_addr) = vma_may_growth.growth
                && addr.floor_to_page() >= min_addr
            {
                // 没找到直接 VMA，但找到了能增长的 VMA，增长并使用该 VMA
                // 注意走到这一步的前提就是没有直接找到包含 addr 的 VMA，因此无需担心重叠
                vma_may_growth.start = addr.floor_to_page();
                vma_may_growth
            } else {
                // 此地址不位于任何可能的 VMA 中，无法处理
                return Err(());
            }
        };

        let flag = match fault_type {
            PageFaultType::Write => VMAFlags::Write,
            PageFaultType::Read => VMAFlags::Read,
            PageFaultType::Execute => VMAFlags::Execute,
        };

        if !vma.flags.contains(flag | VMAFlags::User) {
            return Err(()); // 权限不允许，不能访问
        }

        let mut page_flag = PTEFlags::empty();
        if vma.flags.contains(VMAFlags::Read) {
            page_flag |= PTEFlags::Read;
        }
        if vma.flags.contains(VMAFlags::Write) {
            page_flag |= PTEFlags::Write;
        }
        if vma.flags.contains(VMAFlags::Execute) {
            page_flag |= PTEFlags::Execute;
        }

        let virt_page_num = VirtPageNum::<0>::from(addr);

        // 已经检查了 VMA 部分没有问题（有对应 VMA 且权限允许），接下来映射页表

        match &vma.vma_type {
            VMAType::Anonymous => {
                if fault_type == PageFaultType::Read {
                    // 读取，可以先直接映射到零页
                    let ppn =
                        PhysPageNum::from(VirtAddr::from_ptr(&ZERO_PAGE).to_physical_address());

                    page_flag.remove(PTEFlags::Write);

                    self.page_table.map(virt_page_num, ppn, page_flag).unwrap();
                } else {
                    let mut old_entry = self.page_table.at_mut(virt_page_num);

                    if let Some(old_entry) = &mut old_entry
                        && let old_guard =
                            unsafe { PageMetaGuard::from_asm(PhysPageNum::from_pte(old_entry)) }
                        && old_guard.is_unique_ref()
                    {
                        // 写时复制优化：如果原来映射的页只有这一处引用，那么直接修改页权限，无需复制
                        log::trace!("写时复制优化: {:#x}", PhysPageNum::from_pte(old_entry).0);
                        unsafe { old_guard.into_asm() };
                        old_entry.bits |= PTEFlagsNative::from(PTEFlags::Write).bits();
                    } else {
                        // 否则，需要分配一个新页、复制其内容并将其插入到页表中
                        let physical_memory =
                            PhysPageNum(FRAME_ALLOCATOR.lock().alloc(1).expect("无法分配物理页"));

                        let page_guard = unsafe {
                            PAGE_METADATAS
                                .mark_new_page(physical_memory, page_meta::PageType::Anonymous)
                        };

                        let memory = unsafe { page_guard.visit_memory_mut() };

                        if let Some(old_entry) = &mut old_entry
                            && let old_pte = PhysPageNum::from_pte(old_entry)
                            && old_pte
                                != VirtAddr::from_ptr(&ZERO_PAGE).to_physical_address().into()
                        {
                            // 此时不能笃定复制是否等价于清零，只能老老实实复制
                            let guard = unsafe { PageMetaGuard::from_asm(old_pte) };
                            memory.copy_from_slice(guard.visit_memory());
                            log::trace!("COW, {:#x} => {:#x}", old_pte.0, physical_memory.0);
                            unsafe { guard.into_asm() }; // 此时上面那个 if 已经将所有权转移了，我们这里不能再释放一次（FIXME）
                        } else {
                            // 复制等价于清零，此时清零往往更快
                            unsafe { volatile_set_memory(memory.as_mut_ptr(), 0, memory.len()) };
                            // log::trace!("zeroed, {:#x}", physical_memory.0);
                        }

                        if let Some(old_entry) = old_entry {
                            *old_entry = PageTableEntry::new(
                                physical_memory,
                                old_entry.flags() | PTEFlags::Write,
                            );
                        } else {
                            self.page_table
                                .map(virt_page_num, physical_memory, page_flag)
                                .unwrap();
                        }
                        unsafe { page_guard.into_asm() }; // 交出对新页的引用，交给页表管理
                    }
                }
            }
            VMAType::File { file, page_index } => 'handle: {
                if !vma.flags.contains(VMAFlags::Shared) {
                    // 私有映射
                    let old_entry = self.page_table.at_mut(virt_page_num);

                    if let Some(old_entry) = old_entry {
                        // 写时复制
                        debug_assert_eq!(fault_type, PageFaultType::Write);
                        debug_assert!(!old_entry.flags().contains(PTEFlags::Write));

                        let page_num = PhysPageNum::<0>::from_pte(old_entry);
                        let page_guard = unsafe { PageMetaGuard::from_asm(page_num) };

                        // 注意，和匿名页的情况不一样，页缓存无论如何不能乱改，因此复制是不可避免的

                        let new_phys_mem =
                            PhysPageNum(FRAME_ALLOCATOR.lock().alloc(1).expect("无法分配物理页"));

                        let new_page = unsafe {
                            PAGE_METADATAS
                                .mark_new_page(new_phys_mem, page_meta::PageType::Anonymous)
                        };

                        unsafe { new_page.visit_memory_mut() }
                            .copy_from_slice(page_guard.visit_memory());

                        let asm_handle = unsafe { new_page.into_asm() };
                        debug_assert!(asm_handle == new_phys_mem);

                        *old_entry =
                            PageTableEntry::new(new_phys_mem, PTEFlags::Write | old_entry.flags());
                        drop(page_guard); // 释放对旧页的引用

                        break 'handle;
                    } else {
                        // 还没有映射，修改页权限以便后续写时复制即可。如果 VMA 本身不允许写，此指令没有任何作用。
                        // 注意，哪怕这次的指令就是写入，由于我们不能直接改页缓存（私有映射要求它做出的所有改动对别人不可见），
                        // 我们最终还是要复制一次。那么，再考虑到页缓存也是必要的，也就是说无论如何我们都要读取两次，因此两次 trap
                        // 也是完全可接受的。
                        page_flag.remove(PTEFlags::Write);
                    }
                }

                let pos_offset = virt_page_num.0 - VirtPageNum::<0>::from(vma.start).0;
                let page_index = *page_index + pos_offset as u64;

                let inode = file.get_inode();
                let inode_data_rguard = inode.data.read();
                let page_cache = inode_data_rguard
                    .page_cache
                    .as_ref()
                    .expect("文件映射必须有页缓存");

                let fs = unsafe { inode.fs.as_ref() };

                let cache_page = match page_cache.get(page_index) {
                    Some(page) => page,
                    None => {
                        let page_guard = page_cache.alloc_without_insert(page_index);

                        let page_mem = unsafe { page_guard.visit_memory_mut() };
                        let read_bytes = match fs.ops.read_to_page(inode, page_index, page_mem) {
                            Ok(bytes) => bytes,
                            Err(err) => {
                                debug_assert!(false, "文件映射读取页面失败：{err:?}");
                                return Err(());
                            }
                        };
                        let page_tail_ptr = unsafe { page_mem.as_mut_ptr().add(read_bytes) };
                        unsafe {
                            volatile_set_memory(page_tail_ptr, 0, page_mem.len() - read_bytes)
                        };

                        page_cache.insert(page_index, page_guard)
                    }
                };

                // 由于我们将这个页插入到了页表中，这里需要 +1 引用计数
                unsafe { cache_page.ref_count().fetch_add(1, Ordering::Relaxed) };

                self.page_table
                    .map(virt_page_num, cache_page.get_page_num(), page_flag)
                    .unwrap();
            }
        }

        arch::NativeBasic::tlb_invalidate_addr(addr);

        Ok(())
    }

    /// 将当前地址空间复制一份，并将两个地址空间都改为写时复制。
    ///
    /// 地址空间元数据的复制本身是深拷贝，但底层所有物理页都是浅拷贝。
    ///
    /// 注意此函数很可能需要紧跟 TLB 刷新
    pub fn clone_cow(&mut self) -> Self {
        let page_table = self.page_table.clone_cow();

        Self {
            vmas: self.vmas.clone(),
            page_table,
            brk: self.brk,
        }
    }

    /// 执行 mmap，将 `vma` 插入到当前地址空间中。返回插入 VMA 的起始地址。若长度过长则返回 [`Err`]。
    ///
    /// `vma` 的 `start` 字段相当于 mmap 的 `start` 参数，在 `fixed` 为 false 时是提示，在 `fixed` 为 true 时是强制要求，此时可能会解除现有映射。
    /// 特别地，当 `fixed` 为 false 且 `vma.start` 为 0 时，`start` 字段会直接被忽略。
    pub fn mmap(&mut self, vma: VirtualMemoryArea, fixed: bool) -> Result<VirtAddr, ()> {
        log::trace!("mmap: {vma:x?}, fixed: {fixed}");

        let lower_bound = self.find_vma_index_lower_bound(vma.start);
        let index = lower_bound.map_or(0, |x| x + 1);

        let good_to_place = (vma.start.0 != 0 || fixed)
            && self.vmas.get(index).map_or(true, |v| v.start >= vma.end);

        if good_to_place {
            let ret = vma.start;
            self.vmas.insert(index, vma);
            return Ok(ret);
        }

        if fixed {
            // 解除重叠区域的映射
            self.remove_vmas_between(vma.start, vma.end);
            self.get_page_table_mut()
                .unmap(vma.start.into(), vma.end.into());

            let ret = vma.start;
            self.add_vma(vma); // 使用这个以便自动合并 VMA
            return Ok(ret);
        } else {
            // 退化到自动寻址
            let length = vma.end.0 - vma.start.0;

            let address = 'find_place: {
                let mmap_start_idx = self.vmas.partition_point(|v| v.start < MMAP_START);
                let mmap_end_idx = self.vmas.partition_point(|v| v.start < MMAP_END);

                let mmap_vmas = &mut self.vmas[mmap_start_idx..mmap_end_idx];

                for [vma_1, vma_2] in mmap_vmas.array_windows() {
                    if vma_2.start.0 - vma_1.end.0 >= length {
                        break 'find_place vma_1.end;
                    }
                }

                let end = mmap_vmas.last().map_or(MMAP_START, |v| v.end);
                if end.0 + length <= MMAP_END.0 {
                    break 'find_place end;
                } else {
                    return Err(());
                }
            };

            log::trace!(
                "mmap 自动寻址: [{:#x}, {:#x}) ==> [{:#x}, {:#x})",
                vma.start.0,
                vma.end.0,
                address.0,
                address.0 + length
            );

            self.add_vma(VirtualMemoryArea {
                start: address,
                end: VirtAddr(address.0 + length),
                ..vma
            });
            return Ok(address);
        }
    }

    /// 寻找第一个完全位于给定地址之后（.start > addr）的 VMA 索引
    fn find_vma_index_upper_bound(&self, addr: VirtAddr) -> usize {
        self.vmas.partition_point(|vma| vma.start <= addr)
    }
    /// 寻找最后一个完全位于给定地址之前（.end <= addr）的 VMA 索引。注意这样的索引不一定存在
    fn find_vma_index_lower_bound(&self, addr: VirtAddr) -> Option<usize> {
        let first_bigger = self.vmas.partition_point(|vma| vma.end <= addr);

        if first_bigger == 0 {
            None
        } else {
            // 第一个 .end > addr 的，前一个就是最后一个 .end <= addr 的
            Some(first_bigger - 1)
        }
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        for vma in &self.vmas {
            let start = vma.start;
            let end = vma.end;

            self.page_table.unmap(start.into(), end.into());
        }
    }
}
