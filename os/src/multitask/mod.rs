use core::arch::asm;

pub mod task;
pub mod task_context;
pub mod task_manager;
pub mod task_scheduler;

mod id_allocator;
mod schedulers;

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
    RoundRobin,
    // 公平
    Normal,
}

/// 获取指向当前 Task 结构体的指针。如果上下文不正确，返回值不能被安全使用。
pub fn current_task() -> *mut task::Task {
    let tp;
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("mv {0}, tp", out(reg) tp) },
        target_arch = "loongarch64" => unsafe { asm!("move {0}, $tp", out(reg) tp) },
    };
    tp
}
