use core::sync::atomic::Ordering;

use crate::{
    arch,
    arch::interfaces::{Basic, Regs, Smp},
    basic::{shared_with_asm::SharedWithAsm, sync::SpinMutex, timer},
    multitask::{
        cur_task, current_task,
        task::{Task, TaskArc},
        task_scheduler::TASK_SCHEDULER,
    },
    trap,
};

/// 最大支持的 CPU（核）数量
///
/// QEMU 的 `-smp` 参数不应超过此值。
pub const MAX_CPUS: usize = 8;

/// 每核的空闲任务。索引为 CPU ID（RISC-V 上即 hartid）。
///
/// 空闲任务不会被放入调度器，它们只在对应 CPU 上无任务可跑时被切换过去。
/// 每个空闲任务只会被其所属的 CPU 使用，因此这里除了初始化时外不会产生竞争。
pub static IDLE_TASKS: SpinMutex<[Option<TaskArc>; MAX_CPUS]> =
    SpinMutex::new([const { None }; MAX_CPUS]);

/// 次要核的启动等待标志。
///
/// LoongArch 上所有核会同时从复位向量启动，次要核会在汇编中自旋等待此标志被启动核置位。
/// RISC-V 上次要核停留在 OpenSBI 中，不会用到它。
pub static SECONDARY_BOOT_FLAG: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// 获取当前运行所在 CPU 的 ID。
///
/// 该值在每个 CPU 切换任务时被写入对应任务，因此读取到的永远是当前 CPU 的 ID。
/// 注意：在还没有任何任务运行时（例如 `main` 中），此函数不可用。
#[inline(always)]
pub fn current_cpu_id() -> usize {
    cur_task().last_cpu.load(Ordering::Relaxed)
}

/// 为所有 CPU 创建空闲任务。只能由启动核在完成初始化后、启动其它核之前调用。
pub fn init() {
    for cpu in 0..MAX_CPUS {
        let idle_task = Task::create_idle_task(cpu);
        log::debug!("CPU {cpu} 的空闲任务已创建（TID = {}）", idle_task.tid);
        IDLE_TASKS.lock()[cpu] = Some(idle_task);
    }
}

/// 启动所有其它 CPU，并等待它们全部进入空闲循环。只能由启动核调用。
pub fn start_secondary_cpus() {
    arch::NativeSmp::start_secondary_cpus();
}

/// 空闲任务的主循环。此函数不会返回。
///
/// 当调度器为空时，重置时钟中断并停机等待；否则切换到调度器给出的任务。
/// 注意：每次切换离开空闲任务前，需要收回空闲任务自身所有权（引用计数），
/// 因为切换时（`Task::switch_to`）只会将目标任务的 Arc 转移到汇编侧。
pub extern "C" fn idle_loop() -> ! {
    loop {
        let next = TASK_SCHEDULER.lock().pick_next();
        match next {
            Some(task) => {
                // 收回空闲任务的所有权（它来自启动时或上一次切换时的 `into_asm`）
                let _ = unsafe { TaskArc::from_asm(current_task()) };
                Task::switch_to(task);
            }
            None => {
                // 重置时钟中断以便在约 1 ms 后再次被唤醒检查调度器
                timer::reset_timer_interrupt();
                arch::NativeBasic::halt();
            }
        }
    }
}

/// 由次要 CPU 的汇编入口调用。此函数不会返回。
///
/// 其负责完成仅针对当前 CPU 的初始化，然后进入空闲循环等待任务。
pub extern "C" fn secondary_entry(cpu_id: usize) -> ! {
    // 在进入空闲任务前，tp 还不是任何任务的指针（对刚启动的核而言其值未定义）。
    // 调试构建中的锁断言等会读取它，因此先置零
    arch::NativeRegs::set_tp(0);

    log::info!("CPU {cpu_id} 启动成功，正在进入空闲循环...");

    // 次要核需要单独完成与当前 CPU 相关的初始化

    // TLB 相关 CSR 是每核独立的
    #[cfg(target_arch = "loongarch64")]
    crate::memory::tlb_refill_handler::init_cpu();

    // 注意：次要核的定时器中断目前暂不启用（启用后存在损坏用户内存的 bug，待调查）
    // trap::enable_timer_interrupt();
    // timer::start_timer_tick_tock();

    // 将空闲任务的所有权转移到汇编侧，然后进入它
    let idle_task = IDLE_TASKS
        .lock()
        .get(cpu_id)
        .and_then(|slot| slot.as_ref())
        .unwrap_or_else(|| panic!("CPU {cpu_id} 没有空闲任务"))
        .clone();
    let raw = unsafe { TaskArc::into_asm(idle_task) };

    unsafe { Task::enter_idle_task(raw) };
    unreachable!()
}
