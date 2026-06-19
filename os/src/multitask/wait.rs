use core::{hint::black_box, num::NonZeroUsize};

use alloc::boxed::Box;
use intrusive_collections::{LinkedListLink, SinglyLinkedListLink, UnsafeRef, intrusive_adapter};

use crate::{
    basic::{
        sync::Mutex,
        timer::{get_cpu_tick, us_to_cpu_tick, wake_task_when},
    },
    multitask::{current_task, signal::SignalId, task::Task},
};

#[derive(Debug)]
pub struct WaitQueue {
    queue: Mutex<intrusive_collections::LinkedList<WaitEntryAdapter>>,
}

impl WaitQueue {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(intrusive_collections::LinkedList::new(
                WaitEntryAdapter::new(),
            )),
        }
    }

    pub fn wake(&self, count: usize) -> usize {
        self.queue
            .lock()
            .iter()
            .filter_map(|entry| entry.wake())
            .take(count)
            .count()
    }

    pub fn wake_all(&self) -> usize {
        self.wake(usize::MAX)
    }
}

impl Drop for WaitQueue {
    fn drop(&mut self) {
        self.wake_all();

        if self.queue.get_mut().is_empty() {
            return; // 快速路径，绝大部分析构都是这样的
        }

        // 这里需要绕一下，因为非常遗憾的拿着 &mut queue 轮询是 UB，编译器可以将其优化为死循环
        // 加上 black_box，queue 就有可能在 black_box 被复制从而产生别名，进而打破编译器假设
        // （没错，我们写 C++ 的是这样的）
        let queue = black_box(&self.queue);

        while !queue.lock().is_empty() {
            crate::syscall::basic::yield_();
        }
    }
}

#[derive(Debug)]
pub struct WaitEntry {
    task: *const Task,

    queue_node: LinkedListLink,
    wait_node: SinglyLinkedListLink,

    my_queue: *const WaitQueue,
}

unsafe impl Send for WaitEntry {}
unsafe impl Sync for WaitEntry {}

intrusive_adapter!(WaitEntryAdapter = Box<WaitEntry>: WaitEntry { queue_node => LinkedListLink });
intrusive_adapter!(WaitNodeAdapter = UnsafeRef<WaitEntry>: WaitEntry { wait_node => SinglyLinkedListLink });

impl WaitEntry {
    pub fn wake(&self) -> Option<()> {
        unsafe { self.task.as_ref_unchecked() }.try_wake()
    }
}

pub struct Waiting {
    wait_entries: intrusive_collections::SinglyLinkedList<WaitNodeAdapter>,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        for entry in self.wait_entries.take() {
            let queue = unsafe { entry.my_queue.as_ref_unchecked() };
            let mut queue_guard = queue.queue.lock();

            debug_assert!(queue_guard.iter().any(|e| core::ptr::eq(e, &*entry)));
            unsafe { queue_guard.cursor_mut_from_ptr(&*entry).remove() };
        }
    }
}

impl Waiting {
    pub fn new() -> Self {
        Self {
            wait_entries: intrusive_collections::SinglyLinkedList::new(WaitNodeAdapter::new()),
        }
    }

    pub fn wait(&mut self, queue: &WaitQueue) {
        let entry = Box::new(WaitEntry {
            task: current_task(),
            queue_node: LinkedListLink::new(),
            wait_node: SinglyLinkedListLink::new(),
            my_queue: queue,
        });

        self.wait_entries
            .push_front(unsafe { UnsafeRef::from_raw(&*entry) });

        queue.queue.lock().push_back(entry);
    }

    pub fn sleep(&mut self) -> Result<(), SleepError> {
        Task::sleep_current();

        // 和 Linux 一样，我们使用这种间接方式判断
        let cur_task = unsafe { current_task().as_ref_unchecked() };
        if let Some(signal) = cur_task.find_signal_to_consume() {
            return Err(SleepError::Interrupted(signal));
        }

        Ok(())
    }

    pub fn sleep_uninterruptible(&mut self) {
        Task::sleep_current_uninterruptible();
    }

    pub fn sleep_with_timeout(&mut self, timeout_us: usize) -> SleepWithTimeoutResult {
        let cur_task = unsafe { current_task().as_ref_unchecked() };

        let deadline_tick = get_cpu_tick() + us_to_cpu_tick(timeout_us);

        // 这里不需要担心竞态或者其它问题，因为计时器只会在处理时钟中断时触发，且我们不支持内核态抢占
        let _timeout_guard = wake_task_when(cur_task, deadline_tick);

        Task::sleep_current();

        let rest_tick = deadline_tick.saturating_sub(get_cpu_tick());
        let Some(rest) = NonZeroUsize::new(rest_tick) else {
            return SleepWithTimeoutResult::Timeout;
        };

        let result = if let Some(signal) = cur_task.find_signal_to_consume() {
            Err(SleepError::Interrupted(signal))
        } else {
            Ok(())
        };
        SleepWithTimeoutResult::Normal(rest, result)
    }
}

pub enum SleepError {
    Interrupted(SignalId),
}
#[must_use]
pub enum SleepWithTimeoutResult {
    Timeout,
    Normal(NonZeroUsize, Result<(), SleepError>),
}
