use core::{
    marker::PhantomPinned,
    mem::{offset_of, replace},
    num::NonZero,
    ops::Deref,
    pin::Pin,
    ptr::{NonNull, addr_eq},
};

use alloc::{sync::Arc, vec::Vec};
use elf::{ElfBytes, endian::NativeEndian};
use intrusive_collections::{LinkedListLink, intrusive_adapter};

use crate::{
    arch::{
        self,
        interfaces::{Basic, Regs, Trap, TrapContext},
    },
    basic::{
        DropGuard, UnsafeInited,
        shared_with_asm::SharedWithAsm,
        stack::TaskKernelStackPtr,
        sync::{Mutex, RwLock},
        timer::get_cpu_tick,
    },
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
        process_data::ProcessData,
        task_context::{TaskContext, switch_context},
        task_manager::{PID_ALLOCATOR, register_task},
        task_scheduler::TASK_SCHEDULER,
    },
    syscall::process::CloneFlags,
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

    pub tid: usize,

    pub policy: SchedulePolicy,
    pub priority: TaskPriority,

    pub address_space: Arc<RwLock<AddressSpace>>,

    pub files: Arc<RwLock<FileDescriptorTable>>,
    pub fs_data: Arc<RwLock<FilesystemData>>,

    pub process: Arc<ProcessData>,

    /// 子 Task 链表头
    pub children: Mutex<intrusive_collections::LinkedList<TaskAdapter>>,
    /// 子 Task 链表的侵入式节点，由于所有操作都须借助 children 的锁，它本身无需额外同步
    children_link: LinkedListLink,
    /// 父 Task 指针，通常不会变化
    ///
    /// 通过托孤机制保证总是有效（如果父节点提前死亡，则其改为指向 init）
    pub parent: RwLock<NonNull<Task>>,

    pub wait_state: Mutex<ZombieWaitingState>,

    pub time_stats: Mutex<TaskTimeStats>,

    _pin: PhantomPinned, // Task 的地址需要在 tp、内核栈等区域存储，因此不能移动
}

#[derive(Debug)]
pub struct ZombieWaitingState {
    /// Task 是否已经退出，如果有，则内部存放退出状态 (信号, 返回值)
    pub status: Option<(u8, u8)>,
    /// 是否有父 Task 正在等待当前 Task，如果有，则内部存放用于唤醒等待者的句柄
    // （我们目前根本不支持睡眠，所以其实没什么能放的）
    pub waiter: Option<()>,
}

#[derive(Debug)]
pub struct TaskTimeStats {
    /// 当前 Task 在用户态下运行的周期数
    pub user_time: usize,
    /// 当前 Task 在内核态下运行的周期数
    pub kernel_time: usize,
    /// 当前 Task 的所有已终止且成功被 wait 的子 Task 在用户态下运行的周期数之和
    pub children_user_time: usize,
    /// 当前 Task 的所有已终止且成功被 wait 的子 Task 在内核态下运行的周期数之和
    pub children_kernel_time: usize,

    /// 上次更新时的时间戳（周期数）
    last_update_stamp: usize,
}

impl TaskTimeStats {
    pub fn new() -> Self {
        Self {
            user_time: 0,
            kernel_time: 0,
            children_user_time: 0,
            children_kernel_time: 0,
            last_update_stamp: get_cpu_tick(),
        }
    }

    pub fn update_kernel_time(&mut self) {
        let now = get_cpu_tick();
        let delta = now - self.last_update_stamp;
        self.kernel_time += delta;
        self.last_update_stamp = now;
    }
    pub fn update_user_time(&mut self) {
        let now = get_cpu_tick();
        let delta = now - self.last_update_stamp;
        self.user_time += delta;
        self.last_update_stamp = now;
    }

    pub fn update_children_times(&mut self, children: &Self) {
        self.children_user_time += children.user_time;
        self.children_kernel_time += children.kernel_time;
    }
}

unsafe impl Send for Task {}
unsafe impl Sync for Task {}

intrusive_adapter!(pub TaskAdapter = TaskArc: Task { children_link => LinkedListLink });

pub type TaskArc = Pin<Arc<Task>>;

pub const INIT_PID: usize = 0;
pub static INIT_TASK: UnsafeInited<TaskArc> = unsafe { UnsafeInited::uninit() };

const KERNEL_STACK_LAYOUT: alloc::alloc::Layout = unsafe {
    alloc::alloc::Layout::from_size_align_unchecked(
        TaskKernelStackPtr::size(),
        TaskKernelStackPtr::align(),
    )
};

const USER_STACK_HIGHEST: usize = VIRTUAL_ADDRESS_LOW_HIGHEST + 1;

pub const USER_STACK_BYTES: usize = 2048 * PAGE_BYTES;

static_assertions::const_assert!(USER_STACK_BYTES % PAGE_BYTES == 0);
static_assertions::const_assert!(USER_STACK_BYTES > PAGE_BYTES);

impl Task {
    /// 执行 init 进程。此调用不会返回。
    ///
    /// 和一般进程不同，init 进程直接由内核创建、PID 固定为 [`INIT_PID`] 且永不终止。内核会屏蔽向其发出的所有信号。
    ///
    /// 此函数不会向 init 进程传递任何参数。（FIXME？）
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
            tid: INIT_PID,

            policy,
            priority,

            address_space: Arc::new(RwLock::new(address_space)),
            files: Arc::new(RwLock::new(FileDescriptorTable::new())),

            // FIXME: 后面应该要改
            fs_data: Arc::new(RwLock::new(FilesystemData {
                current_work_dir: work_dir,
            })),

            process: Arc::new(ProcessData::new(INIT_PID, INIT_PID)),

            children: Mutex::new(intrusive_collections::LinkedList::new(TaskAdapter::new())),
            children_link: LinkedListLink::new(),
            parent: RwLock::new(NonNull::dangling()), // init 进程没有父进程，先放一个悬空指针，后面改为指向自己

            wait_state: Mutex::new(ZombieWaitingState {
                status: None,
                waiter: None,
            }),
            time_stats: Mutex::new(TaskTimeStats::new()),

            _pin: PhantomPinned,
        });

        // 填充 parent
        unsafe { task.parent.as_ptr().write(NonNull::from_ref(&task)) };

        // 将 task 地址写入内核栈（注意我们现在不在这个内核栈上）
        unsafe { task_ptr_ptr.write(task.deref()) };
        // 然后，将其写入 tp
        unsafe { Self::set_current_task(task.clone().into_asm()) };

        // 现在我们已经设定好了任务上下文

        // 注意这里不能改为传入 &mut AddressSpace，因为内部它可能触发缺页异常，缺页异常处理时要获取 AddressSpace 的写权限，
        // 因此传入大粒度的可变引用会导致死锁或违反别名规则。
        let entry = Self::load_elf(&file, &task.address_space).unwrap().0;

        register_task(task.clone());
        unsafe { UnsafeInited::init_unsafe(&INIT_TASK, task) };

        // 至此，task 终于完全初始化完毕了！

        unsafe { arch::NativeBasic::execute_init(entry, USER_STACK_HIGHEST, kstack_sp) };
    }

    /// 复制一份当前任务，并将其添加到调度器中，返回新任务。子任务的返回值（a0）会被篡改为 0。
    ///
    /// 注意此函数在内核端就是普通函数，返回用户态后新任务才可能被执行。
    pub fn fork(&self, flags: CloneFlags, ustack: Option<NonZero<usize>>) -> TaskArc {
        // 对于内核栈，必须分配新的
        let (_kstack, task_ptr_ptr) = Self::alloc_kstack();
        let kstack_sp = (task_ptr_ptr as usize) / STACK_ALIGN * STACK_ALIGN;
        // 而且必须压入（几乎）完全一致的 TrapContext 并伪造 TaskContext
        let ret_trap_ctxt_addr = kstack_sp - size_of::<arch::NativeTrapContext>();
        let ret_trap_ctxt_ptr = ret_trap_ctxt_addr as *mut arch::NativeTrapContext;
        let ret_trap_ctxt = unsafe {
            ret_trap_ctxt_ptr
                .as_uninit_mut()
                .unwrap_unchecked()
                .write(self.get_trap_context().read())
        };
        if let Some(ustack) = ustack {
            // 此时还要手动修改用户栈指针
            ret_trap_ctxt.regs.sp = ustack.get();
        }
        // 别忘了修改其返回值
        *ret_trap_ctxt.ret_reg_mut() = 0;
        // 接下来是 TaskContext
        let ret_task_ctxt_ptr =
            (ret_trap_ctxt_ptr.addr() - size_of::<TaskContext>()) as *mut TaskContext;
        let kstack_sp = ret_task_ctxt_ptr as usize;
        unsafe {
            ret_task_ctxt_ptr
                .byte_add(offset_of!(TaskContext, return_address))
                .cast::<usize>()
                .write(arch::NativeTrap::return_to_user as usize);
            ret_task_ctxt_ptr
                .byte_add(offset_of!(TaskContext, stack_pointer))
                .cast::<usize>()
                .write(kstack_sp);

            // 我们还需要初始化 page_table_register，这个需要在后面写入

            // 剩余部分保持未初始化即可，它们会被 TrapContext 覆盖因此完全不重要
        }
        // 于是相对应的，kstack 也要改变，以伪造出该 Task 已经挂起的场景
        let kstack = unsafe { TaskKernelStackPtr::from_addr(kstack_sp) };

        // TID 也要分配新的
        let tid = PID_ALLOCATOR
            .lock()
            .allocate()
            .expect("必须要能为新任务分配 PID（TODO）");

        // 这两个直接复制即可
        let policy = self.policy;
        let priority = self.priority;

        let address_space = if flags.contains(CloneFlags::VM) {
            // 浅拷贝一份 Arc 即可
            self.address_space.clone()
        } else {
            // 地址空间需要复制一份，并将所有页设为写时复制
            let address_space = Arc::new(RwLock::new(self.address_space.write().clone_cow()));
            // 由于所有页被设为了写时复制，本进程的页表很可能也已经改变，因此需要刷新 TLB
            arch::NativeBasic::tlb_invalidate_all();
            address_space
        };

        // 写入之前没写入的 page_table_register
        unsafe {
            ret_task_ctxt_ptr
                .byte_add(offset_of!(TaskContext, page_table_register))
                .cast::<usize>()
                .write(address_space.read().get_page_table().to_native_register());
        }

        let files = if flags.contains(CloneFlags::FILES) {
            // 共享同一个文件描述符表
            self.files.clone()
        } else {
            // 使用初始时相等的两个文件描述符表
            Arc::new(RwLock::new(self.files.read().clone()))
        };

        let fs_data = if flags.contains(CloneFlags::FS) {
            // 共享文件系统信息（文件系统的根目录、当前工作目录和 umask 值）
            self.fs_data.clone()
        } else {
            // 使用初始时相等的两个文件系统信息
            Arc::new(RwLock::new(self.fs_data.read().clone()))
        };

        let process = if flags.contains(CloneFlags::THREAD) {
            // 线程，放在同一个进程（线程组）中，浅拷贝即可
            self.process.clone()
        } else {
            // 进程，直接新建一个进程（线程组），注意仍在同一个进程组中
            Arc::new(ProcessData::new(tid, self.process.pgid))
        };

        let parent = if flags.contains(CloneFlags::PARENT) {
            // 克隆父指针，父 Task 和自身相同
            self.parent_task()
        } else {
            // 不克隆父指针，父 Task 设为自身
            self
        };

        let ret = Arc::pin(Self {
            kstack,
            tid,

            policy,
            priority,

            address_space,
            files,
            fs_data,

            process,

            // 这几个不应当复制
            children: Mutex::new(intrusive_collections::LinkedList::new(TaskAdapter::new())),
            children_link: LinkedListLink::new(),
            parent: RwLock::new(NonNull::from_ref(parent)),

            wait_state: Mutex::new(ZombieWaitingState {
                status: None,
                waiter: None,
            }),
            time_stats: Mutex::new(TaskTimeStats::new()),

            _pin: PhantomPinned,
        });

        if !flags.contains(CloneFlags::THREAD) {
            parent.children.lock().push_back(ret.clone());
        }

        unsafe { task_ptr_ptr.write(ret.deref()) };

        register_task(ret.clone());

        TASK_SCHEDULER.lock().add_task(ret.clone());

        log::trace!("forked: {} => {tid}", self.tid);

        ret
    }

    /// 将当前任务替换为 `file` 对应可执行文件的内容。不传递任何参数。返回辅助向量。
    ///
    /// 注意此函数在内核端就是普通函数，返回用户态后替换后的任务才可能被执行。
    pub fn replace_execute(&self, file: FileArc) -> Result<AuxiliaryVec, LoadElfError> {
        // 创建新的地址空间，并替换当前地址空间

        let old_addr_space = replace(&mut *self.address_space.write(), Self::new_addr_space());
        self.address_space.read().get_page_table().activitate();

        let guard = DropGuard::new(|| {
            old_addr_space.get_page_table().activitate();
            *self.address_space.write() = old_addr_space;
        });

        // 加载 ELF 文件并获取入口地址
        let (entry, auxv) = Self::load_elf(&file, &self.address_space)?;

        // 将入口地址和栈指针写入到 TrapContext 中
        // 特别注意，和前面几个函数不同，我们并没有更换内核栈，因此不需要伪造任何东西，只要修改 ret_pc 和 sp 即可
        // （剩余寄存器的值并不重要）
        let trap_context = unsafe { self.get_trap_context().cast_mut().as_mut_unchecked() };
        trap_context.ret_pc = entry;
        trap_context.regs.sp = USER_STACK_HIGHEST;

        // 关闭标记为 CLOEXEC 的文件描述符
        self.files.write().close_on_exec();

        // OK，这样就可以了！
        let _ = DropGuard::release(guard);
        Ok(auxv)
    }

    pub unsafe fn set_current_task(task_ptr: *const Task) {
        arch::NativeRegs::set_tp(task_ptr.addr());
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

        // 现在我们被重新调度了

        // 重新设置 last_update_stamp
        let cur_task = unsafe { current_task().as_ref_unchecked() };
        cur_task.time_stats.lock().last_update_stamp = get_cpu_tick();
    }

    /// 获取父 Task 的引用
    pub fn parent_task(&self) -> &Task {
        unsafe { self.parent.read().as_ref() }
    }

    /// 当此 Task 退出时调用，若父进程还未 wait 则回收部分资源并将自身标记为僵尸状态，否则唤醒父进程且不做额外操作
    pub fn on_exit(&self, signal: u8, return_value: u8) {
        let mut wait_state = self.wait_state.lock();

        let old = wait_state.status.replace((signal, return_value));
        debug_assert!(old.is_none());

        if let Some(waiter) = wait_state.waiter {
            let _ = waiter; // 假装我们唤醒了等待中的父 Task
        } else {
            // *self.address_space.write() = AddressSpace::new();
            // *self.files.write() = FileDescriptorTable::new();
            self.transfer_children_to_init();
        }
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

        ret.add_vma(VirtualMemoryArea {
            start: VirtAddr(USER_STACK_HIGHEST - PAGE_BYTES),
            end: VirtAddr(USER_STACK_HIGHEST),
            flags: VMAFlags::Read | VMAFlags::Write | VMAFlags::User,
            vma_type: VMAType::Anonymous,
            growth: VMAGrowth::GrowDown(VirtAddr(USER_STACK_HIGHEST - USER_STACK_BYTES)),
        });

        ret
    }

    /// 从 ELF 文件中加载程序，填入对应地址空间中，并返回入口地址和辅助向量
    ///
    /// 它也负责设置初始堆区 VMA 和 brk。
    /// （和 Linux 不同，我们初始时就有一个很短的堆区 VMA，因为反正虚拟地址多得用不完，这样只会掩盖少量用户态越界访问）
    fn load_elf(
        file: &FileArc,
        address_space: &RwLock<AddressSpace>,
    ) -> Result<(usize, AuxiliaryVec), LoadElfError> {
        use aux_ids::*;

        let head = unsafe { File::kernel_read_page(file.get_inode(), 0) }.unwrap();

        let elf =
            ElfBytes::<NativeEndian>::minimal_parse_no_section_headers(head).map_err(|err| {
                match err {
                    err @ elf::ParseError::SliceReadError(..) => todo!("超大 ELF 头({err:?})"),
                    _ => LoadElfError::BadElfHead,
                }
            })?;

        #[cfg(target_arch = "riscv64")]
        const EXPECTED_MACHINE: u16 = elf::abi::EM_RISCV;
        #[cfg(target_arch = "loongarch64")]
        const EXPECTED_MACHINE: u16 = elf::abi::EM_LOONGARCH;

        if elf.ehdr.e_machine != EXPECTED_MACHINE {
            log::error!(
                "ELF 文件的 e_machine 字段不匹配（应当为 {EXPECTED_MACHINE:#x}，得到 {:#x}），无法加载",
                elf.ehdr.e_machine
            );
            return Err(LoadElfError::ArchMismatch);
        }

        let mut auxv = Vec::new();

        let mut max_load_seg_end_addr = None;

        let segs = elf.segments().unwrap();
        for seg in segs {
            use elf::abi::{PT_DYNAMIC, PT_GNU_STACK, PT_INTERP, PT_LOAD, PT_NULL};

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
                        return Err(LoadElfError::BadAlignment);
                    }

                    let inseg_offset = seg.p_offset % seg.p_align;
                    let inpage_offset = seg.p_offset % PAGE_BYTES;

                    if seg.p_vaddr % seg.p_align != inseg_offset {
                        log::error!(
                            "ELF 的 P_LOAD 段的 p_vaddr（{:#x}）和 p_offset（{:#x}）不满足分页对齐要求，无法加载",
                            seg.p_vaddr,
                            seg.p_offset
                        );
                        return Err(LoadElfError::BadAlignment);
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

                    max_load_seg_end_addr.replace(zeros_end); // ELF 标准规定 PT_LOAD 段必须按地址升序排列
                }
                PT_DYNAMIC | PT_INTERP => {
                    log::error!("暂不支持动态链接的程序");
                    log::warn!(
                        "由于 basic 测试全部是动态链接的静态程序，暂时强行将此程序当作静态链接的处理"
                    );
                    // return Err(LoadElfError::NotYetImplemented); // FIXME
                }
                PT_GNU_STACK => {
                    log::info!(
                        "发现 PT_GNU_STACK 段，然而，我们的栈区本就不可执行，因此无需进一步操作"
                    );
                }
                other => log::warn!("未知 p_type: {other:#x}，已跳过"),
            }
        }

        // 初始化堆区 VMA 和 brk
        {
            let Some(program_data_end) = max_load_seg_end_addr else {
                log::error!("ELF 文件没有有效的 PT_LOAD 段，无法加载");
                return Err(LoadElfError::NoLoadSegment);
            };

            debug_assert_eq!(program_data_end.0 % PAGE_BYTES, 0);

            let init_brk = VirtAddr(program_data_end.0 + PAGE_BYTES);

            let mut addr_space = address_space.write();

            addr_space.brk = init_brk;
            addr_space.add_vma(VirtualMemoryArea {
                start: program_data_end,
                end: init_brk,
                flags: VMAFlags::Read | VMAFlags::Write | VMAFlags::User,
                vma_type: VMAType::Anonymous,
                growth: VMAGrowth::Fixed,
            });
        }

        let entry = elf.ehdr.e_entry as usize;

        auxv.push(AuxiliaryEntry(AT_ENTRY, entry));
        auxv.push(AuxiliaryEntry(AT_PAGESZ, PAGE_BYTES));

        auxv.push(AuxiliaryEntry(AT_NULL, 0));
        Ok((entry, auxv))
    }

    fn transfer_children_to_init(&self) {
        let my_children = self.children.lock().take();
        if !my_children.is_empty() {
            log::trace!(
                "TID 为 {} 的 Task 的子进程（{:?}）将被转移到 init",
                self.tid,
                my_children.iter().map(|task| task.tid).collect::<Vec<_>>()
            );
            INIT_TASK
                .children
                .lock()
                .cursor_mut()
                .splice_before(my_children);
        }
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
    pub fn get_trap_context(&self) -> *const arch::NativeTrapContext {
        let misaligned_top = self.kstack.get_high_addr() - size_of::<*const Task>();
        let top = misaligned_top / STACK_ALIGN * STACK_ALIGN;
        (top - size_of::<arch::NativeTrapContext>()) as *const arch::NativeTrapContext
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        unsafe {
            alloc::alloc::dealloc(self.kstack.get_low_addr() as *mut u8, KERNEL_STACK_LAYOUT)
        };

        PID_ALLOCATOR.lock().deallocate(self.tid);

        self.transfer_children_to_init();

        // 注意，Task 被 drop 意味着它已经从父 Task 的 children 中被移除了，因此此处无需重复清理

        log::trace!("Task {} dropped", self.tid);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LoadElfError {
    ArchMismatch,
    BadAlignment,
    NoLoadSegment,
    NotYetImplemented,
    BadElfHead,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AuxiliaryEntry(pub usize, pub usize);

unsafe impl bytemuck::NoUninit for AuxiliaryEntry {}

pub type AuxiliaryVec = Vec<AuxiliaryEntry>;

mod aux_ids {
    pub const AT_NULL: usize = 0;
    pub const AT_PHENT: usize = 4;
    pub const AT_PAGESZ: usize = 6;
    pub const AT_ENTRY: usize = 9;
}
