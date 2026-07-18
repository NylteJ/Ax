use core::ops::Deref;
use core::sync::atomic::AtomicU8;
use core::sync::atomic::Ordering;

use crate::basic::UnsafeInited;

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
        assert!(Self::is_inited(self), "OnceInited 还未被初始化");
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

    pub fn is_inited(this: &Self) -> bool {
        this.inited.load(Ordering::Acquire) == 2
    }
}

impl<T> Drop for OnceInited<T> {
    fn drop(&mut self) {
        if *self.inited.get_mut() == 2 {
            unsafe { UnsafeInited::drop(&mut self.value) };
        }
    }
}
