use core::cell::UnsafeCell;
use core::fmt::Debug;
use core::mem::offset_of;
use core::ops::Deref;
use core::ops::DerefMut;
use core::ops::DerefPure;
use core::ptr::NonNull;
use core::sync::atomic::AtomicBool;
#[cfg(debug_assertions)]
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering;

#[cfg(debug_assertions)]
use crate::multitask::current_task;
use crate::syscall;

#[derive(Default)]
pub struct MutexBase<T: ?Sized, const SPIN: bool> {
    flag: AtomicBool,
    #[cfg(debug_assertions)]
    task: AtomicUsize,
    value: UnsafeCell<T>,
}

impl<T, const SPIN: bool> MutexBase<T, SPIN> {
    pub const fn new(value: T) -> Self {
        Self {
            flag: AtomicBool::new(false),
            value: UnsafeCell::new(value),
            #[cfg(debug_assertions)]
            task: AtomicUsize::new(usize::MAX),
        }
    }

    pub const fn from_value(val: NonNull<T>) -> NonNull<Self> {
        unsafe { val.byte_sub(offset_of!(Self, value)).cast() }
    }

    pub const fn to_value(this: NonNull<Self>) -> NonNull<T> {
        unsafe { this.byte_add(offset_of!(Self, value)).cast() }
    }
}

impl<T: ?Sized, const SPIN: bool> MutexBase<T, SPIN> {
    #[track_caller]
    pub fn lock(&self) -> MutexGuardBase<'_, T, SPIN> {
        let guard = loop {
            if let Some(guard) = self.try_lock() {
                // 我们目前根本不开启内核态中断，因此自旋锁也不用关中断
                break guard;
            }

            // log::trace!("MutexBase {self:p} 被锁住了（警告：这可能是死锁的征兆！）");
            #[cfg(debug_assertions)]
            assert!(
                unsafe { current_task().as_ref() }
                    .is_none_or(|task| task.tid != self.task.load(Ordering::Relaxed)),
                "死锁了！TID = {}",
                self.task.load(Ordering::Relaxed)
            );

            if SPIN {
                // 内核态无抢占（自旋锁临界区内不会发生任务切换），因此持锁者要么在另一个核上
                // 运行并即将释放锁，要么就是死锁（由上面的断言捕获）
                core::hint::spin_loop();
            } else {
                // TODO：真正睡眠
                syscall::basic::yield_();
            }
        };

        #[cfg(debug_assertions)]
        unsafe { current_task().as_ref() }
            .inspect(|task| self.task.store(task.tid, Ordering::Relaxed));

        guard
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    pub fn try_lock(&self) -> Option<MutexGuardBase<'_, T, SPIN>> {
        if self.flag.swap(true, Ordering::Acquire) {
            None
        } else {
            Some(MutexGuardBase {
                value: unsafe { NonNull::new_unchecked(self.value.get()) },
                flag: &self.flag,
                #[cfg(debug_assertions)]
                task: &self.task,
            })
        }
    }

    pub fn as_ptr(&self) -> *mut T {
        self.value.get()
    }

    /// 假设锁已通过某种方式被释放，只是出于某种原因 [`MutexGuardBase`] 的析构函数暂时没有被执行（例如，如果 [`forget`][`core::mem::forget`] 了 guard，就会产生这种情况）
    ///
    /// 注意它不是解锁函数，也不执行任何内存同步。
    ///
    /// # Safety
    /// 在下一个 [`Self::lock`] 调用发生之后，原有的锁守卫（如果有）必须不再能被以任何方式使用（包括 drop，不包含移动）。
    ///
    /// 除此之外的操作都是安全的。例如，可以在未上锁的 [`MutexBase`] 上调用此函数，且调用此函数后仍然可以使用原锁守卫。
    pub unsafe fn assume_unlocked(&self) {
        self.flag.store(false, Ordering::Relaxed);
    }
}

pub struct MutexGuardBase<'a, T: ?Sized, const SPIN: bool> {
    value: NonNull<T>,
    flag: &'a AtomicBool,
    #[cfg(debug_assertions)]
    task: &'a AtomicUsize,
}

impl<T: ?Sized, const SPIN: bool> Drop for MutexGuardBase<'_, T, SPIN> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
        #[cfg(debug_assertions)]
        self.task.store(usize::MAX, Ordering::Relaxed);
    }
}

impl<T: ?Sized, const SPIN: bool> Deref for MutexGuardBase<'_, T, SPIN> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}

impl<T: ?Sized, const SPIN: bool> DerefMut for MutexGuardBase<'_, T, SPIN> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.value.as_mut() }
    }
}

unsafe impl<T: ?Sized, const SPIN: bool> DerefPure for MutexGuardBase<'_, T, SPIN> {}

impl<'a, T, const SPIN: bool> MutexGuardBase<'a, T, SPIN> {
    /// 解锁，然后获取对应 [`MutexBase`] 的不可变引用。
    ///
    /// 如果忽略返回值，则其完全等价于 [`drop`]。
    #[must_use]
    pub fn unlock_and_get_mutex(self) -> &'a MutexBase<T, SPIN> {
        let mtx = MutexBase::from_value(self.value);
        drop(self);

        unsafe { mtx.as_ref() }
    }
}

unsafe impl<T: ?Sized + Send, const SPIN: bool> Sync for MutexBase<T, SPIN> {}
unsafe impl<T: ?Sized + Send, const SPIN: bool> Send for MutexBase<T, SPIN> {}

impl<T: ?Sized + Debug, const SPIN: bool> Debug for MutexBase<T, SPIN> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut f = f.debug_struct("MutexBase");

        if let Some(guard) = self.try_lock() {
            f.field("value", &guard.deref());
        } else {
            f.field("value", &"<locked>");
        }

        f.finish()
    }
}

/// 睡眠互斥锁
pub type Mutex<T> = MutexBase<T, false>;
/// 自旋互斥锁
pub type SpinMutex<T> = MutexBase<T, true>;

/// 睡眠互斥锁守卫
pub type MutexGuard<'a, T> = MutexGuardBase<'a, T, false>;
/// 自旋互斥锁守卫
pub type SpinMutexGuard<'a, T> = MutexGuardBase<'a, T, true>;
