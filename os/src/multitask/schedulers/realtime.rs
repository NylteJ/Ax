use core::sync::atomic::Ordering;

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
    fn add_task(&mut self, task: TaskArc) -> Result<(), TaskArc> {
        let (SchedulePolicy::FIFO | SchedulePolicy::RoundRobin { .. }) = &task.policy else {
            return Err(task);
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
        let idx = self.has_tasks.first_one()?;
        let queue = &mut self.tasks[idx];

        // 任务在被抢占时会被先加入队列、后真正切换出去（二者之间存在一段“仍在运行却在队列中”的
        // 窗口）。若此时被其它核选走，就会导致同一任务同时在两个核上运行（用户内存被破坏）。
        // 因此跳过仍标记为运行中的任务，等其真正挂起后再调度。
        // 当前任务可能在等待被唤醒时仍留在队列中（例如 wait 的自唤醒：睡眠任务仍在自己
        // 的 do_sleep 循环里运行，唤醒者将其加入队列后，需要它自己把自己“拣回”来完成睡眠）。
        // 这种自拣回必须放行，否则任务将永远卡在 do_sleep 里。
        let cur = crate::multitask::current_task();
        for _ in 0..queue.len() {
            let task = queue.pop_front().unwrap();
            if task.running.load(Ordering::Relaxed)
                && !core::ptr::addr_eq(&*task as *const _ as *const (), cur)
            {
                queue.push_back(task);
                continue;
            }
            if queue.is_empty() {
                self.has_tasks.set(idx, false);
            }
            return Some(task);
        }

        None
    }

    fn on_time_interrupt(&mut self, task: &Task) -> Result<Option<TaskArc>, ()> {
        match &task.policy {
            SchedulePolicy::FIFO => Ok(None), // FIFO 不需要处理时间中断，跑到跑完
            SchedulePolicy::RoundRobin { rest_time_slice } => {
                let old_rest = rest_time_slice.fetch_sub(1, Ordering::Relaxed);
                debug_assert_ne!(old_rest, 0);
                if old_rest != 1 {
                    return Ok(None); // 时间片还没用完，继续跑
                }

                const SLICE_PER_TASK: u8 = 128;

                let old_rest = rest_time_slice.swap(SLICE_PER_TASK, Ordering::Relaxed);
                debug_assert_eq!(
                    old_rest, 0,
                    "这里不该有竞争（其实连原子变量都能直接省略的）"
                ); // TODO: 看看能不能真的省略

                let queue = &mut self.tasks[task.priority as usize];

                // 与 `pick_next` 相同的原因：跳过仍运行中的任务，避免同一任务被两个核同时运行
                let mut next_task = None;
                for _ in 0..queue.len() {
                    let candidate = queue.pop_front().unwrap();
                    if candidate.running.load(Ordering::Relaxed) {
                        queue.push_back(candidate);
                        continue;
                    }
                    next_task = Some(candidate);
                    break;
                }

                let Some(next_task) = next_task else {
                    return Ok(None); // 队列里没有其它可调度的同优先级任务，继续跑即可（更低优先级的必须等我们跑完）
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
    pub const fn new() -> Self {
        Self {
            tasks: [const { VecDeque::new() }; REALTIME_PRIORITY_COUNT],
            has_tasks: BitArray::ZERO,
        }
    }

}
