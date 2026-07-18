use tuple_for_each::TupleForEach;

use crate::basic::sync::Mutex;
use crate::multitask::schedulers::realtime::RealtimeScheduler;
use crate::multitask::{
    schedulers::Scheduler,
    task::{Task, TaskArc},
};

#[derive(TupleForEach)]
struct Schedulers(RealtimeScheduler);

pub struct TaskScheduler {
    schedulers: Schedulers,
}

impl TaskScheduler {
    pub const fn new() -> Self {
        Self {
            schedulers: Schedulers(RealtimeScheduler::new()),
        }
    }

    pub fn add_task(&mut self, mut task: TaskArc) {
        let schedulers = &mut self.schedulers;
        schedulers_for_each!(scheduler in mut schedulers {
            match scheduler.add_task(task) {
                Ok(()) => return,
                Err(tsk) => task = tsk,
            }
        });

        panic!("无法为 Task {:?} 找到合适的调度器", *task);
    }

    pub fn pick_next(&mut self) -> Option<TaskArc> {
        let schedulers = &mut self.schedulers;
        schedulers_for_each!(scheduler in mut schedulers {
            if let Some(task) = scheduler.pick_next() {
                return Some(task);
            }
        });

        None
    }

    /// 发生时钟中断时调用。返回 Some 时调用方应当使用 switch_context 切换到返回的 Task。否则无需任何操作。
    pub fn on_time_interrupt(&mut self, task: &Task) -> Option<TaskArc> {
        // Performance Note: 或许考虑像 Linux 内核那样直接缓存一个指针（Rust 那么写起来太麻烦了 XP）？
        let schedulers = &mut self.schedulers;
        schedulers_for_each!(scheduler in mut schedulers {
            if let Ok(task) = scheduler.on_time_interrupt(task) {
                return task;
            }
        });

        panic!("无法为 Task {:?} 找到合适的调度器来处理时间中断", task);
    }
}

pub static TASK_SCHEDULER: Mutex<TaskScheduler> = Mutex::new(TaskScheduler::new());
