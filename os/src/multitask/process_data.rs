#[derive(Debug)]
pub struct ProcessData {
    /// 主线程的 TID，也是进程（线程组）的 PID
    pub pid: usize,
    /// 进程组 ID
    pub pgid: usize,
}

impl ProcessData {
    pub fn new(pid: usize, pgid: usize) -> Self {
        Self { pid, pgid }
    }
}
