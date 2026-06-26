use core::num::NonZero;

pub mod bstr;
pub mod console;
pub mod debugger;
pub mod global_allocator;
pub mod logger;
pub mod lru;
pub mod shared_with_asm;
pub mod stack;
pub mod sync;
pub mod timer;

mod panic_handler;

/// 强行延长所给引用的生命周期，比 transmute 更安全，比手动转换更清晰
pub unsafe fn extend_lifetime<'a, T: ?Sized>(rf: &'_ T) -> &'a T {
    unsafe { core::ptr::NonNull::from_ref(rf).as_ref() }
}

/// 强行延长所给引用的生命周期，比 transmute 更安全，比手动转换更清晰
pub unsafe fn extend_lifetime_mut<'a, 'b, T: ?Sized>(rf: &'_ mut T) -> &'a mut T {
    unsafe { core::ptr::NonNull::from_mut(rf).as_mut() }
}

#[inline(always)]
pub const fn cold_path() {
    core::intrinsics::cold_path();
}

/// 一个在单次初始化后只读的包装器，提供方便的访问机制
pub struct UnsafeInited<T> {
    value: core::cell::SyncUnsafeCell<core::mem::MaybeUninit<T>>,
    #[cfg(debug_assertions)]
    inited: core::sync::atomic::AtomicBool,
}

impl<T> UnsafeInited<T> {
    /// 返回未初始化的 `UnsafeInited`。
    ///
    /// # Safety
    /// 调用者必须保证在调用 `init` 前不会访问 `value`。
    ///
    /// 它被标记为 unsafe 是为了访问能够不加 unsafe。
    pub const unsafe fn uninit() -> Self {
        Self {
            value: core::cell::SyncUnsafeCell::new(core::mem::MaybeUninit::uninit()),
            #[cfg(debug_assertions)]
            inited: core::sync::atomic::AtomicBool::new(false),
        }
    }
    /// 返回零初始化的 `UnsafeInited`。
    ///
    /// # Safety
    /// 调用者必须保证在调用 `init` 前不会访问 `value`，或者全零比特是 `T` 的合法值。
    ///
    /// 它被标记为 unsafe 是为了访问能够不加 unsafe。
    pub const unsafe fn zeroed() -> Self {
        Self {
            value: core::cell::SyncUnsafeCell::new(core::mem::MaybeUninit::zeroed()),
            #[cfg(debug_assertions)]
            inited: core::sync::atomic::AtomicBool::new(false),
        }
    }

    /// 初始化当前 `UnsafeInited`。
    ///
    /// 警告：多次调用它时，前几次调用初始化的值不会被 drop。
    pub fn init(this: &mut Self, value: T) {
        this.value.get_mut().write(value);

        #[cfg(debug_assertions)]
        this.inited
            .store(true, core::sync::atomic::Ordering::Relaxed);
    }

    /// 初始化当前 `UnsafeInited`。
    ///
    /// 警告：多次调用它时，前几次调用初始化的值不会被 drop。
    ///
    /// # Safety
    /// 调用者需保证不并发调用此函数。
    pub unsafe fn init_unsafe(this: &Self, value: T) {
        unsafe { this.value.get().as_mut_unchecked().write(value) };

        #[cfg(debug_assertions)]
        this.inited
            .store(true, core::sync::atomic::Ordering::Relaxed);
    }

    pub unsafe fn drop(this: &mut Self) {
        #[cfg(debug_assertions)]
        assert!(this.inited.load(core::sync::atomic::Ordering::Relaxed));

        unsafe { this.value.get_mut().assume_init_drop() };
    }
}

impl<T> core::ops::Deref for UnsafeInited<T> {
    type Target = T;

    #[track_caller]
    fn deref(&self) -> &Self::Target {
        #[cfg(debug_assertions)]
        assert!(
            self.inited.load(core::sync::atomic::Ordering::Relaxed),
            "访问了未初始化的 UnsafeInited"
        );

        use uninit::extension_traits::MaybeUninitExt;

        unsafe { self.value.get().as_ref_unchecked().assume_init_by_ref() }
    }
}

impl<'a, T: 'a> IntoIterator for &'a UnsafeInited<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;

    #[track_caller]
    fn into_iter(self) -> Self::IntoIter {
        use core::ops::Deref;
        self.deref().into_iter()
    }
}

pub struct DropGuard<F: FnOnce()> {
    func: core::mem::ManuallyDrop<F>,
}

impl<F: FnOnce()> DropGuard<F> {
    pub fn new(func: F) -> Self {
        Self {
            func: core::mem::ManuallyDrop::new(func),
        }
    }

    #[must_use]
    pub fn release(self) -> F {
        use core::mem::ManuallyDrop;

        let mut this = ManuallyDrop::new(self);
        unsafe { ManuallyDrop::take(&mut this.func) }
    }
}

impl<F: FnOnce()> Drop for DropGuard<F> {
    fn drop(&mut self) {
        let func = unsafe { core::mem::ManuallyDrop::take(&mut self.func) };

        func();
    }
}

#[cold]
pub fn cleanup_and_shutdown() -> ! {
    log::debug!("已进入清理并关机流程");

    crate::filesystem::filesystems::foreach_filesystem(|fs| fs.writeback(NonZero::<usize>::MAX));

    log::debug!("清理完毕，正式关机");
    use crate::arch::{self, interfaces::Basic};
    arch::NativeBasic::shutdown(true);
}

#[cold]
pub fn abort_and_shutdown() -> ! {
    use crate::arch::{self, interfaces::Basic};
    arch::NativeBasic::shutdown(false);
}
