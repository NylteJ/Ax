use core::{
    cell::UnsafeCell,
    ffi::c_int,
    marker::PhantomPinned,
    mem::{offset_of, replace},
    num::NonZero,
    ops::Deref,
    ptr::{NonNull, addr_eq, null_mut},
    sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, Ordering},
};

use alloc::{alloc::AllocError, collections::TryReserveError, sync::Arc, vec::Vec};
use elf::{ElfBytes, endian::NativeEndian};
use intrusive_collections::{LinkedListLink, RBTreeLink, UnsafeRef};

use crate::{
    arch::{
        self,
        interfaces::{Basic, Regs, Trap, TrapContext},
    },
    basic::{
        DropGuard, UnsafeInited,
        bstr::ByteString,
        shared_with_asm::SharedWithAsm,
        stack::TaskKernelStackPtr,
        sync::{SpinMutex, SpinRwLock},
        timer::{check_all_timers, get_cpu_tick, set_itimer_real},
    },
    filesystem::vfs::{self, File, FileArc, Path},
    memory::{
        PAGE_BYTES, STACK_ALIGN,
        address::{DYNAMIC_INTERPRETER_BASE, USER_REGULAR_MEMORY_END, VirtAddr},
        address_space::AddressSpace,
        uaccess::{CopyError, copy_one_to_user, fill_user_memory},
        virtual_memory_area::{VMAFlags, VMAGrowth, VMAType, VirtualMemoryArea},
    },
    multitask::{
        SchedulePolicy, TaskPriority, cur_task, current_task,
        fdtable::FileDescriptorTable,
        fs_data::FilesystemData,
        futex::futex_lock,
        process_data::{ProcessData, ProcessRLimits},
        signal::{RtSignalData, SignalHandler, SignalId, SignalsWaitingList},
        task_arc::TaskAdapter,
        task_context::{TaskContext, switch_context},
        task_manager::{PID_ALLOCATOR, register_task},
        task_scheduler::TASK_SCHEDULER,
    },
    syscall::{misc::RobustListHead, process::CloneFlags, sync::get_futex_key},
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

    pub address_space: SpinRwLock<Arc<SpinRwLock<AddressSpace>>>,

    pub files: Arc<SpinRwLock<FileDescriptorTable>>,
    pub fs_data: Arc<SpinRwLock<FilesystemData>>,

    pub process: Arc<ProcessData>,
    /// 进程的 Task 链表的侵入式节点，由于所有操作都须借助 ProcessData 中的锁，它本身无需额外同步
    pub(super) task_of_process_link: LinkedListLink,

    pub signal_handler: Arc<SpinRwLock<SignalHandler>>,
    pub signals_wl: SpinMutex<SignalsWaitingList>,

    pub robust_list_head: AtomicPtr<RobustListHead>,

    /// 子 Task 链表头
    pub children: SpinMutex<intrusive_collections::LinkedList<TaskAdapter>>,
    /// 子 Task 链表的侵入式节点，由于所有操作都须借助 children 的锁，它本身无需额外同步
    pub(super) children_link: LinkedListLink,
    /// 父 Task 指针，通常不会变化
    ///
    /// 通过托孤机制保证总是有效（如果父节点提前死亡，则其改为指向 init）
    pub parent: SpinMutex<NonNull<Task>>,

    pub wait_state: SpinMutex<ZombieWaitingState>,
    /// 退出时会向父进程发送的信号，无法被改变。[`SIGINVAL`][`SignalId::SIGINVAL`] 表示不发送。
    pub exit_signal: SignalId,
    /// 任务状态，具体值的含义见 [`TaskState`]
    state: AtomicU8,

    pub time_stats: SpinRwLock<TaskTimeStats>,

    /// CLONE_CHILD_CLEARTID 和 set_tid_address 设定的地址，在 Task 退出时会由内核清除其中的值并唤醒对应 futex。
    ///
    /// 空指针表示无需操作。
    pub tid_address: AtomicPtr<c_int>,

    /// 受 `TIMERS` 锁保护
    pub timer_rbtree_key: UnsafeCell<usize>,
    /// 受 `TIMERS` 锁保护
    pub timer_rbtree_link: RBTreeLink,

    /// (下次到期时间, 间隔时间)，单位为 CPU tick，受 `ITIMERS_REAL` 锁保护
    pub itimer_real: UnsafeCell<Option<(usize, Option<NonZero<usize>>)>>,
    /// 受 `ITIMERS_REAL` 锁保护
    pub itimer_real_rbtree_link: RBTreeLink,

    pub user_ids: SpinRwLock<TaskUserIds>,

    pub(super) refcount: AtomicU32,

    _pin: PhantomPinned, // Task 的地址需要在 tp、内核栈等区域存储，因此不能移动
}

#[derive(Debug)]
pub struct ZombieWaitingState {
    /// Task 是否已经退出，如果有，则内部存放退出状态 (信号, 返回值)
    pub status: Option<(SignalId, u8)>,
}

#[derive(Debug)]
#[repr(u8)]
pub enum TaskState {
    /// 正常状态，正在运行或可被调度
    Normal = 0,
    /// 睡眠状态，不可被调度，必须被手动唤醒
    Sleeping = 1,
    /// 睡眠状态，和 [`Sleeping`][`Self::Sleeping`] 不同的是不能被任何信号打断
    ///
    /// 慎重使用，因为连 SIGKILL 这类特殊信号都无法打断它
    SleepingUninterruptible = 2,
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

#[derive(Debug, Clone, Copy)]
pub struct TaskUserIds {
    pub uid: u32,
    pub gid: u32,
    pub fsuid: u32,
    pub fsgid: u32,
    pub euid: u32,
    pub egid: u32,
    pub suid: u32,
    pub sgid: u32,
}

unsafe impl Send for Task {}
unsafe impl Sync for Task {}

pub use super::task_arc::TaskArc;

pub const INIT_PID: usize = 0;
pub static INIT_TASK: UnsafeInited<TaskArc> = unsafe { UnsafeInited::uninit() };

const KERNEL_STACK_LAYOUT: alloc::alloc::Layout = unsafe {
    alloc::alloc::Layout::from_size_align_unchecked(
        TaskKernelStackPtr::size(),
        TaskKernelStackPtr::align(),
    )
};

const USER_STACK_HIGHEST: usize = USER_REGULAR_MEMORY_END;

pub const USER_STACK_BYTES: usize = 2048 * PAGE_BYTES;

static_assertions::const_assert!(USER_STACK_BYTES % PAGE_BYTES == 0);
static_assertions::const_assert!(USER_STACK_BYTES > PAGE_BYTES);

/// TLS (Thread-Local Storage) information extracted from ELF PT_TLS segment
struct TlsInfo {
    /// File offset of TLS initialization image
    file_offset: u64,
    /// Size of TLS initialization image in file
    file_size: u64,
    /// Total size of TLS block in memory (includes zero-filled BSS portion)
    mem_size: u64,
}

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

        let (kstack, task_ptr_ptr) = Self::alloc_kstack().expect("必须能够为 init 进程分配内核栈");
        let kstack_sp = (task_ptr_ptr as usize) / STACK_ALIGN * STACK_ALIGN;

        let address_space = Self::new_addr_space();

        // 激活用户态页表（必须提前激活，因为后面可能要往用户空间里写一堆东西）
        address_space.get_page_table().activitate();

        // 先创建一个半成品 task
        let task = TaskArc::try_new(Self {
            kstack,
            tid: INIT_PID,

            policy,
            priority,

            address_space: SpinRwLock::new(Arc::new(SpinRwLock::new(address_space))),
            files: Arc::new(SpinRwLock::new(FileDescriptorTable::new())),

            // FIXME: 后面应该要改
            fs_data: Arc::new(SpinRwLock::new(FilesystemData {
                current_work_dir: work_dir,
            })),

            process: Arc::new(ProcessData::new(
                INIT_PID,
                INIT_PID,
                INIT_PID,
                ProcessRLimits::default(),
            )),
            task_of_process_link: LinkedListLink::new(),

            robust_list_head: AtomicPtr::new(null_mut()),

            signal_handler: Arc::new(SpinRwLock::new(SignalHandler::new())),
            signals_wl: SpinMutex::new(SignalsWaitingList::new()),

            children: SpinMutex::new(intrusive_collections::LinkedList::new(TaskAdapter::new())),
            children_link: LinkedListLink::new(),
            parent: SpinMutex::new(NonNull::dangling()), // init 进程没有父进程，先放一个悬空指针，后面改为指向自己

            wait_state: SpinMutex::new(ZombieWaitingState { status: None }),
            exit_signal: SignalId::SIGINVAL,
            state: AtomicU8::new(TaskState::Normal as u8),

            time_stats: SpinRwLock::new(TaskTimeStats::new()),

            tid_address: AtomicPtr::new(null_mut()),

            timer_rbtree_key: UnsafeCell::new(0),
            timer_rbtree_link: RBTreeLink::new(),

            itimer_real: UnsafeCell::new(None),
            itimer_real_rbtree_link: RBTreeLink::new(),

            // root
            user_ids: SpinRwLock::new(TaskUserIds {
                uid: 0,
                gid: 0,
                fsuid: 0,
                fsgid: 0,
                euid: 0,
                egid: 0,
                suid: 0,
                sgid: 0,
            }),

            refcount: AtomicU32::new(1),

            _pin: PhantomPinned,
        })
        .expect("必须要能够为 init 进程分配内存");

        // 填充 parent
        unsafe { task.parent.as_ptr().write(NonNull::from_ref(&task)) };

        // 将自身放入 process 的 task 列表中
        task.process
            .tasks
            .write()
            .push_back(unsafe { UnsafeRef::from_raw(&*task) });

        // 将 task 地址写入内核栈（注意我们现在不在这个内核栈上）
        unsafe { task_ptr_ptr.write(task.deref()) };
        // 然后，将其写入 tp
        unsafe { Self::set_current_task(task.clone().into_asm()) };

        // 现在我们已经设定好了任务上下文

        // 注意这里不能改为传入 &mut AddressSpace，因为内部它可能触发缺页异常，缺页异常处理时要获取 AddressSpace 的写权限，
        // 因此传入大粒度的可变引用会导致死锁或违反别名规则。
        let entry = Self::load_elf(&file, &task.address_space.read()).unwrap().0;

        register_task(&task);
        unsafe { UnsafeInited::init_unsafe(&INIT_TASK, task) };

        // 至此，task 终于完全初始化完毕了！

        unsafe { arch::NativeBasic::execute_init(entry, USER_STACK_HIGHEST, kstack_sp) };
    }

    /// 复制一份当前任务，并将其添加到调度器中，返回新任务的 TID。子任务的返回值（a0）会被篡改为 0。
    ///
    /// `ptid`、`tls` 和 `ctid` 参数只在指定了对应 `flags` 时生效。
    ///
    /// `exit_signal` 指定为 [`SIGINVAL`][`SignalId::SIGINVAL`] 时不发送任何信号，其余时候此信号会在退出时被发送给父进程。
    ///
    /// 注意此函数在内核端就是普通函数，返回用户态后新任务才可能被执行。此外，此函数在返回 [`Err`] 时提供强异常保证。
    pub fn fork(
        &self,
        flags: CloneFlags,
        ustack: Option<NonZero<usize>>,
        ptid: *mut c_int,
        tls: usize,
        ctid: *mut c_int,
        exit_signal: SignalId,
    ) -> Result<usize, ForkError> {
        // 对于内核栈，必须分配新的
        let (_kstack, task_ptr_ptr) =
            Self::alloc_kstack().map_err(|_| ForkError::NotEnoughMemory)?;
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
        // 以及必要时修改 tp
        if flags.contains(CloneFlags::SETTLS) {
            ret_trap_ctxt.regs.tp = tls;
        }
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

            // 对于 LoongArch，还需要保存浮点寄存器和 EUEN
            #[cfg(target_arch = "loongarch64")]
            unsafe {
                let euen: usize;
                core::arch::asm!("csrrd {0}, 0x2", out(reg) euen);

                if euen & 1 != 0 {
                    crate::multitask::task_context::store_fregs_to(ret_task_ctxt_ptr);
                }

                ret_task_ctxt_ptr
                    .byte_add(offset_of!(TaskContext, euen))
                    .cast::<usize>()
                    .write(euen);
            };

            // 剩余部分保持未初始化即可，它们会被 TrapContext 覆盖因此完全不重要
        }
        // 于是相对应的，kstack 也要改变，以伪造出该 Task 已经挂起的场景
        let kstack = unsafe { TaskKernelStackPtr::from_addr(kstack_sp) };

        // TID 也要分配新的
        let tid = PID_ALLOCATOR
            .lock()
            .allocate()
            .ok_or(ForkError::NotEnoughTid)?;

        if flags.contains(CloneFlags::PARENT_SETTID) {
            // 此时我们需要将 TID 同时写入父子地址空间
            // 不难发现在 fork 地址空间前写入能省去许多麻烦
            copy_one_to_user(ptid, &(tid as c_int)).map_err(|err| ForkError::CopyError(err))?;
        }

        // 这两个直接复制即可
        let policy = self.policy.clone();
        let priority = self.priority;

        let my_address_space = self.address_space.read();
        let address_space = if flags.contains(CloneFlags::VM) {
            // 浅拷贝一份 Arc 即可
            my_address_space.clone()
        } else {
            // 地址空间需要复制一份，并将所有页设为写时复制
            let address_space = Arc::new(SpinRwLock::new(my_address_space.write().clone_cow()));
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
            Arc::new(SpinRwLock::new(self.files.read().clone()))
        };

        let fs_data = if flags.contains(CloneFlags::FS) {
            // 共享文件系统信息（文件系统的根目录、当前工作目录和 umask 值）
            self.fs_data.clone()
        } else {
            // 使用初始时相等的两个文件系统信息
            Arc::new(SpinRwLock::new(self.fs_data.read().clone()))
        };

        let process = if flags.contains(CloneFlags::THREAD) {
            // 线程，放在同一个进程（线程组）中，浅拷贝即可
            self.process.clone()
        } else {
            // 进程，直接新建一个进程（线程组），注意仍在同一个进程组中，以及新进程默认继承旧进程的限制
            let rlimits = self.process.rlimits.read().clone();
            Arc::new(ProcessData::new(
                tid,
                self.process.pgid.load(Ordering::Relaxed),
                self.process.sid.load(Ordering::Relaxed),
                rlimits,
            ))
        };

        let signal_handler = if flags.contains(CloneFlags::SIGHAND) {
            // 共享同一个信号处理器
            self.signal_handler.clone()
        } else {
            // 使用初始时相等的两个信号处理器
            Arc::new(SpinRwLock::new(self.signal_handler.read().clone()))
        };

        let parent = if flags.intersects(CloneFlags::PARENT | CloneFlags::THREAD) {
            // 克隆父指针，父 Task 和自身相同（注意 THREAD 隐含这点）
            self.parent_task()
        } else {
            // 不克隆父指针，父 Task 设为自身
            self
        };

        let child = TaskArc::try_new(Self {
            kstack,
            tid,

            policy,
            priority,

            address_space: SpinRwLock::new(address_space),
            files,
            fs_data,

            process,
            task_of_process_link: LinkedListLink::new(),

            robust_list_head: AtomicPtr::new(null_mut()),

            signal_handler,
            signals_wl: SpinMutex::new(self.signals_wl.lock().clone_for_fork()),

            // 这几个不应当复制
            children: SpinMutex::new(intrusive_collections::LinkedList::new(TaskAdapter::new())),
            children_link: LinkedListLink::new(),
            parent: SpinMutex::new(NonNull::from_ref(parent)),

            wait_state: SpinMutex::new(ZombieWaitingState { status: None }),
            exit_signal,
            state: AtomicU8::new(TaskState::Normal as u8),

            time_stats: SpinRwLock::new(TaskTimeStats::new()),

            tid_address: AtomicPtr::new(ctid),

            timer_rbtree_key: UnsafeCell::new(0),
            timer_rbtree_link: RBTreeLink::new(),

            itimer_real: UnsafeCell::new(None), // 不继承
            itimer_real_rbtree_link: RBTreeLink::new(),

            user_ids: SpinRwLock::new(*self.user_ids.read()),

            refcount: AtomicU32::new(1),

            _pin: PhantomPinned,
        })?;

        if !flags.contains(CloneFlags::THREAD) {
            // children 以进程为单位
            parent.children.lock().push_back(child.clone());
        }

        child
            .process
            .tasks
            .write()
            .push_back(unsafe { UnsafeRef::from_raw(&*child) });

        unsafe { task_ptr_ptr.write(child.deref()) };

        register_task(&child);

        TASK_SCHEDULER.lock().add_task(child);

        log::trace!("forked: {} => {tid}", self.tid);

        Ok(tid)
    }

    /// 将当前任务替换为 `file` 对应可执行文件的内容。不传递任何参数。返回半成品辅助向量。
    ///
    /// 注意此函数在内核端就是普通函数，返回用户态后替换后的任务才可能被执行。
    pub fn replace_execute(&self, file: FileArc) -> Result<AuxiliaryVec, LoadElfError> {
        // 创建新的地址空间，并替换当前地址空间

        let new_addr_space = Arc::new(SpinRwLock::new(Self::new_addr_space()));

        let mut my_addr_space = self.address_space.write();

        // 注意这里只替换当前 Task 的地址空间，其它共享地址空间的进程不受影响
        // TODO：按 Linux 语义，其它线程应当终止
        let old_addr_space = replace(&mut *my_addr_space, new_addr_space);

        let my_addr_space = my_addr_space.downgrade();

        my_addr_space.read().get_page_table().activitate();

        let guard = DropGuard::new(|| {
            old_addr_space.read().get_page_table().activitate();
            *self.address_space.write() = old_addr_space;
        });

        // 加载 ELF 文件并获取入口地址
        let load_elf_result = Self::load_elf(&file, &my_addr_space);
        drop(my_addr_space); // 避免和 DropGuard 中的写锁产生死锁
        let (entry, auxv, tls_info) = load_elf_result?;

        // 将入口地址和栈指针写入到 TrapContext 中
        // 特别注意，和前面几个函数不同，我们并没有更换内核栈，因此不需要伪造任何东西，只要修改 ret_pc 和 sp 即可
        // （剩余寄存器的值并不重要）
        let trap_context = unsafe { self.get_trap_context().cast_mut().as_mut_unchecked() };
        trap_context.ret_pc = entry;
        trap_context.regs.sp = USER_STACK_HIGHEST;

        // Set up TLS for the main thread if the ELF has a TLS segment.
        // glibc expects the kernel to allocate TLS and set tp.
        if let Some(ref tls) = tls_info {
            let tls_memsz = tls.mem_size as usize;
            if tls_memsz > 0 {
                // TCB (Thread Control Block) size before TLS data.
                // Both LoongArch and RISC-V glibc use TLS variant 1 (TLS_TCB_AT_TP=1):
                // tp -> TCB (tcbhead_t) -> TLS data.
                // tcbhead_t is 2 pointers (dtv + self) = 16 bytes on 64-bit.
                const TCB_SIZE: usize = 16;
                let total_size = TCB_SIZE + tls_memsz;
                let layout = unsafe {
                    alloc::alloc::Layout::from_size_align_unchecked(total_size, 16)
                };
                let tls_base = unsafe { alloc::alloc::alloc(layout) };
                if !tls_base.is_null() {
                    // Zero-initialize the entire TLS block
                    unsafe { core::ptr::write_bytes(tls_base, 0, total_size) };

                    // Copy TLS initialization image to offset TCB_SIZE
                    if tls.file_size > 0 {
                        let mut file_guard = file.data.lock();
                        if file_guard
                            .fseek(vfs::SeekFrom::Start(tls.file_offset))
                            .is_err()
                        {
                            log::error!("TLS: fseek 失败，无法加载 TLS 初始化映像");
                            unsafe { alloc::alloc::dealloc(tls_base, layout) };
                            return Err(LoadElfError::BadElfHead);
                        }
                        let dst = unsafe { tls_base.add(TCB_SIZE) };
                        // Clamp copy size to the smaller of file_size and mem_size
                        // (should always be file_size since we validated p_filesz <= p_memsz)
                        let copy_size = (tls.file_size as usize).min(tls_memsz);
                        // kread 按页（4 KiB）逐页调用 visitor，必须用游标推进写入位置，
                        // 否则跨多页的 TLS 初始化映像会互相覆盖。
                        let mut copied = 0usize;
                        if file_guard
                            .kread(
                                |data| {
                                    let len = data.len().min(copy_size - copied);
                                    unsafe {
                                        core::ptr::copy_nonoverlapping(
                                            data.as_ptr(),
                                            dst.add(copied),
                                            len,
                                        )
                                    };
                                    copied += len;
                                    Ok(len as u64)
                                },
                                copy_size as u64,
                            )
                            .is_err()
                        {
                            log::error!("TLS: kread 失败，无法加载 TLS 初始化映像");
                            unsafe { alloc::alloc::dealloc(tls_base, layout) };
                            return Err(LoadElfError::BadElfHead);
                        }
                    }

                    // tp points to TCB (= base of allocation)
                    trap_context.regs.tp = tls_base as usize;
                    log::info!(
                        "TLS: base={tls_base:p}, tp={:#x}, tcb={}, filesz={}, memsz={}",
                        trap_context.regs.tp, TCB_SIZE, tls.file_size, tls.mem_size
                    );
                }
            }
        }

        // 关闭标记为 CLOEXEC 的文件描述符
        self.files.write().close_on_exec();

        // 清空待决信号
        self.signals_wl.lock().clear_for_exec();

        // 重置自定义信号处理函数
        // FIXME：万一两个进程共用一个 SignalHandler 怎么办？
        self.signal_handler.write().reset_custom_handlers();

        // OK，这样就可以了！
        let _ = DropGuard::release(guard);
        Ok(auxv)
    }

    pub unsafe fn set_current_task(task_ptr: *const Task) {
        arch::NativeRegs::set_tp(task_ptr.addr());
    }

    /// 暂时切换到此任务。此任务必须不在运行中，因此不能切换到当前任务。
    ///
    /// 通常情况下（当前任务还在调度器中），此调用迟早会正常返回，但当前任务的确可能要在很久以后才能被调度到，因此请尽可能释放不必要的资源
    pub fn switch_to(target: TaskArc) {
        debug_assert_eq!(
            target.state.load(Ordering::Relaxed),
            TaskState::Normal as u8
        );
        debug_assert!(!addr_eq(&*target, current_task()));

        let context = target.get_context();
        unsafe { target.into_asm() };
        unsafe { switch_context(context.as_ref_unchecked()) };

        // 现在我们被重新调度了

        // 重新设置 last_update_stamp
        cur_task().time_stats.write().last_update_stamp = get_cpu_tick();
    }

    /// 和 [`Self::switch_to`] 类似，但允许 `target` 为当前任务
    pub fn switch_to_maybe_current(target: TaskArc) {
        if addr_eq(target.deref(), current_task()) {
            unsafe { target.into_asm() };
        } else {
            Self::switch_to(target);
        }
    }

    /// 获取父 Task 的引用
    pub fn parent_task(&self) -> &Task {
        unsafe { self.parent.lock().as_ref() }
    }

    /// 当此 Task 退出时调用，将子进程转移到 init 进程下并唤醒等待的父进程，并向父进程发送恰当的终止信号。
    ///
    /// 它还会执行 CLEARTID 操作并释放一部分资源。
    pub fn on_exit(&self, signal: SignalId, return_value: u8) {
        let old = self
            .wait_state
            .lock()
            .status
            .replace((signal, return_value));
        debug_assert!(old.is_none());

        self.transfer_children_to_init();
        self.parent_task().process.waiting_for_child.wake_all();

        if self.exit_signal != SignalId::SIGINVAL {
            log::trace!(
                "TID 为 {} 的线程退出，向父进程（PID = {}）发送信号 {:?}",
                self.tid,
                self.parent_task().process.pid,
                self.exit_signal
            );
            self.parent_task().process.send_signal(self.exit_signal);
        }

        // 很坑的一步是这个不能放在析构函数里，不然极其容易死锁（与 Linux 也不一致）
        if let tid_addr = self.tid_address.load(Ordering::Relaxed).cast()
            && let Ok(futex_key) = get_futex_key(tid_addr, false)
        {
            let mut futex_word = futex_lock(&futex_key);

            let store_result = futex_word.relaxed_atom_store(tid_addr, 0);
            if let Err(err) = store_result {
                log::warn!("向 tid_address {tid_addr:p} 写入 0 失败，发生错误 {err:?}");
            } else {
                log::trace!("成功向 tid_address {tid_addr:p} 写入 0");
            }

            let woken = futex_word.wake(1, u32::MAX);
            log::trace!("唤醒了 tid_address {tid_addr:p} 上的 {woken} 个等待者");
        }

        *self.files.write() = FileDescriptorTable::new();
    }

    /// 向此 Task 发送信号，若 Task 正在睡眠则还可能会唤醒 Task。
    ///
    /// 注意此函数只是向对应 Task 发送信号，信号的处理可能被延迟，且无论如何只会在该 Task 准备返回用户态时处理
    pub fn send_signal(&self, signal: SignalId) {
        if self.tid == INIT_PID {
            // init 进程不处理任何信号
            return;
        }

        let not_blocked = {
            let mut wl = self.signals_wl.lock();
            wl.add(signal);
            !wl.mask.contains(signal.as_mask())
        };

        if not_blocked {
            self.try_interrupt_sleep();
        }
    }

    pub fn find_signal_to_consume(&self) -> Option<SignalId> {
        let wl = self.signals_wl.lock();
        let mask = wl.mask;
        let private = wl.find_can_consume();
        drop(wl);

        private.or_else(|| {
            self.process
                .signals_wl
                .lock()
                .find_can_consume_with_mask(mask)
        })
    }
    pub fn try_consume_signal(&self) -> Option<(SignalId, Option<RtSignalData>)> {
        let mut wl = self.signals_wl.lock();
        let mask = wl.mask;
        let private = wl.try_consume_one();
        drop(wl);

        private.or_else(|| {
            self.process
                .signals_wl
                .lock()
                .try_consume_one_with_mask(mask)
        })
    }

    fn alloc_kstack() -> Result<(TaskKernelStackPtr, *mut *const Task), ()> {
        let kstack_btm = unsafe { alloc::alloc::alloc(KERNEL_STACK_LAYOUT) };
        if kstack_btm.is_null() {
            return Err(());
        }

        let kstack_btm = kstack_btm as usize;
        let kstack_top = kstack_btm + TaskKernelStackPtr::size();

        // SAFETY: 这就是内核栈的下边界
        let kstack = unsafe { TaskKernelStackPtr::from_addr(kstack_btm) };

        let task_ptr_addr = kstack_top - size_of::<*const ()>();

        Ok((kstack, task_ptr_addr as *mut *const Task))
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

    /// 从 ELF 文件中加载程序，填入对应地址空间中，并返回入口地址和半成品辅助向量
    ///
    /// 它也负责设置初始堆区 VMA 和 brk。
    /// （和 Linux 不同，我们初始时就有一个很短的堆区 VMA，因为反正虚拟地址多得用不完，这样只会掩盖少量用户态越界访问）
    ///
    /// Returns (entry_point, auxv, optional_tls_info)
    fn load_elf(
        file: &FileArc,
        address_space: &SpinRwLock<AddressSpace>,
    ) -> Result<(usize, AuxiliaryVec, Option<TlsInfo>), LoadElfError> {
        struct InnerReturn {
            load_seg_start: VirtAddr,
            load_seg_end: VirtAddr,
            program_entry: usize,
            interp_entry: Option<usize>,
            ehdr: elf::file::FileHeader<NativeEndian>,
            tls_info: Option<TlsInfo>,
        }

        fn inner(
            file: &FileArc,
            address_space: &SpinRwLock<AddressSpace>,
            loading_interp: bool,
        ) -> Result<InnerReturn, LoadElfError> {
            let head = unsafe { File::get_page_cache(file.get_inode(), 0) }.unwrap();

            let elf = ElfBytes::<NativeEndian>::minimal_parse_no_section_headers(head).map_err(
                |err| {
                    if let elf::ParseError::SliceReadError((_from, to)) = err {
                        debug_assert!(to <= PAGE_BYTES, "超大 ELF 头的解析尚未实现");
                    }

                    LoadElfError::BadElfHead
                },
            )?;

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

            let mut load_seg_start = None;
            let mut load_seg_end = None;

            let mut interp_entry = None;
            let mut tls_info = None;

            let segs = elf.segments().unwrap();
            for seg in segs {
                use elf::abi::{PT_GNU_STACK, PT_INTERP, PT_LOAD, PT_NULL, PT_TLS};

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

                        let interp_offset = if loading_interp {
                            DYNAMIC_INTERPRETER_BASE.0 as u64
                        } else {
                            0
                        };

                        // 注意 inseg_offset 相等也推出了 inpage_offset 相等（读者自证不难）
                        // 此外，后面其实不用考虑 inseg_offset，因为我们的加载等操作还是按页为单位来的

                        let vaddr = seg.p_vaddr - inpage_offset + interp_offset;
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

                            if file_end != zeros_end {
                                address_space.write().add_vma(VirtualMemoryArea {
                                    start: file_end,
                                    end: zeros_end,
                                    flags,
                                    vma_type: VMAType::Anonymous,
                                    growth: VMAGrowth::Fixed,
                                });
                            }
                        }

                        load_seg_end.replace(zeros_end); // ELF 标准规定 PT_LOAD 段必须按地址升序排列
                        if load_seg_start.is_none() {
                            load_seg_start.replace(VirtAddr(vaddr as usize));
                        }
                    }

                    PT_INTERP => {
                        if interp_entry.is_some() {
                            log::error!("ELF 文件有多个 PT_INTERP 段，无法加载");
                            return Err(LoadElfError::BadInterpreter);
                        }

                        if loading_interp {
                            log::error!("正在加载解释器，但又遇到了 PT_INTERP 段，无法加载");
                            return Err(LoadElfError::BadInterpreter);
                        }

                        let off = seg.p_offset;
                        let len = seg.p_filesz;

                        if len == 0 {
                            log::error!("ELF 的 PT_INTERP 段的 p_filesz 不能为 0，无法加载");
                            return Err(LoadElfError::BadInterpreter);
                        }

                        let len = len - 1; // 解释器路径必须以 '\0' 结尾，因此实际长度要减一

                        let interp_path = {
                            let mut path = ByteString(Vec::try_with_capacity(len as usize)?);
                            let mut file_data_guard = file.data.lock();
                            file_data_guard.fseek(vfs::SeekFrom::Start(off)).unwrap();
                            file_data_guard
                                .kread(
                                    |data| {
                                        path.0.extend(data);
                                        Ok(data.len() as u64)
                                    },
                                    len,
                                )
                                .unwrap();
                            path
                        };

                        log::trace!("发现解释器路径：{interp_path}");

                        if interp_path.get(0) != Some(&b'/') {
                            log::error!("解释器路径必须是非空绝对路径，无法加载"); // 绝对路径的部分待定
                            return Err(LoadElfError::BadInterpreter);
                        }

                        let ptw = vfs::PathToWalk::Absolute(&interp_path);
                        let path = vfs::walk_path(ptw)
                            .and_then(|walked| walked.ensure())
                            .map_err(|_| LoadElfError::BadInterpreter)?;

                        log::trace!("找到解释器 {}！", path.0.data.lock().name);

                        let file = path
                            .0
                            .data
                            .lock()
                            .ops
                            .open(path.1, vfs::FileFlags::Read)
                            .map_err(|_| LoadElfError::BadInterpreter)?;

                        let entry = inner(&file, address_space, true)?.program_entry;
                        interp_entry.replace(entry + DYNAMIC_INTERPRETER_BASE.0);
                    }
                    PT_TLS => {
                        log::info!(
                            "发现 PT_TLS 段: offset={:#x}, filesz={:#x}, memsz={:#x}",
                            seg.p_offset, seg.p_filesz, seg.p_memsz
                        );
                        if tls_info.is_some() {
                            log::error!("ELF 文件有多个 PT_TLS 段，无法加载");
                            return Err(LoadElfError::BadElfHead);
                        }
                        // Validate PT_TLS invariants per ELF spec: p_memsz >= p_filesz
                        if seg.p_filesz > seg.p_memsz {
                            log::error!("PT_TLS 段异常: p_filesz > p_memsz");
                            return Err(LoadElfError::BadElfHead);
                        }
                        // Note: TLS segment can appear both in the executable and in the
                        // interpreter. We store the TLS info; in the outer function we
                        // will allocate and set up the TLS area for the main thread.
                        tls_info = Some(TlsInfo {
                            file_offset: seg.p_offset,
                            file_size: seg.p_filesz,
                            mem_size: seg.p_memsz,
                        });
                    }
                    PT_GNU_STACK => {
                        log::info!(
                            "发现 PT_GNU_STACK 段，然而，我们的栈区本就不可执行，因此无需进一步操作"
                        );
                    }
                    other => log::warn!("未知 p_type: {other:#x}，已跳过"),
                }
            }

            Ok(InnerReturn {
                load_seg_start: load_seg_start.ok_or(LoadElfError::NoLoadSegment)?,
                load_seg_end: load_seg_end.ok_or(LoadElfError::NoLoadSegment)?,
                program_entry: elf.ehdr.e_entry as usize,
                interp_entry,
                ehdr: elf.ehdr,
                tls_info,
            })
        }

        let InnerReturn {
            load_seg_start,
            load_seg_end,
            program_entry,
            interp_entry,
            ehdr,
            tls_info,
        } = inner(file, address_space, false)?;

        // 初始化堆区 VMA 和 brk
        {
            let program_data_end = load_seg_end;

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

        macro_rules! aux {
            ($($key: ident: $val: expr $(,)?)+; with $reserved: literal more space) => {{
                use AuxId::*;
                let mut ret = Vec::with_capacity(${count($key)} + $reserved);
                $(
                    unsafe { ret.push_within_capacity(AuxiliaryEntry($key, $val)).unwrap_unchecked() };
                )+
                ret
            }};
        }

        let is_dynamic = interp_entry.is_some();

        let auxv = aux![
            AT_ENTRY: program_entry,
            AT_PAGESZ: PAGE_BYTES,
            AT_PHDR: load_seg_start.0 + ehdr.e_phoff as usize,
            AT_PHNUM: ehdr.e_phnum as usize,
            AT_PHENT: ehdr.e_phentsize as usize,
            AT_BASE: if is_dynamic { DYNAMIC_INTERPRETER_BASE.0 } else { 0 };
            with 2 more space
        ];

        Ok((interp_entry.unwrap_or(program_entry), auxv, tls_info))
    }

    fn transfer_children_to_init(&self) {
        let my_children = self.children.lock().take();
        if !my_children.is_empty() {
            for child in &my_children {
                *child.parent.lock() = NonNull::from_ref(&INIT_TASK);
            }

            INIT_TASK
                .children
                .lock()
                .cursor_mut()
                .splice_before(my_children);

            let woken = INIT_TASK.process.waiting_for_child.wake_all();
            debug_assert!(woken <= 1);
            log::trace!("转移子进程给 init 后唤醒了 {} 个等待的进程", woken);
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

impl Task {
    pub fn sleep_current() {
        cur_task().do_sleep(TaskState::Sleeping);
    }
    pub fn sleep_current_uninterruptible() {
        cur_task().do_sleep(TaskState::SleepingUninterruptible);
    }

    fn do_sleep(&self, new_state: TaskState) {
        let old_state = self.state.swap(new_state as u8, Ordering::Relaxed);
        debug_assert_eq!(
            old_state,
            TaskState::Normal as u8,
            "试图在非正常状态 （{old_state}） 下睡眠"
        );

        loop {
            if let Some(next) = { TASK_SCHEDULER.lock().pick_next() } {
                // 注意有概率下一个被唤醒的还是我们（如果外层循环真正起效，则很大概率还是我们），
                Task::switch_to_maybe_current(next);

                // 注意我们没有将自身添加回调度器，也没有转移 tp 中的 TaskArc 的所有权，回到这里说明睡眠已经结束了

                // 注意这个 Acquire load 是必须的，因为我们需要与唤醒者的 Release store 同步
                let cur_state = self.state.load(Ordering::Acquire);

                debug_assert_eq!(cur_state, TaskState::Normal as u8);
                break;
            }

            // 此时只可能是设定了定时唤醒（因为这是唯一一个可以不运行任何指令就触发的唤醒条件），
            // 检查定时器即可

            arch::NativeBasic::halt();
            check_all_timers(); // 如果唤醒了任务，下次循环前面的 if 条件将成真
        }
    }

    /// 尝试唤醒对应进程，若对应进程并未睡眠则无效果并返回 [`None`]，反之返回 [`Some`]。
    pub fn try_wake(&self) -> Option<()> {
        let old_state = self.state.swap(TaskState::Normal as u8, Ordering::Release);

        if old_state == TaskState::Normal as u8 {
            return None;
        }

        self.wake();

        Some(())
    }

    /// 尝试唤醒对应进程，若对应进程并未睡眠或正处于不可打断的睡眠状态则返回 [`None`]，反之返回 [`Some`]。
    pub fn try_interrupt_sleep(&self) -> Option<()> {
        self.state
            .compare_exchange(
                TaskState::Sleeping as u8,
                TaskState::Normal as u8,
                Ordering::Release,
                Ordering::Relaxed,
            )
            .ok()?;

        self.wake();

        Some(())
    }

    fn wake(&self) {
        // 注意我们先前睡眠时没有转移走 tp 中的所有权，而是留到这里转移
        TASK_SCHEDULER
            .lock()
            .add_task(unsafe { TaskArc::from_asm(self) });
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        unsafe {
            alloc::alloc::dealloc(self.kstack.get_low_addr() as *mut u8, KERNEL_STACK_LAYOUT)
        };

        PID_ALLOCATOR.lock().deallocate(self.tid);

        self.transfer_children_to_init();

        {
            let mut tasks = self.process.tasks.write();
            debug_assert!(tasks.iter().any(|task| addr_eq(task, self)));
            unsafe { tasks.cursor_mut_from_ptr(self) }.remove();
        }

        unsafe { set_itimer_real(self, None) };

        if !self.robust_list_head.get_mut().is_null() {
            log::error!("TODO: 处理 robust_list_head");
        }

        // 注意，Task 被 drop 意味着它已经从父 Task 的 children 中被移除了，因此此处无需重复清理

        log::trace!("Task {} dropped", self.tid);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LoadElfError {
    ArchMismatch,
    BadAlignment,
    NoLoadSegment,
    BadElfHead,
    NotEnoughMemory,
    BadInterpreter,
}

impl From<TryReserveError> for LoadElfError {
    fn from(_value: TryReserveError) -> Self {
        Self::NotEnoughMemory
    }
}

#[derive(Debug)]
pub enum ForkError {
    NotEnoughTid,
    NotEnoughMemory,
    CopyError(CopyError),
}

impl From<AllocError> for ForkError {
    fn from(_value: AllocError) -> Self {
        Self::NotEnoughMemory
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::NoUninit)]
pub struct AuxiliaryEntry(pub AuxId, pub usize);

pub type AuxiliaryVec = Vec<AuxiliaryEntry>;

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::NoUninit)]
#[expect(non_camel_case_types)]
pub enum AuxId {
    AT_NULL = 0,
    AT_PHDR = 3,
    AT_PHENT = 4,
    AT_PHNUM = 5,
    AT_PAGESZ = 6,
    AT_BASE = 7,
    AT_ENTRY = 9,
    AT_RANDOM = 25,
}
