use core::{mem::replace, ops::Bound, ptr::NonNull};

use alloc::collections::btree_map::BTreeMap;

use crate::{
    arch::{self, interfaces::Chrono},
    basic::{UnsafeInited, sync::Mutex},
    multitask::task::Task,
};

pub fn cpu_tick_to_us(tick: usize) -> usize {
    tick / (*CLOCK_FREQ / 1000000) // 想你了 std::chrono 😭
}

pub fn us_to_cpu_tick(us: usize) -> usize {
    us * (*CLOCK_FREQ / 1000000)
}

pub fn get_cpu_tick() -> usize {
    arch::NativeChrono::get_cpu_tick()
}

const TIMER_INTERRUPT_PER_SECOND: usize = 1000;

/// 启动时钟中断。此函数只应被调用一次，后续应当使用 `reset_timer_interrupt`。
pub fn start_timer_tick_tock() {
    arch::NativeChrono::start_timer_tick_tock(TIMER_INTERRUPT_PER_SECOND);
}

/// 调整时钟中断使其仍会继续稳定触发
pub fn reset_timer_interrupt() {
    arch::NativeChrono::reset_timer_interrupt(TIMER_INTERRUPT_PER_SECOND);
}

pub static CLOCK_FREQ: UnsafeInited<usize> = unsafe { UnsafeInited::uninit() };

pub fn init_clock_freq() {
    unsafe { UnsafeInited::init_unsafe(&CLOCK_FREQ, arch::NativeChrono::get_clock_freq()) };
}

struct TaskPtr(pub NonNull<Task>);
unsafe impl Send for TaskPtr {}
unsafe impl Sync for TaskPtr {}

static TIMERS: Mutex<BTreeMap<usize, TaskPtr>> = Mutex::new(BTreeMap::new());

pub fn wake_task_when(task: &Task, cpu_tick: usize) {
    let task = TaskPtr(NonNull::from(task));

    let mut timers = TIMERS.lock();

    let mut cursor = timers.lower_bound_mut(Bound::Excluded(&cpu_tick));
    debug_assert!(
        cursor.peek_prev().is_none_or(|(tick, _)| *tick <= cpu_tick)
            && cursor.peek_next().is_none_or(|(tick, _)| *tick > cpu_tick)
    );

    if cursor.peek_prev().is_none_or(|(tick, _)| *tick != cpu_tick) {
        unsafe { cursor.insert_after_unchecked(cpu_tick, task) };
    } else {
        let mut new_tick = cpu_tick + 1;
        while cursor
            .peek_next()
            .is_some_and(|(tick, _)| *tick == new_tick)
        {
            cursor.next();
            new_tick += 1;
        }

        debug_assert!(cursor.peek_next().is_none_or(|(tick, _)| *tick > new_tick));
        unsafe { cursor.insert_after_unchecked(new_tick, task) };
    }
}

pub fn check_timers() {
    let timeouted = {
        let mut timers = TIMERS.lock();
        let tick = get_cpu_tick();
        if timers
            .lower_bound(Bound::Excluded(&tick))
            .peek_prev()
            .is_none()
        {
            return;
        }
        let rest = timers.split_off(&tick);
        replace(&mut *timers, rest)
    };

    for task in timeouted.into_values() {
        unsafe { task.0.as_ref() }.try_wake();
    }
}
