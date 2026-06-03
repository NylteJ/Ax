use core::sync::atomic::AtomicBool;

use intrusive_collections::{LinkedListLink, UnsafeRef, intrusive_adapter};

use crate::{basic::sync::RwLock, multitask::task::Task};

#[derive(Debug)]
pub struct ProcessData {
    /// 主线程的 TID，也是进程（线程组）的 PID
    pub pid: usize,
    /// 进程组 ID
    pub pgid: usize,

    pub tasks: RwLock<intrusive_collections::LinkedList<TaskOfProcessAdapter>>,

    /// 是否正在退出（exit_group）
    pub is_exiting: AtomicBool,
}

intrusive_adapter!(pub TaskOfProcessAdapter = UnsafeRef<Task>: Task { task_of_process_link => LinkedListLink });

impl ProcessData {
    pub fn new(pid: usize, pgid: usize) -> Self {
        Self {
            pid,
            pgid,
            tasks: RwLock::new(intrusive_collections::LinkedList::new(
                TaskOfProcessAdapter::new(),
            )),
            is_exiting: AtomicBool::new(false),
        }
    }
}

#[cfg(debug_assertions)]
impl Drop for ProcessData {
    fn drop(&mut self) {
        assert!(self.tasks.get_mut().is_empty());
        log::trace!("process {} dropped", self.pid);
    }
}
