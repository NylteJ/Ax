pub trait Chrono {
    type CpuTickType: Copy;

    fn get_cpu_tick() -> Self::CpuTickType;

    /// 获取时钟频率（单位：Hz）
    ///
    /// 此值应当是一个常数。这意味着此函数可能只会被调用一次。
    fn get_clock_freq() -> Self::CpuTickType;

    /// 启动时钟中断。此函数只应被调用一次，后续应当使用 `reset_timer_interrupt`。
    fn start_timer_tick_tock(interrupt_per_sec: usize);
    /// 调整时钟中断使其仍会继续稳定触发
    fn reset_timer_interrupt(interrupt_per_sec: usize);
}
