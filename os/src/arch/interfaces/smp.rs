/// SMP（多核）相关操作
pub unsafe trait Smp {
    /// 启动所有其它 CPU。只能由启动核调用。
    ///
    /// 返回时各次要 CPU 可能仍在启动过程中，但它们最终都会进入各自空闲任务的循环等待调度。
    fn start_secondary_cpus();
}
