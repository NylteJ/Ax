use crate::{
    arch::{self, interfaces::Chrono},
    basic::UnsafeInited,
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
