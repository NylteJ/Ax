use core::{marker::PhantomPinned, mem::offset_of, ops::Deref, pin::Pin, ptr::addr_eq};

use alloc::sync::Arc;
use elf::{ElfBytes, endian::NativeEndian};

use crate::{
    basic::{shared_with_asm::SharedWithAsm, stack::TaskKernelStackPtr, sync::RwLock},
    filesystem::vfs::{File, FileArc, Path},
    memory::{
        PAGE_BYTES, STACK_ALIGN,
        address::{VIRTUAL_ADDRESS_LOW_HIGHEST, VirtAddr},
        address_space::AddressSpace,
        uaccess::fill_user_memory,
        virtual_memory_area::{VMAFlags, VMAGrowth, VMAType, VirtualMemoryArea},
    },
    multitask::{
        SchedulePolicy, TaskPriority, current_task,
        fdtable::FileDescriptorTable,
        fs_data::FilesystemData,
        task_context::{TaskContext, switch_context},
        task_manager::{PID_ALLOCATOR, register_task},
        task_scheduler::TASK_SCHEDULER,
    },
    trap::trap_handler::{TrapContext, return_to_user, trap_handler_raw_from_user},
};

/// 当前任务的所有元数据。拥有任务所有资源的所有权。
///
/// 在内核态下，tp / $tp 应当始终指向它。
#[derive(Debug)]
pub struct Task {
    /// 任务的内核栈指针
    ///
    /// 注意：它只在任务挂起时保证精确！
    pub kstack: TaskKernelStackPtr,

    pub pid: usize,

    pub policy: SchedulePolicy,
    pub priority: TaskPriority,

    pub address_space: Arc<RwLock<AddressSpace>>,

    pub files: Arc<RwLock<FileDescriptorTable>>,
    pub fs_data: Arc<RwLock<FilesystemData>>,

    _pin: PhantomPinned, // Task 的地址需要在 tp、内核栈等区域存储，因此不能移动
}

pub type TaskArc = Pin<Arc<Task>>;

pub const INIT_PID: usize = 0;

const KERNEL_STACK_LAYOUT: alloc::alloc::Layout = unsafe {
    alloc::alloc::Layout::from_size_align(TaskKernelStackPtr::size(), TaskKernelStackPtr::align())
        .unwrap_unchecked()
};

const USER_STACK_HIGHEST: usize = VIRTUAL_ADDRESS_LOW_HIGHEST + 1;

impl Task {
    /// 执行 init 进程。此调用不会返回。
    ///
    /// 和一般进程不同，init 进程直接由内核创建、PID 固定为 [`INIT_PID`] 且永不终止。内核会屏蔽向其发出的所有信号。
    pub fn execute_init(
        policy: SchedulePolicy,
        priority: TaskPriority,
        file: FileArc,
        work_dir: Path,
    ) -> ! {
        let success = PID_ALLOCATOR.lock().allocate_specific(INIT_PID);
        assert!(success, "必须能够为 init 进程分配 PID {INIT_PID}");

        let (kstack, task_ptr_ptr) = Self::alloc_kstack();
        let kstack_sp = (task_ptr_ptr as usize) / STACK_ALIGN * STACK_ALIGN;

        let address_space = Self::new_addr_space();

        // 激活用户态页表（必须提前激活，因为后面可能要往用户空间里写一堆东西）
        address_space.get_page_table().activitate();

        // 先创建一个半成品 task
        let task = Arc::pin(Self {
            kstack,
            pid: INIT_PID,

            policy,
            priority,

            address_space: Arc::new(RwLock::new(address_space)),
            files: Arc::new(RwLock::new(FileDescriptorTable::new())),

            // FIXME: 后面应该要改
            fs_data: Arc::new(RwLock::new(FilesystemData {
                current_work_dir: work_dir,
            })),

            _pin: PhantomPinned,
        });

        // 将 task 地址写入内核栈（注意我们现在不在这个内核栈上）
        unsafe { task_ptr_ptr.write(task.deref()) };
        // 然后，将其写入 tp
        unsafe { Self::set_current_task(task.clone().into_asm()) };

        // 现在我们已经设定好了任务上下文

        // 注意这里不能改为传入 &mut AddressSpace，因为内部它可能触发缺页异常，缺页异常处理时要获取 AddressSpace 的写权限，
        // 因此传入大粒度的可变引用会导致死锁或违反别名规则。
        let entry = Self::load_elf(&file, &task.address_space);

        register_task(task);

        // 至此，task 终于完全初始化完毕了！

        // FIXME
        use core::arch::asm;
        cfg_select! {
            target_arch = "riscv64" => unsafe {
                use riscv::register::sstatus;

                sstatus::set_spp(sstatus::SPP::User);

                asm!(
                    "csrw stvec, {trap_handler_raw_from_user}",
                    "csrw sscratch, {stack}",
                    "csrw sepc, {entry}",
                    "csrrw sp, sscratch, {kstack}",
                    "sret",
                    trap_handler_raw_from_user = in(reg) trap_handler_raw_from_user,
                    stack = in(reg) USER_STACK_HIGHEST,
                    entry = in(reg) entry,
                    kstack = in(reg) kstack_sp,
                    options(noreturn),
                );
            },
            target_arch = "loongarch64" => unsafe {
                use bit_field::BitField;

                let mut prmd: usize;
                asm!("csrrd {0}, 0x0", out(reg) prmd); // CRMD
                prmd.set_bits(0..2, 3);
                prmd.set_bit(2, true);
                asm!("csrwr {0}, 0x1", in(reg) prmd);

                asm!(
                    "csrwr {trap_handler_raw_from_user}, 0xc",
                    "csrwr {entry}, 0x6",
                    "csrwr {kstack}, 0x30",
                    "move $sp, {stack}",
                    "ertn",
                    trap_handler_raw_from_user = in(reg) trap_handler_raw_from_user,
                    entry = in(reg) entry,
                    stack = in(reg) USER_STACK_HIGHEST,
                    kstack = in(reg) kstack_sp,
                    options(noreturn),
                );
            },
        }
    }

    /// 复制一份当前任务，并将其添加到调度器中，返回新任务。
    ///
    /// 注意此函数在内核端就是普通函数，返回用户态后新任务才可能被执行。
    pub fn fork(&self) -> TaskArc {
        // 对于内核栈，必须分配新的
        let (_kstack, task_ptr_ptr) = Self::alloc_kstack();
        let kstack_sp = (task_ptr_ptr as usize) / STACK_ALIGN * STACK_ALIGN;
        // 而且必须压入完全一致的 TrapContext 并伪造 TaskContext
        let ret_trap_ctxt_ptr = (kstack_sp - size_of::<TrapContext>()) as *mut TrapContext;
        unsafe { ret_trap_ctxt_ptr.write(self.get_trap_context().read()) };
        let ret_task_ctxt_ptr =
            (ret_trap_ctxt_ptr.addr() - size_of::<TaskContext>()) as *mut TaskContext;
        let kstack_sp = ret_task_ctxt_ptr as usize;
        unsafe {
            ret_task_ctxt_ptr
                .byte_add(offset_of!(TaskContext, return_address))
                .cast::<usize>()
                .write(return_to_user as usize);
            ret_task_ctxt_ptr
                .byte_add(offset_of!(TaskContext, stack_pointer))
                .cast::<usize>()
                .write(kstack_sp);

            // 我们还需要初始化 page_table_register，这个需要在后面写入

            // 剩余部分保持未初始化即可，它们完全不重要
        }
        // 于是相对应的，kstack 也要改变，以伪造出该 Task 已经挂起的场景
        let kstack = unsafe { TaskKernelStackPtr::from_addr(kstack_sp) };

        // PID 也要分配新的
        let pid = PID_ALLOCATOR
            .lock()
            .allocate()
            .expect("必须要能为新任务分配 PID（TODO）");

        // 这两个直接复制即可
        let policy = self.policy;
        let priority = self.priority;

        // 地址空间需要复制一份，并将所有页设为写时复制
        let address_space = Arc::new(RwLock::new(self.address_space.write().clone_cow()));
        // 由于所有页被设为了写时复制，本进程的页表很可能也已经改变，因此需要刷新 TLB
        cfg_select! {
            target_arch = "riscv64" => riscv::asm::sfence_vma_all(),
            target_arch = "loongarch64" => unsafe { core::arch::asm!("invtlb 0x0, $zero, $zero") },
        }

        // 写入之前没写入的 page_table_register
        unsafe {
            ret_task_ctxt_ptr
                .byte_add(offset_of!(TaskContext, page_table_register))
                .cast::<usize>()
                .write(address_space.read().get_page_table().to_native_register());
        }

        // 这两个都可以直接浅拷贝
        let files = Arc::new(RwLock::new(self.files.read().clone()));
        let fs_data = Arc::new(RwLock::new(self.fs_data.read().clone()));

        let ret = Arc::pin(Self {
            kstack,
            pid,

            policy,
            priority,

            address_space,
            files,
            fs_data,

            _pin: PhantomPinned,
        });

        unsafe { task_ptr_ptr.write(ret.deref()) };

        register_task(ret.clone());

        TASK_SCHEDULER.lock().add_task(ret.clone());

        log::trace!("forked: {} => {pid}", self.pid);

        ret
    }

    /// 将当前任务替换为 `file` 对应可执行文件的内容。
    ///
    /// 注意此函数在内核端就是普通函数，返回用户态后替换后的任务才可能被执行。
    pub fn replace_execute(&self, file: FileArc) {
        // 创建新的地址空间，并替换当前地址空间
        {
            let mut my_addr_space = self.address_space.write();
            *my_addr_space = Self::new_addr_space();
            my_addr_space.get_page_table().activitate();
        }

        // 加载 ELF 文件并获取入口地址
        let entry = Self::load_elf(&file, &self.address_space);

        // 将入口地址和栈指针写入到 TrapContext 中
        // 特别注意，和前面几个函数不同，我们并没有更换内核栈，因此不需要伪造任何东西，只要修改 ret_pc 和 sp 即可
        // （剩余寄存器的值并不重要）
        let trap_context = unsafe { self.get_trap_context().cast_mut().as_mut_unchecked() };
        trap_context.ret_pc = entry;
        trap_context.registers[TrapContext::STACK_POINTER] = USER_STACK_HIGHEST;

        // OK，这样就可以了！
    }

    pub unsafe fn set_current_task(task_ptr: *const Task) {
        use core::arch::asm;

        cfg_select! {
            target_arch = "riscv64" =>     unsafe { asm!("mv    tp, {0}", in(reg) task_ptr) },
            target_arch = "loongarch64" => unsafe { asm!("move $tp, {0}", in(reg) task_ptr) },
        }
    }

    /// 暂时切换到此任务。
    ///
    /// 通常情况下（当前任务还在调度器中），此调用迟早会正常返回，但当前任务的确可能要在很久以后才能被调度到，因此请尽可能释放不必要的资源
    pub fn switch_to(self: TaskArc) {
        if addr_eq(self.deref(), current_task()) {
            // 仍然切换到当前任务，直接返回即可。
            // 注意这不仅是优化，也是正确性所必需的：下面的 switch_context 无法处理这种情况。
            return;
        }

        let context = self.get_context();
        unsafe { self.into_asm() };
        unsafe { switch_context(context.as_ref_unchecked()) };
    }

    fn alloc_kstack() -> (TaskKernelStackPtr, *mut *const Task) {
        let kstack_btm = unsafe { alloc::alloc::alloc(KERNEL_STACK_LAYOUT) };
        if kstack_btm.is_null() {
            panic!("无法为 Task 分配内核栈");
        }

        let kstack_btm = kstack_btm as usize;
        let kstack_top = kstack_btm + TaskKernelStackPtr::size();

        // SAFETY: 这就是内核栈的下边界
        let kstack = unsafe { TaskKernelStackPtr::from_addr(kstack_btm) };

        let task_ptr_addr = kstack_top - size_of::<*const ()>();

        (kstack, task_ptr_addr as *mut *const Task)
    }

    /// 分配新的 [`AddressSpace`]，并在其中填充好用户栈区的 VMA
    fn new_addr_space() -> AddressSpace {
        let mut ret = AddressSpace::new();

        const USER_STACK_BYTES: usize = 2048 * PAGE_BYTES;

        static_assertions::const_assert!(USER_STACK_BYTES % PAGE_BYTES == 0);
        static_assertions::const_assert!(USER_STACK_BYTES > PAGE_BYTES);

        ret.add_vma(VirtualMemoryArea {
            start: VirtAddr(USER_STACK_HIGHEST - PAGE_BYTES),
            end: VirtAddr(USER_STACK_HIGHEST),
            flags: VMAFlags::Read | VMAFlags::Write | VMAFlags::User,
            vma_type: VMAType::Anonymous,
            growth: VMAGrowth::GrowDown(VirtAddr(USER_STACK_HIGHEST - USER_STACK_BYTES)),
        });

        ret
    }

    fn load_elf(file: &FileArc, address_space: &RwLock<AddressSpace>) -> usize {
        let head = unsafe { File::kernel_read_page(file.get_inode(), 0) }.unwrap();

        let elf = ElfBytes::<NativeEndian>::minimal_parse_no_section_headers(head).unwrap();

        let segs = elf.segments().unwrap();
        for seg in segs {
            use elf::abi::{PT_DYNAMIC, PT_GNU_STACK, PT_LOAD, PT_NULL};

            match seg.p_type {
                PT_NULL => (),
                PT_LOAD => {
                    const PAGE_BYTES: u64 = crate::memory::PAGE_BYTES as u64;

                    if seg.p_align < PAGE_BYTES
                        || seg.p_align % PAGE_BYTES != 0
                        || !seg.p_align.is_power_of_two()
                    {
                        log::error!(
                            "ELF 的 P_LOAD 段的 p_align（{}）错误，无法加载",
                            seg.p_align
                        );
                        todo!("错误处理");
                    }

                    let inseg_offset = seg.p_offset % seg.p_align;
                    let inpage_offset = seg.p_offset % PAGE_BYTES;

                    if seg.p_vaddr % seg.p_align != inseg_offset {
                        log::error!(
                            "ELF 的 P_LOAD 段的 p_vaddr（{:#x}）和 p_offset（{:#x}）不满足分页对齐要求，无法加载",
                            seg.p_vaddr,
                            seg.p_offset
                        );
                        todo!("错误处理");
                    }

                    // 注意 inseg_offset 相等也推出了 inpage_offset 相等（读者自证不难）
                    // 此外，后面其实不用考虑 inseg_offset，因为我们的加载等操作还是按页为单位来的

                    let vaddr = seg.p_vaddr - inpage_offset;
                    let off = seg.p_offset - inpage_offset;
                    let f_len = seg.p_filesz + inpage_offset;
                    let m_len = seg.p_memsz + inpage_offset;

                    let flags = {
                        use elf::abi::{PF_R, PF_W, PF_X};

                        let mut flags = VMAFlags::User;

                        if seg.p_flags & PF_X != 0 {
                            flags |= VMAFlags::Execute;
                        }
                        if seg.p_flags & PF_R != 0 {
                            flags |= VMAFlags::Read;
                        }
                        if seg.p_flags & PF_W != 0 {
                            flags |= VMAFlags::Write;
                        }

                        flags
                    };

                    // TODO: 约束对应 ELF 文件不可被修改

                    let file_start = VirtAddr(vaddr as usize);
                    let file_end = VirtAddr((vaddr + f_len) as usize).ceil_to_page();
                    let zeros_end = VirtAddr((vaddr + m_len) as usize).ceil_to_page();

                    if seg.p_filesz == 0 {
                        // 这个时候不需要往下进行（因为对齐，往下进行有可能最后会加一块文件映射然后再写入，实际上完全没必要），
                        // 直接映射匿名段即可

                        address_space.write().add_vma(VirtualMemoryArea {
                            start: file_start,
                            end: zeros_end,
                            flags,
                            vma_type: VMAType::Anonymous,
                            growth: VMAGrowth::Fixed,
                        });

                        continue;
                    }

                    address_space.write().add_vma(VirtualMemoryArea {
                        start: file_start,
                        end: file_end,
                        flags,
                        vma_type: VMAType::File {
                            file: file.clone(),
                            page_index: off / PAGE_BYTES,
                        },
                        growth: VMAGrowth::Fixed,
                    });

                    if m_len > f_len {
                        if let tail = f_len % PAGE_BYTES
                            && tail != 0
                        {
                            let rest = (PAGE_BYTES - tail) as usize;
                            let ptr = (vaddr + f_len) as *mut u8;
                            fill_user_memory(ptr, rest, 0).unwrap();
                        }

                        address_space.write().add_vma(VirtualMemoryArea {
                            start: file_end,
                            end: zeros_end,
                            flags,
                            vma_type: VMAType::Anonymous,
                            growth: VMAGrowth::Fixed,
                        });
                    }
                }
                PT_DYNAMIC => {
                    log::error!("暂不支持动态链接的程序");
                    todo!("错误处理");
                }
                PT_GNU_STACK => {
                    log::info!(
                        "发现 PT_GNU_STACK 段，然而，我们的栈区本就不可执行，因此无需进一步操作"
                    );
                }
                other => log::warn!("未知 p_type: {other:#x}，已跳过"),
            }
        }

        elf.ehdr.e_entry as usize
    }

    /// 获取 TaskContext
    ///
    /// # Safety
    /// 此函数本身安全，但当前任务不处于挂起状态时其返回值不能被安全使用
    pub fn get_context(&self) -> *const TaskContext {
        self.kstack.get_addr() as _
    }
    /// 获取 TrapContext
    ///
    /// # Safety
    /// 此函数本身安全，但当前任务不处于内核态时其返回值不能被安全使用
    pub fn get_trap_context(&self) -> *const TrapContext {
        let misaligned_top = self.kstack.get_high_addr() - size_of::<*const TaskContext>();
        let top = misaligned_top / STACK_ALIGN * STACK_ALIGN;
        (top - size_of::<TrapContext>()) as *const TrapContext
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        unsafe {
            alloc::alloc::dealloc(self.kstack.get_low_addr() as *mut u8, KERNEL_STACK_LAYOUT)
        };

        PID_ALLOCATOR.lock().deallocate(self.pid);

        crate::println!("Task {} dropped", self.pid);
    }
}
