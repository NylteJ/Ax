use core::{marker::PhantomPinned, ops::Deref, pin::Pin};

use alloc::sync::{Arc, UniqueArc};
use elf::{ElfBytes, endian::NativeEndian};

use crate::{
    basic::{shared_with_asm::SharedWithAsm, stack::TaskKernelStackPtr, sync::RwLock},
    filesystem::vfs::{File, FileArc, Path},
    memory::{
        PAGE_BYTES,
        address::{VIRTUAL_ADDRESS_LOW_HIGHEST, VirtAddr},
        address_space::AddressSpace,
        uaccess::fill_user_memory,
        virtual_memory_area::{VMAFlags, VMAGrowth, VMAType, VirtualMemoryArea},
    },
    multitask::{
        SchedulePolicy, TaskPriority,
        fdtable::FileDescriptorTable,
        fs_data::FilesystemData,
        task_context::TaskContext,
        task_manager::{PID_ALLOCATOR, register_task},
    },
    trap::trap_handler::trap_handler_raw_from_user,
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

    // pub exe_file: Pin<Arc<File>>,
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
        let kstack_sp = task_ptr_ptr as usize;

        let mut address_space = Self::new_addr_space();

        // 激活用户态页表（必须提前激活，因为后面可能要往用户空间里写一堆东西）
        address_space.get_mut().get_page_table().activitate();

        // 先创建一个半成品 task
        let task = Arc::pin(Self {
            kstack,
            pid: INIT_PID,

            policy,
            priority,

            address_space: UniqueArc::into_arc(address_space),
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

    pub unsafe fn set_current_task(task_ptr: *const Task) {
        use core::arch::asm;

        cfg_select! {
            target_arch = "riscv64" =>     unsafe { asm!("mv    tp, {0}", in(reg) task_ptr) },
            target_arch = "loongarch64" => unsafe { asm!("move $tp, {0}", in(reg) task_ptr) },
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
    fn new_addr_space() -> UniqueArc<RwLock<AddressSpace>> {
        let mut ret = UniqueArc::new(RwLock::new(AddressSpace::new()));

        let addr_space = ret.get_mut();

        const USER_STACK_BYTES: usize = 2048 * PAGE_BYTES;

        static_assertions::const_assert!(USER_STACK_BYTES % PAGE_BYTES == 0);
        static_assertions::const_assert!(USER_STACK_BYTES > PAGE_BYTES);

        addr_space.add_vma(VirtualMemoryArea {
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
