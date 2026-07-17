use core::{ptr::null, sync::atomic::Ordering};

use crate::{
    basic::{
        id_allocator::IdAllocator,
        shared_with_asm::SharedWithAsm,
        sync::{Mutex, RwLock},
    },
    multitask::task::{Task, TaskArc},
};

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

#[must_use]
pub(super) fn unregister_task(checker: impl FnOnce() -> bool, pid: usize) -> bool {
    let mut map_guard = PID_TASK_MAP.write();
    if checker() {
        map_guard[pid].0 = null();
        true
    } else {
        false
    }
}

pub fn get_task_by_tid(pid: usize) -> Option<TaskArc> {
    let map_guard = PID_TASK_MAP.read();

    let ptr = map_guard.get(pid)?.0;
    if ptr.is_null() {
        return None;
    }

    let task = unsafe { TaskArc::from_asm(ptr) };

    task.refcount.fetch_add(1, Ordering::Relaxed);
    Some(task)
}

pub fn foreach_task(f: impl FnMut(&Task)) {
    // TODO: 性能优化（侵入式链表，请）
    PID_TASK_MAP
        .read()
        .iter()
        .filter_map(|ptr| unsafe { ptr.0.as_ref() })
        .for_each(f);
}
