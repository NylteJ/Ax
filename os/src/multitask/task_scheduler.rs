use core::pin::Pin;

use alloc::{boxed::Box, sync::Arc, vec::Vec};

use alloc::vec;
use lazy_static::lazy_static;

use crate::basic::sync::RwLock;
use crate::multitask::{schedulers::Scheduler, task::Task};

pub struct TaskScheduler {
    schedulers: Vec<Box<dyn Scheduler>>,
}

impl TaskScheduler {
    pub fn new() -> Self {
        use super::schedulers::*;

        Self {
            schedulers: vec![
                Box::new(realtime::RealtimeScheduler::new()),
                Box::new(idle::IdleScheduler::new()),
            ],
        }
    }

    pub fn add_task(&mut self, task: Pin<Arc<Task>>) {
        for scheduler in &mut self.schedulers {
            if scheduler.add_task(task.clone()).is_ok() {
                return;
            }
        }

        panic!("无法为 Task {:?} 找到合适的调度器", *task);
    }

    pub fn pick_next(&mut self) -> Option<Pin<Arc<Task>>> {
        for scheduler in &mut self.schedulers {
            if let Some(task) = scheduler.pick_next() {
                return Some(task);
            }
        }

        None
    }
}

lazy_static! {
    pub static ref TASK_SCHEDULER: RwLock<TaskScheduler> = RwLock::new(TaskScheduler::new());
}
