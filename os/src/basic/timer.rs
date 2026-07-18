use core::{mem::replace, num::NonZero};

use intrusive_collections::{
    Bound, KeyAdapter, RBTree as IntrusiveRBTree, RBTreeLink, UnsafeRef, intrusive_adapter,
};

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

intrusive_adapter!(TaskTimerAdapter = UnsafeRef<Task>: Task { timer_rbtree_link => RBTreeLink });

fn timeout_tick(task: &Task) -> usize {
    *unsafe { task.timer_rbtree_key.get().as_ref_unchecked() }
}
fn set_timeout_tick(task: &Task, tick: usize) {
    *unsafe { task.timer_rbtree_key.get().as_mut_unchecked() } = tick;
}

impl<'a> KeyAdapter<'a> for TaskTimerAdapter {
    type Key = usize;
    fn get_key(&self, s: &'a Task) -> usize {
        timeout_tick(s)
    }
}

intrusive_adapter!(TaskIItimerRealAdapter = UnsafeRef<Task>: Task { itimer_real_rbtree_link => RBTreeLink });

impl<'a> KeyAdapter<'a> for TaskIItimerRealAdapter {
    type Key = usize;
    fn get_key(&self, s: &'a Task) -> usize {
        unsafe { s.itimer_real.get().as_ref_unchecked().unwrap_unchecked() }.0
    }
}

static TIMERS: UnsafeInited<Mutex<IntrusiveRBTree<TaskTimerAdapter>>> =
    unsafe { UnsafeInited::uninit() };

struct RemoveTimerGuard<'a> {
    task: &'a Task,
}

impl<'a> Drop for RemoveTimerGuard<'a> {
    fn drop(&mut self) {
        let mut timers = TIMERS.lock();

        if !self.task.timer_rbtree_link.is_linked() {
            return; // 说明已经超时唤醒过了
        }

        unsafe {
            timers
                .cursor_mut_from_ptr(self.task)
                .remove()
                .unwrap_unchecked()
        };
    }
}

static ITIMERS_REAL: UnsafeInited<Mutex<IntrusiveRBTree<TaskIItimerRealAdapter>>> =
    unsafe { UnsafeInited::uninit() };

pub fn init() {
    unsafe {
        UnsafeInited::init_unsafe(
            &TIMERS,
            Mutex::new(IntrusiveRBTree::new(TaskTimerAdapter::new())),
        )
    };

    unsafe {
        UnsafeInited::init_unsafe(
            &ITIMERS_REAL,
            Mutex::new(IntrusiveRBTree::new(TaskIItimerRealAdapter::new())),
        )
    };
}

#[must_use = "返回值是 RAII 守卫"]
pub fn wake_task_when(task: &Task, cpu_tick: usize) -> impl Drop + '_ {
    let task_uref = unsafe { UnsafeRef::from_raw(task) };

    let mut timers = TIMERS.lock();

    let mut cursor = timers.lower_bound_mut(Bound::Included(&cpu_tick));
    debug_assert!(
        cursor
            .peek_prev()
            .get()
            .is_none_or(|task| timeout_tick(task) < cpu_tick)
            && cursor
                .get()
                .is_none_or(|task| timeout_tick(task) >= cpu_tick)
    );

    let mut actual_tick = cpu_tick;
    while cursor
        .get()
        .is_some_and(|task| timeout_tick(task) == actual_tick)
    {
        cursor.move_next();
        actual_tick += 1;
    }

    debug_assert!(
        cursor
            .peek_prev()
            .get()
            .is_none_or(|task| timeout_tick(task) < actual_tick)
            && cursor
                .get()
                .is_none_or(|task| timeout_tick(task) > actual_tick)
    );
    set_timeout_tick(task, actual_tick);
    cursor.insert_before(task_uref);

    return RemoveTimerGuard { task };
}

pub fn check_timers() {
    let mut timers = TIMERS.lock();
    let tick = get_cpu_tick();

    // 注意这里需要一直持有锁，否则我们还得用非侵入式容器盛装超时的任务（不然 RemoveTimerGuard 会认为 Task 还在红黑树中）
    let mut cursor = timers.front_mut();
    while cursor.get().is_some_and(|task| timeout_tick(task) <= tick) {
        cursor.remove().unwrap().try_wake();
    }
}

/// # Safety
/// `new_value` 为 [`None`]，或者调用方必须保证 `task` 在定时器到期或主动取消（通过 [`None`] 调用）前保持有效
pub unsafe fn set_itimer_real(
    task: &Task,
    new_value: Option<(usize, Option<NonZero<usize>>)>,
) -> Option<(usize, Option<NonZero<usize>>)> {
    let mut itimers_real = ITIMERS_REAL.lock();

    let old_value = replace(
        unsafe { task.itimer_real.get().as_mut_unchecked() },
        new_value,
    );

    if old_value.is_some() {
        unsafe {
            itimers_real
                .cursor_mut_from_ptr(task)
                .remove()
                .unwrap_unchecked()
        };
    }

    if new_value.is_some() {
        itimers_real.insert(unsafe { UnsafeRef::from_raw(task) });
    }

    old_value
}

pub fn get_itimer_real(task: &Task) -> Option<(usize, Option<NonZero<usize>>)> {
    let _guard = ITIMERS_REAL.lock();
    unsafe { *task.itimer_real.get() }
}

pub fn check_itimers_real() {
    let mut itimers_real = ITIMERS_REAL.lock();
    let tick = get_cpu_tick();

    let mut reinsert_list =
        intrusive_collections::SinglyLinkedList::new(TaskIItimerRealAdapter::new());

    let mut cursor = itimers_real.front_mut();
    loop {
        let Some(task) = cursor.get() else {
            break;
        };

        let itimer_real = unsafe { task.itimer_real.get().as_mut_unchecked() };
        let (expired_tick, interval) = unsafe { itimer_real.as_mut().unwrap_unchecked() };

        if *expired_tick > tick {
            break;
        }

        task.process
            .send_signal(crate::multitask::signal::SignalId::SIGALRM);

        let task = unsafe { cursor.remove().unwrap_unchecked() };

        if let Some(interval) = interval {
            *expired_tick += interval.get();
            reinsert_list.push_front(task);
        } else {
            *itimer_real = None;
        }
    }

    for task in reinsert_list {
        itimers_real.insert(task);
    }
}

pub fn check_all_timers() {
    check_timers();
    check_itimers_real();
}
