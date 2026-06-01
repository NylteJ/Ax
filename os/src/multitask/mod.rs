pub mod task;
pub mod task_context;
pub mod task_manager;
pub mod task_scheduler;

mod fdtable;
mod fs_data;
mod id_allocator;
mod process_data;
mod schedulers;

pub use fdtable::{FDTableEntry, FileDescriptorFlags};

type TaskPriority = u8;

pub const REALTIME_PRIORITY_COUNT: usize = 100; // 0 ~ 99
pub const NORMAL_PRIORITY_COUNT: usize = 40; // 100 ~ 139

static_assertions::const_assert!(
    REALTIME_PRIORITY_COUNT + NORMAL_PRIORITY_COUNT <= (1 << TaskPriority::BITS) as usize
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulePolicy {
    // 实时
    FIFO,
    RoundRobin,
    // 公平
    Normal,
}

/// 获取指向当前 Task 结构体的指针。如果上下文不正确，返回值不能被安全使用。
///
/// 顺带一提，不设计成返回引用的形式是因为很难说清它的生命周期（'static 显然会带来数不尽的安全隐患）。
#[inline]
pub fn current_task() -> *const task::Task {
    use crate::arch::{self, interfaces::Regs};

    arch::NativeRegs::get_tp() as _
}
