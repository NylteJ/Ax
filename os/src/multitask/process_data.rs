use core::{
    ops::{Index, IndexMut},
    sync::atomic::{AtomicBool, AtomicUsize},
};

use intrusive_collections::{LinkedListLink, UnsafeRef, intrusive_adapter};

use crate::{
    basic::sync::{SpinMutex, SpinRwLock},
    multitask::{
        signal::{SignalId, SignalsWaitingList},
        task::{INIT_PID, Task},
        task_manager::PID_COUNT,
        wait::WaitQueue,
    },
    syscall::basic::{RLimit, RLimitResource},
};

#[derive(Debug)]
pub struct ProcessData {
    /// 主线程的 TID，也是进程（线程组）的 PID
    pub pid: usize,
    /// 进程组 ID
    pub pgid: AtomicUsize,
    /// 会话 ID (session ID)
    pub sid: AtomicUsize,

    pub tasks: SpinRwLock<intrusive_collections::LinkedList<TaskOfProcessAdapter>>,

    /// 是否正在退出（exit_group）
    pub is_exiting: AtomicBool,

    pub rlimits: SpinRwLock<ProcessRLimits>,

    pub waiting_for_child: WaitQueue,

    /// 线程组级别的信号等待队列，其掩码无作用，线程始终用自己的掩码从中取信号
    pub signals_wl: SpinMutex<SignalsWaitingList>,
}

intrusive_adapter!(pub TaskOfProcessAdapter = UnsafeRef<Task>: Task { task_of_process_link => LinkedListLink });

impl ProcessData {
    pub fn new(pid: usize, pgid: usize, sid: usize, rlimits: ProcessRLimits) -> Self {
        Self {
            pid,
            pgid: AtomicUsize::new(pgid),
            sid: AtomicUsize::new(sid),
            tasks: SpinRwLock::new(intrusive_collections::LinkedList::new(
                TaskOfProcessAdapter::new(),
            )),
            is_exiting: AtomicBool::new(false),
            rlimits: SpinRwLock::new(rlimits),
            waiting_for_child: WaitQueue::new(),
            signals_wl: SpinMutex::new(SignalsWaitingList::new()),
        }
    }

    pub fn send_signal(&self, signal: SignalId) {
        if self.pid == INIT_PID {
            // init 进程不处理任何信号
            return;
        }

        self.signals_wl.lock().add(signal);

        for task in self.tasks.read().iter() {
            // FIXME: 这里其实会有 TOCTOU，先跑着看看
            if !task.signals_wl.lock().mask.contains(signal.as_mask()) {
                task.try_interrupt_sleep();
                break;
            }
        }
    }
}

#[cfg(debug_assertions)]
impl Drop for ProcessData {
    fn drop(&mut self) {
        assert!(self.tasks.get_mut().is_empty());
        log::trace!("process {} dropped", self.pid);
    }
}

const RLIMITS_COUNT: usize = RLimitResource::MAX as usize + 1;

#[derive(Debug, Clone)]
pub struct ProcessRLimits {
    pub limits: [RLimit; RLIMITS_COUNT],
}

impl ProcessRLimits {
    pub const fn unlimited() -> Self {
        Self {
            limits: [RLimit::unlimited(); RLIMITS_COUNT],
        }
    }
}

impl Index<RLimitResource> for ProcessRLimits {
    type Output = RLimit;

    fn index(&self, index: RLimitResource) -> &Self::Output {
        &self.limits[index as usize]
    }
}

impl IndexMut<RLimitResource> for ProcessRLimits {
    fn index_mut(&mut self, index: RLimitResource) -> &mut Self::Output {
        &mut self.limits[index as usize]
    }
}

impl Default for ProcessRLimits {
    fn default() -> Self {
        let mut limits = Self::unlimited();

        use RLimitResource::*;
        limits[RLIMIT_CORE] = RLimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        limits[RLIMIT_NOFILE] = RLimit {
            rlim_cur: 1024,
            rlim_max: 65536,
        };
        limits[RLIMIT_NPROC] = RLimit {
            rlim_cur: 512,
            rlim_max: PID_COUNT - 1,
        };

        limits
    }
}
