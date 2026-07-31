use core::cell::UnsafeCell;
use core::fmt::Debug;
use core::mem::ManuallyDrop;
use core::mem::offset_of;
use core::ops::Deref;
use core::ops::DerefMut;
use core::ops::DerefPure;
use core::ptr::NonNull;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;

use crate::syscall;

#[derive(Default)]
pub struct RwLockBase<T: ?Sized, const SPIN: bool> {
    state: AtomicU32,
    value: UnsafeCell<T>,
}

impl<T, const SPIN: bool> RwLockBase<T, SPIN> {
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            value: UnsafeCell::new(value),
        }
    }

    pub const fn from_value(val: NonNull<T>) -> NonNull<Self> {
        unsafe { val.byte_sub(offset_of!(Self, value)).cast() }
    }

    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: ?Sized, const SPIN: bool> RwLockBase<T, SPIN> {
    const WRITER_BIT: u32 = 1 << 31;

    #[track_caller]
    pub fn read(&self) -> ReadGuardBase<'_, T, SPIN> {
        loop {
            if let Some(guard) = self.try_read() {
                break guard;
            }

            if SPIN {
                // 内核态无抢占（自旋锁临界区内不会发生任务切换），因此持锁者要么在另一个核上
                // 运行并即将释放锁，要么就是死锁
                core::hint::spin_loop();
            } else {
                syscall::basic::yield_();
            }
        }
    }

    #[track_caller]
    pub fn try_read(&self) -> Option<ReadGuardBase<'_, T, SPIN>> {
        self.state
            .fetch_update(Ordering::Acquire, Ordering::Relaxed, |state| {
                if state != Self::WRITER_BIT {
                    Some(state + 1)
                } else {
                    None
                }
            })
            .ok()
            .map(|_| ReadGuardBase {
                value: unsafe { NonNull::new_unchecked(self.value.get()) },
                state: &self.state,
            })
    }

    #[track_caller]
    pub fn write(&self) -> WriteGuardBase<'_, T, SPIN> {
        while self
            .state
            .compare_exchange_weak(0, Self::WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            if SPIN {
                // 内核态无抢占（自旋锁临界区内不会发生任务切换），因此持锁者要么在另一个核上
                // 运行并即将释放锁，要么就是死锁
                core::hint::spin_loop();
            } else {
                syscall::basic::yield_();
            }
        }

        WriteGuardBase {
            value: unsafe { NonNull::new_unchecked(self.value.get()) },
            state: &self.state,
        }
    }

    pub fn as_ptr(&self) -> *mut T {
        self.value.get()
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

pub struct ReadGuardBase<'a, T: ?Sized, const SPIN: bool> {
    value: NonNull<T>,
    state: &'a AtomicU32,
}
pub struct WriteGuardBase<'a, T: ?Sized, const SPIN: bool> {
    value: NonNull<T>,
    state: &'a AtomicU32,
}

impl<T: ?Sized, const SPIN: bool> Drop for ReadGuardBase<'_, T, SPIN> {
    fn drop(&mut self) {
        self.state.fetch_sub(1, Ordering::Release);
    }
}
impl<T: ?Sized, const SPIN: bool> Drop for WriteGuardBase<'_, T, SPIN> {
    fn drop(&mut self) {
        self.state.store(0, Ordering::Release);
    }
}

impl<T: ?Sized, const SPIN: bool> Deref for ReadGuardBase<'_, T, SPIN> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}
impl<T: ?Sized, const SPIN: bool> Deref for WriteGuardBase<'_, T, SPIN> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}
impl<T: ?Sized, const SPIN: bool> DerefMut for WriteGuardBase<'_, T, SPIN> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.value.as_mut() }
    }
}

unsafe impl<T: ?Sized, const SPIN: bool> DerefPure for ReadGuardBase<'_, T, SPIN> {}
unsafe impl<T: ?Sized, const SPIN: bool> DerefPure for WriteGuardBase<'_, T, SPIN> {}

impl<'a, T: ?Sized, const SPIN: bool> WriteGuardBase<'a, T, SPIN> {
    #[must_use]
    pub fn downgrade(self) -> ReadGuardBase<'a, T, SPIN> {
        let this = ManuallyDrop::new(self);
        this.state.store(1, Ordering::Release);
        ReadGuardBase {
            value: this.value,
            state: this.state,
        }
    }
}

impl<'a, T: ?Sized, const SPIN: bool> ReadGuardBase<'a, T, SPIN> {
    #[must_use]
    pub fn try_upgrade(self) -> Result<WriteGuardBase<'a, T, SPIN>, Self> {
        let cas_result = self.state.compare_exchange(
            1,
            RwLockBase::<T, SPIN>::WRITER_BIT,
            Ordering::Acquire,
            Ordering::Relaxed,
        );

        if cas_result.is_err() {
            return Err(self);
        }

        let this = ManuallyDrop::new(self);
        Ok(WriteGuardBase {
            value: this.value,
            state: this.state,
        })
    }

    #[must_use]
    pub fn upgrade(mut self) -> WriteGuardBase<'a, T, SPIN> {
        loop {
            match self.try_upgrade() {
                Ok(guard) => break guard,
                Err(this) => self = this,
            }

            if SPIN {
                // 内核态无抢占（自旋锁临界区内不会发生任务切换），因此持锁者要么在另一个核上
                // 运行并即将释放锁，要么就是死锁
                core::hint::spin_loop();
            } else {
                syscall::basic::yield_();
            }
        }
    }
}

unsafe impl<T: ?Sized + Send + Sync, const SPIN: bool> Sync for RwLockBase<T, SPIN> {}
unsafe impl<T: ?Sized + Send, const SPIN: bool> Send for RwLockBase<T, SPIN> {}

impl<T: ?Sized + Debug, const SPIN: bool> Debug for RwLockBase<T, SPIN> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut f = f.debug_struct("RwLockBase");

        if let Some(guard) = self.try_read() {
            f.field("value", &guard.deref());
        } else {
            f.field("value", &"<writting>");
        }

        f.finish()
    }
}

/// 睡眠读写锁
pub type RwLock<T> = RwLockBase<T, false>;
/// 自旋读写锁
pub type SpinRwLock<T> = RwLockBase<T, true>;

pub type RwLockReadGuard<'a, T> = ReadGuardBase<'a, T, false>;
pub type SpinRwLockReadGuard<'a, T> = ReadGuardBase<'a, T, true>;

pub type RwLockWriteGuard<'a, T> = WriteGuardBase<'a, T, false>;
pub type SpinRwLockWriteGuard<'a, T> = WriteGuardBase<'a, T, true>;
