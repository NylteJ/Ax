#[cfg(debug_assertions)]
use core::cell::Cell;
use core::cell::UnsafeCell;
use core::fmt::Debug;
use core::hint;
use core::mem::offset_of;
use core::ops::Deref;
use core::ops::DerefMut;
use core::ops::DerefPure;
use core::ptr::NonNull;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::AtomicU8;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;

use crate::basic::UnsafeInited;

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

    pub fn into_inner(self) -> T {
        self.value.into_inner()
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

#[derive(Default)]
pub struct Mutex<T: ?Sized> {
    flag: AtomicBool,
    #[cfg(debug_assertions)]
    debug_msg: Cell<Option<&'static core::panic::Location<'static>>>,
    value: UnsafeCell<T>,
}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            flag: AtomicBool::new(false),
            value: UnsafeCell::new(value),
            #[cfg(debug_assertions)]
            debug_msg: Cell::new(None),
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
        let guard = loop {
            if let Some(guard) = self.try_lock() {
                break guard;
            }

            #[cfg(debug_assertions)]
            panic!(
                "我们现在还不支持并行，因此一定是发生了死锁。上一个上锁位置:\n{:?}",
                self.debug_msg.take()
            );
            #[cfg(not(debug_assertions))]
            panic!("我们现在还不支持并行，因此一定是发生了死锁");
            hint::spin_loop();
        };

        #[cfg(debug_assertions)]
        self.debug_msg.set(Some(core::panic::Location::caller()));

        guard
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self.flag.swap(true, Ordering::Acquire) {
            None
        } else {
            Some(MutexGuard {
                value: unsafe { NonNull::new_unchecked(self.value.get()) },
                flag: &self.flag,
            })
        }
    }

    pub fn as_ptr(&self) -> *mut T {
        self.value.get()
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

impl<T: ?Sized + Debug> Debug for Mutex<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut f = f.debug_struct("Mutex");

        if let Some(guard) = self.try_lock() {
            f.field("value", &guard.deref());
        } else {
            f.field("value", &"<locked>");
        }

        f.finish()
    }
}

/// 安全版本的 [`UnsafeInited`]，只能初始化一次且初始化期间不能被访问。能任意并发访问。
///
/// 虽然其正确实现了 [`Drop`]，但其主要设计用途依旧是用作全局静态变量
pub struct OnceInited<T> {
    inited: AtomicU8,
    value: UnsafeInited<T>,
}

impl<T> Deref for OnceInited<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        if self.inited.load(Ordering::Acquire) != 2 {
            panic!("OnceInited 还未被初始化");
        }

        self.value.deref()
    }
}

impl<T> OnceInited<T> {
    pub const fn new() -> Self {
        Self {
            inited: AtomicU8::new(0),
            value: unsafe { UnsafeInited::uninit() },
        }
    }

    pub fn init(this: &Self, value: T) {
        this.inited
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .expect("OnceInited 只能被初始化一次");

        unsafe { UnsafeInited::init_unsafe(&this.value, value) };

        this.inited.store(2, Ordering::Release);
    }
}

impl<T> Drop for OnceInited<T> {
    fn drop(&mut self) {
        if *self.inited.get_mut() == 2 {
            unsafe { UnsafeInited::drop(&mut self.value) };
        }
    }
}
