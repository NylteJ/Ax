pub mod futex;
pub mod signal;
pub mod task;
pub mod task_arc;
pub mod task_context;
pub mod task_manager;
pub mod task_scheduler;
pub mod wait;

mod fdtable;
mod fs_data;
mod process_data;
mod schedulers;

use core::sync::atomic::{AtomicU8, Ordering};

pub use fdtable::{FDTableEntry, FileDescriptorFlags, InsertFDError};

type TaskPriority = u8;

pub const REALTIME_PRIORITY_COUNT: usize = 100; // 0 ~ 99
pub const NORMAL_PRIORITY_COUNT: usize = 40; // 100 ~ 139

static_assertions::const_assert!(
    REALTIME_PRIORITY_COUNT + NORMAL_PRIORITY_COUNT <= (1 << TaskPriority::BITS) as usize
);

#[derive(Debug)]
pub enum SchedulePolicy {
    // 实时
    FIFO,
    RoundRobin { rest_time_slice: AtomicU8 },
    // 公平
    Normal,
}

impl Clone for SchedulePolicy {
    fn clone(&self) -> Self {
        match self {
            Self::FIFO => Self::FIFO,
            Self::RoundRobin { rest_time_slice } => Self::RoundRobin {
                rest_time_slice: AtomicU8::new(rest_time_slice.load(Ordering::Relaxed)),
            },
            Self::Normal => Self::Normal,
        }
    }
}

/// 获取指向当前 Task 结构体的指针。如果上下文不正确，返回值不能被安全使用。
#[inline(always)]
pub fn current_task() -> *const task::Task {
    use crate::arch::{self, interfaces::Regs};

    arch::NativeRegs::get_tp() as _
}

/// 获取当前 Task 结构体。
///
/// 它完全等价于 `unsafe { current_task().as_ref_unchecked() }` 并且本身并不安全，请小心使用。
#[inline(always)]
pub fn cur_task<'a>() -> &'a task::Task {
    unsafe { current_task().as_ref_unchecked() }
}
