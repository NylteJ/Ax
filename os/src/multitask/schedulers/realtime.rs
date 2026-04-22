use core::pin::Pin;

use alloc::{collections::vec_deque::VecDeque, sync::Arc};
use bitvec::{BitArr, array::BitArray};

use crate::multitask::{
    REALTIME_PRIORITY_COUNT, SchedulePolicy, schedulers::Scheduler, task::Task,
};

/// 实时调度器
///
/// 只存放实时优先级（0 ~ 99）的任务。优先调度高优先级任务，只在没有高优先级任务时调度低优先级任务。
pub struct RealtimeScheduler {
    tasks: [VecDeque<Pin<Arc<Task>>>; REALTIME_PRIORITY_COUNT],
    has_tasks: BitArr!(for REALTIME_PRIORITY_COUNT),
}

impl Scheduler for RealtimeScheduler {
    fn add_task(&mut self, task: Pin<Arc<Task>>) -> Result<(), ()> {
        let (SchedulePolicy::FIFO | SchedulePolicy::RoundRobin) = task.policy else {
            return Err(());
        };

        // 调度策略不在这里体现，此处两种策略没有区别

        let priority = task.priority as usize;
        assert!(priority < REALTIME_PRIORITY_COUNT);

        if self.tasks[priority].is_empty() {
            self.has_tasks.set(priority, true);
        }

        self.tasks[priority].push_back(task);
        Ok(())
    }

    fn pick_next(&mut self) -> Option<Pin<Arc<Task>>> {
        self.has_tasks.first_one().map(|idx| {
            let queue = &mut self.tasks[idx];
            if queue.len() == 1 {
                self.has_tasks.set(idx, false);
            }
            queue.pop_front().unwrap()
        })
    }
}

impl RealtimeScheduler {
    pub fn new() -> Self {
        Self {
            tasks: [const { VecDeque::new() }; REALTIME_PRIORITY_COUNT],
            has_tasks: BitArray::ZERO,
        }
    }
}
