use core::pin::Pin;

use alloc::sync::Arc;

use crate::multitask::task::Task;

pub mod idle;
pub mod realtime;

pub trait Scheduler {
    fn add_task(&mut self, task: Pin<Arc<Task>>) -> Result<(), ()>;

    fn pick_next(&mut self) -> Option<Pin<Arc<Task>>>;
}
