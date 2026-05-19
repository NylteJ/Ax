use alloc::{boxed::Box, vec::Vec};

use alloc::vec;
use lazy_static::lazy_static;

use crate::basic::sync::Mutex;
use crate::multitask::{
    schedulers::Scheduler,
    task::{Task, TaskArc},
};

pub struct TaskScheduler {
    schedulers: Vec<Box<dyn Scheduler>>,
}

impl TaskScheduler {
    pub fn new() -> Self {
        use super::schedulers::*;

        Self {
            schedulers: vec![
                Box::new(realtime::RealtimeScheduler::new()),
                // Box::new(idle::IdleScheduler::new()),
            ],
        }
    }

    pub fn add_task(&mut self, task: TaskArc) {
        for scheduler in &mut self.schedulers {
            if scheduler.add_task(task.clone()).is_ok() {
                return;
            }
        }

        panic!("无法为 Task {:?} 找到合适的调度器", *task);
    }

    pub fn pick_next(&mut self) -> Option<TaskArc> {
        for scheduler in &mut self.schedulers {
            if let Some(task) = scheduler.pick_next() {
                return Some(task);
            }
        }

        None
    }

    /// 发生时钟中断时调用。返回 Some 时调用方应当使用 switch_context 切换到返回的 Task。否则无需任何操作。
    pub fn on_time_interrupt(&mut self, task: &Task) -> Option<TaskArc> {
        // Performance Note: 或许考虑像 Linux 内核那样直接缓存一个指针（Rust 那么写起来太麻烦了 XP）？
        for scheduler in &mut self.schedulers {
            if let Ok(task) = scheduler.on_time_interrupt(task) {
                return task;
            }
        }

        panic!("无法为 Task {:?} 找到合适的调度器来处理时间中断", task);
    }
}

lazy_static! {
    pub static ref TASK_SCHEDULER: Mutex<TaskScheduler> = Mutex::new(TaskScheduler::new());
}
