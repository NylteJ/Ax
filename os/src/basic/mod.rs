pub mod console;
pub mod debugger;
pub mod global_allocator;
pub mod logger;
pub mod sbi;
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
pub unsafe fn extend_lifetime_mut<'a, T: ?Sized>(rf: &'_ mut T) -> &'a mut T {
    unsafe { core::ptr::NonNull::from_mut(rf).as_mut() }
}
