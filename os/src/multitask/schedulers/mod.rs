use crate::multitask::task::{Task, TaskArc};

pub mod idle;
pub mod realtime;

pub trait Scheduler {
    fn add_task(&mut self, task: TaskArc) -> Result<(), ()>;

    fn pick_next(&mut self) -> Option<TaskArc>;

    /// 发生时钟中断时调用。返回 Ok 表示能处理这个 Task，反之表示无法处理这个 Task。如果返回的 Ok 携带了值，则表示需要调用方用 switch_context 切换到 Task。无需重复 add_task。
    fn on_time_interrupt(&mut self, task: &Task) -> Result<Option<TaskArc>, ()>; // 为什么 NonNull 是 mut 的 :(
}
