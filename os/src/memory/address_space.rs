use core::{intrinsics::volatile_set_memory, mem::replace, pin::Pin, sync::atomic::Ordering};

use alloc::vec::Vec;

use crate::{
    basic::shared_with_asm::SharedWithAsm,
    memory::{
        ZERO_PAGE,
        address::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum},
        frame_allocator::FRAME_ALLOCATOR,
        page::{PTEFlags, PageTableEntry},
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
}

impl AddressSpace {
    pub fn new() -> Self {
        Self {
            vmas: Vec::new(),
            page_table: PageTable::new_table(),
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

        if self.vmas[begin].start < start {
            // 第一个 VMA 部分位于范围外

            if self.vmas[begin].end > end {
                // |==============|
                //    |========|
                let ed = replace(&mut self.vmas[begin].end, start);
                self.vmas.insert(
                    begin + 1,
                    VirtualMemoryArea {
                        start: end,
                        end: ed,
                        flags: self.vmas[begin].flags,
                        vma_type: self.vmas[begin].vma_type.clone(),
                        growth: self.vmas[begin].growth,
                    },
                );

                return;
            } else {
                // |=======|
                //    |========|
                self.vmas[begin].end = start;
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
            last.start = end;
        }

        self.vmas.drain(full_remove..last_index);
    }

    pub fn get_page_table(&self) -> &PageTable<2> {
        self.page_table.as_ref().get_ref()
    }

    pub fn get_page_table_mut(&mut self) -> Pin<&mut PageTable<2>> {
        self.page_table.as_mut()
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
                    // TODO: 为唯一引用时写时复制可以不用复制
                    let physical_memory =
                        PhysPageNum(FRAME_ALLOCATOR.lock().alloc(1).expect("无法分配物理页"));

                    let page_guard = unsafe {
                        PAGE_METADATAS
                            .mark_new_page(physical_memory, page_meta::PageType::Anonymous)
                    };

                    // 对于匿名映射，需要清零内容
                    let memory = unsafe { page_guard.visit_memory_mut() };
                    unsafe { volatile_set_memory(memory.as_mut_ptr(), 0, memory.len()) };

                    let asm_handle = unsafe { page_guard.into_asm() };
                    debug_assert!(asm_handle == physical_memory);

                    self.page_table
                        .map(virt_page_num, physical_memory, page_flag)
                        .unwrap_or_else(|old_entry| {
                            // 写时复制
                            debug_assert_eq!(fault_type, PageFaultType::Write);
                            debug_assert!(!old_entry.flags().contains(PTEFlags::Write));
                            debug_assert_eq!(
                                PhysAddr::from(PhysPageNum::from_pte(old_entry))
                                    .to_virtual_address(),
                                VirtAddr::from_ptr(&ZERO_PAGE)
                            );

                            *old_entry = PageTableEntry::new(
                                physical_memory,
                                old_entry.flags() | PTEFlags::Write,
                            );

                            // 没有必要释放旧页，反正它总是全局零页
                        });
                }
            }
            VMAType::File { file, page_index } => 'handle: {
                if !vma.flags.contains(VMAFlags::Shared) {
                    // 私有映射
                    let old_entry = self.page_table.as_mut().at_mut(virt_page_num);

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

        cfg_select! {
            target_arch = "riscv64" => riscv::asm::sfence_vma_all(), // TODO: 避免全量刷新
            target_arch = "loongarch64" => unsafe { core::arch::asm!("invtlb 0x0, $zero, $zero") }, // TODO: 避免全量刷新
        }

        Ok(())
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

            unsafe { self.page_table.as_mut().get_unchecked_mut() }.unmap(start.into(), end.into());
        }
    }
}
