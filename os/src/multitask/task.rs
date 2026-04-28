use core::{marker::PhantomPinned, pin::Pin, ptr::write};

use alloc::sync::Arc;

use crate::{
    basic::{stack::TaskKernelStackPtr, sync::RwLock},
    memory::{
        PAGE_BYTES,
        address::{VIRTUAL_ADDRESS_LOW_HIGHEST, VirtAddr},
        address_space::AddressSpace,
        virtual_memory_area::{VMAFlags, VMAGrowth, VMAType, VirtualMemoryArea},
    },
    multitask::{
        SchedulePolicy, TaskPriority,
        task_context::TaskContext,
        task_manager::{PID_ALLOCATOR, register_task},
    },
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

    pub memory_space: Arc<RwLock<AddressSpace>>,

    // FIXME: 这是临时的
    pub ustack: usize,
    pub entry: RwLock<Option<usize>>,

    _pin: PhantomPinned, // Task 的地址需要在 tp、内核栈等区域存储，因此不能移动
}

pub type TaskArc = Pin<Arc<Task>>;

impl Task {
    /// 创建一个新的 Task 并为它分配内核栈等资源。返回的 Arc 保证具有其独占所有权。
    ///
    /// 不会将其添加到调度器中
    pub fn new(policy: SchedulePolicy, priority: TaskPriority, entry: usize) -> Pin<Arc<Self>> {
        let pid = PID_ALLOCATOR.lock().allocate().expect("必须能够分配 PID");

        let task_ptr_addr = {
            use alloc::alloc;

            let layout = alloc::Layout::from_size_align(
                TaskKernelStackPtr::size(),
                TaskKernelStackPtr::align(),
            )
            .unwrap();

            let kstack_ptr = unsafe { alloc::alloc(layout) };
            if kstack_ptr.is_null() {
                panic!("无法为 Task 分配内核栈");
            }

            let kstack_btm = kstack_ptr as usize;
            let kstack_top = kstack_btm + TaskKernelStackPtr::size();

            kstack_top - size_of::<*const ()>()
        };

        let memory_space = Arc::new(RwLock::new(AddressSpace::new()));

        const USER_STACK_HIGHEST: usize = VIRTUAL_ADDRESS_LOW_HIGHEST + 1;

        memory_space.write().add_vma(VirtualMemoryArea {
            start: VirtAddr(USER_STACK_HIGHEST - PAGE_BYTES),
            end: VirtAddr(USER_STACK_HIGHEST),
            flags: VMAFlags::Read | VMAFlags::Write | VMAFlags::User,
            vma_type: VMAType::Anonymous,
            growth: VMAGrowth::GrowDown(VirtAddr(USER_STACK_HIGHEST - 2048 * PAGE_BYTES)),
        });

        let ret = Arc::pin(Self {
            // SAFETY: 内核栈大小大于单个指针大小，因此此值一定仍在内核栈内，且不位于边界
            kstack: unsafe { TaskKernelStackPtr::from_addr(task_ptr_addr) },
            pid,

            policy,
            priority,

            memory_space,

            ustack: USER_STACK_HIGHEST,
            entry: RwLock::new(Some(entry)),

            _pin: PhantomPinned,
        });

        unsafe {
            write(
                task_ptr_addr as *mut usize,
                ret.as_ref().get_ref() as *const _ as usize,
            );
        }

        register_task(ret.clone());

        ret
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
        use alloc::alloc;

        let layout =
            alloc::Layout::from_size_align(TaskKernelStackPtr::size(), TaskKernelStackPtr::align())
                .unwrap();

        unsafe { alloc::dealloc(self.kstack.get_low_addr() as *mut u8, layout) };

        PID_ALLOCATOR.lock().deallocate(self.pid);

        crate::println!("Task {} dropped", self.pid);
    }
}
