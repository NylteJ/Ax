use alloc::collections::vec_deque::VecDeque;
use bitvec::{BitArr, array::BitArray};

use crate::{
    basic::shared_with_asm::SharedWithAsm,
    multitask::{
        REALTIME_PRIORITY_COUNT, SchedulePolicy,
        schedulers::Scheduler,
        task::{Task, TaskArc},
    },
};

/// 实时调度器
///
/// 只存放实时优先级（0 ~ 99）的任务。优先调度高优先级任务，只在没有高优先级任务时调度低优先级任务。
pub struct RealtimeScheduler {
    tasks: [VecDeque<TaskArc>; REALTIME_PRIORITY_COUNT],
    has_tasks: BitArr!(for REALTIME_PRIORITY_COUNT),
}

impl Scheduler for RealtimeScheduler {
    fn add_task(&mut self, task: TaskArc) -> Result<(), ()> {
        let (SchedulePolicy::FIFO | SchedulePolicy::RoundRobin) = task.policy else {
            return Err(());
        };

        // 调度策略不在这里体现，此处两种策略没有区别

        let priority = task.priority as usize;

        if self.tasks[priority].is_empty() {
            self.has_tasks.set(priority, true);
        }

        self.tasks[priority].push_back(task);
        Ok(())
    }

    fn pick_next(&mut self) -> Option<TaskArc> {
        self.has_tasks.first_one().map(|idx| {
            let queue = &mut self.tasks[idx];
            if queue.len() == 1 {
                self.has_tasks.set(idx, false);
            }
            queue.pop_front().unwrap()
        })
    }

    fn on_time_interrupt(&mut self, task: &Task) -> Result<Option<TaskArc>, ()> {
        match task.policy {
            SchedulePolicy::FIFO => Ok(None), // FIFO 不需要处理时间中断，跑到跑完
            SchedulePolicy::RoundRobin => {
                // 暂时只给跑一个时间片
                let queue = &mut self.tasks[task.priority as usize];

                let Some(next_task) = queue.pop_front() else {
                    return Ok(None); // 队列里没有其它同优先级的任务，继续跑即可（更低优先级的必须等我们跑完）
                };

                // 切换任务
                queue.push_back(unsafe { TaskArc::from_asm(task) });
                Ok(Some(next_task))
            }
            _ => Err(()),
        }
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
