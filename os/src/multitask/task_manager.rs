use crate::{
    basic::sync::Mutex,
    multitask::{
        id_allocator::IdAllocator,
        task::{Task, TaskArc},
    },
};

use hashbrown::HashMap;
use pin_weak::sync::PinWeak;

pub static PID_ALLOCATOR: Mutex<IdAllocator<32768>> = Mutex::new(IdAllocator::new());

lazy_static::lazy_static! {
    static ref PID_TASK_MAP: Mutex<HashMap<usize, PinWeak<Task>>> = Mutex::new(HashMap::new());
}

/// 将 Task 注册到全局 Task 表中
pub fn register_task(task: TaskArc) {
    let mut map = PID_TASK_MAP.lock();
    let entry = map.entry(task.tid);

    use hashbrown::hash_map::Entry;
    match entry {
        Entry::Occupied(mut occupied) => {
            if occupied.get().strong_count() == 0 {
                occupied.insert(PinWeak::downgrade(task));
            } else {
                let pid = task.tid;
                assert!(
                    occupied.get().ptr_eq(&PinWeak::downgrade(task)),
                    "试图插入拥有重复 PID 的不同任务（PID = {pid}）",
                );
            }
        }
        Entry::Vacant(vacant) => {
            vacant.insert(PinWeak::downgrade(task));
        }
    }
}

pub fn get_task_by_pid(pid: usize) -> Option<TaskArc> {
    PID_TASK_MAP.lock().get(&pid)?.upgrade()
}
