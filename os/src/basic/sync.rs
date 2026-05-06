#[cfg(debug_assertions)]
use core::cell::Cell;
use core::cell::UnsafeCell;
use core::hint;
use core::mem::offset_of;
use core::ops::Deref;
use core::ops::DerefMut;
use core::ops::DerefPure;
use core::ptr::NonNull;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;

#[cfg(debug_assertions)]
use alloc::string::String;
use alloc::string::ToString;

#[derive(Debug)]
pub struct RwLock<T: ?Sized> {
    state: AtomicU32,
    value: UnsafeCell<T>,
}

impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            value: UnsafeCell::new(value),
        }
    }

    pub const fn from_value(val: NonNull<T>) -> NonNull<Self> {
        unsafe { val.byte_sub(offset_of!(Self, value)).cast() }
    }
}

impl<T: ?Sized> RwLock<T> {
    const WRITER_BIT: u32 = 1 << 31;

    #[track_caller]
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state != Self::WRITER_BIT
                && self
                    .state
                    .compare_exchange_weak(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                break;
            }

            panic!("我们现在还不支持并行，因此一定是发生了死锁");
            hint::spin_loop();
        }

        RwLockReadGuard {
            value: unsafe { NonNull::new_unchecked(self.value.get()) },
            state: &self.state,
        }
    }

    #[track_caller]
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        while self
            .state
            .compare_exchange_weak(0, Self::WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            panic!("我们现在还不支持并行，因此一定是发生了死锁");
            hint::spin_loop();
        }

        RwLockWriteGuard {
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

pub struct RwLockReadGuard<'a, T: ?Sized> {
    value: NonNull<T>,
    state: &'a AtomicU32,
}
pub struct RwLockWriteGuard<'a, T: ?Sized> {
    value: NonNull<T>,
    state: &'a AtomicU32,
}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        self.state.fetch_sub(1, Ordering::Release);
    }
}
impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        self.state.store(0, Ordering::Release);
    }
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}
impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}
impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.value.as_mut() }
    }
}

unsafe impl<T: ?Sized> DerefPure for RwLockReadGuard<'_, T> {}
unsafe impl<T: ?Sized> DerefPure for RwLockWriteGuard<'_, T> {}

// SAFETY: 目前我们不支持并行，故而无需担心数据竞争
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}
unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}

pub struct Mutex<T: ?Sized> {
    flag: AtomicBool,
    #[cfg(debug_assertions)]
    debug_msg: Cell<String>,
    value: UnsafeCell<T>,
}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            flag: AtomicBool::new(false),
            value: UnsafeCell::new(value),
            #[cfg(debug_assertions)]
            debug_msg: Cell::new(String::new()),
        }
    }

    pub const fn from_value(val: NonNull<T>) -> NonNull<Self> {
        unsafe { val.byte_sub(offset_of!(Self, value)).cast() }
    }

    pub const fn to_value(this: NonNull<Self>) -> NonNull<T> {
        unsafe { this.byte_add(offset_of!(Self, value)).cast() }
    }
}

impl<T: ?Sized> Mutex<T> {
    #[track_caller]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        while self.flag.swap(true, Ordering::Acquire) {
            #[cfg(debug_assertions)]
            panic!(
                "我们现在还不支持并行，因此一定是发生了死锁。上一个上锁位置:\n{}",
                self.debug_msg.take()
            );
            #[cfg(not(debug_assertions))]
            panic!("我们现在还不支持并行，因此一定是发生了死锁");
            hint::spin_loop();
        }

        #[cfg(debug_assertions)]
        self.debug_msg
            .set(core::panic::Location::caller().to_string());

        MutexGuard {
            value: unsafe { NonNull::new_unchecked(self.value.get()) },
            flag: &self.flag,
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

pub struct MutexGuard<'a, T: ?Sized> {
    value: NonNull<T>,
    flag: &'a AtomicBool,
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.value.as_mut() }
    }
}

unsafe impl<T: ?Sized> DerefPure for MutexGuard<'_, T> {}

// SAFETY: 目前我们不支持并行，故而无需担心数据竞争
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
