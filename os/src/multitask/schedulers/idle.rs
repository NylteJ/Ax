// use core::pin::Pin;

// use alloc::sync::Arc;

// use crate::multitask::{
//     SchedulePolicy,
//     schedulers::Scheduler,
//     task::{Task, TaskArc},
// };

// /// IDLE 调度器
// ///
// /// 恒定返回 idle 任务，不允许添加任务
// pub struct IdleScheduler {
//     task: &'static Task,
// }

// impl Scheduler for IdleScheduler {
//     fn add_task(&mut self, _task: TaskArc) -> Result<(), ()> {
//         Err(())
//     }

//     fn pick_next(&mut self) -> Option<TaskArc> {
//         // TODO
//         None
//     }

//     fn on_time_interrupt(&mut self, _task: &Task) -> Result<Option<TaskArc>, ()> {
//         Ok(None) // no-op
//     }
// }

// impl IdleScheduler {
//     pub fn new() -> Self {
//         let task = Task::new(SchedulePolicy::Normal, 114, 0); // TODO

//         // SAFETY: 我们稍后会直接 leak 它，因此其地址不会移动
//         let task = unsafe { Pin::into_inner_unchecked(task) };

//         let task = Arc::into_raw(task);

//         // SAFETY: task 由 Arc 创建，因此保证有效；同时，由于引用计数 > 0，task 永远不会被回收。你自由了！
//         let task: &'static _ = unsafe { task.as_ref_unchecked() };

//         Self { task }
//     }
// }
