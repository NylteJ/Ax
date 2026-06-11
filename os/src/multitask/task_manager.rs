use core::{intrinsics::unlikely, mem::forget, pin::Pin, ptr::null};

use crate::{
    basic::sync::{Mutex, RwLock},
    multitask::{
        id_allocator::IdAllocator,
        task::{Task, TaskArc},
    },
};

use alloc::sync::Arc;

pub const PID_COUNT: usize = 8192;
pub static PID_ALLOCATOR: Mutex<IdAllocator<PID_COUNT>> = Mutex::new(IdAllocator::new());

#[derive(Clone, Copy)]
struct TaskPtr(*const Task);

unsafe impl Send for TaskPtr {}
unsafe impl Sync for TaskPtr {}

static PID_TASK_MAP: RwLock<[TaskPtr; PID_COUNT]> = RwLock::new([TaskPtr(null()); PID_COUNT]);

pub(super) fn register_task(task: &Task) {
    PID_TASK_MAP.write()[task.tid].0 = task;
}

pub(super) fn unregister_task(pid: usize) {
    PID_TASK_MAP.write()[pid].0 = null();
}

pub fn get_task_by_tid(pid: usize) -> Option<TaskArc> {
    let map_guard = PID_TASK_MAP.read();

    let ptr = map_guard.get(pid)?.0;
    if ptr.is_null() {
        return None;
    }

    // 注意这里可能存在的竞态：
    // 诚然，由于我们在 Task 的析构函数中 unregister 了对应 Task，这里能找到就说明对应 Task 尚未完成析构
    // 然而，此时对应 Task 可能已经开始析构，只是因为暂时无法获取 PID_TASK_MAP 的写锁而阻塞
    // 因此此处必须检查引用计数
    let arc = unsafe { Arc::from_raw(ptr) }; // 暂时安全，尽管其安全不变式可能违反
    if unlikely(Arc::strong_count(&arc) == 0) {
        forget(arc);
        None
    } else {
        unsafe { Arc::increment_strong_count(ptr) };
        Some(unsafe { Pin::new_unchecked(arc) })
    }
}
