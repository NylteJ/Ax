use core::{mem::forget_unsized, pin::Pin};

use alloc::{boxed::Box, sync::Arc};

/// 标记此类型将与汇编代码共享。
///
/// 这意味着它在某些时候可能完全不在 Rust 端可达。换言之，其所有权会在 Rust 端和汇编端之间转移。
pub unsafe trait SharedWithAsm {
    type AsmType: Copy;

    /// 将自身所有权转移给汇编端。如果返回值未妥善处理则可能引发资源泄露或内存不安全。
    ///
    /// # Safety
    /// 其本身看似安全，但由于其返回的原始类型通常无法有效表达 Rust 中的约束条件，不恰当的使用仍会导致问题。
    unsafe fn into_asm(self) -> Self::AsmType;
    /// 从汇编端获取所有权。输入值不正确则会导致内存不安全。
    ///
    /// # Safety
    /// 输入必须来源于 `into_asm` 的返回值，且每次 `into_asm` 必须恰好对应一次 `from_asm` 或 `drop_asm`。
    unsafe fn from_asm(asm_value: Self::AsmType) -> Self;
    /// 从汇编端转移并立即丢弃所有权。它在某些时候能带来更好的性能。输入值不正确则会导致内存不安全。
    ///
    /// # Safety
    /// 输入必须来源于 `into_asm` 的返回值，且每次 `into_asm` 必须恰好对应一次 `from_asm` 或 `drop_asm`。
    unsafe fn drop_asm(asm_value: Self::AsmType)
    where
        Self: Sized,
    {
        drop(unsafe { Self::from_asm(asm_value) });
    }

    /// 从汇编端克隆一个所有权。输入值不正确则会导致内存不安全。
    unsafe fn clone_asm(asm_value: Self::AsmType) -> Self::AsmType
    where
        Self: Clone,
    {
        let this = unsafe { Self::from_asm(asm_value) };
        forget_unsized(this.clone());
        unsafe { Self::into_asm(this) }
    }
}

unsafe impl<T: ?Sized> SharedWithAsm for Pin<Arc<T>> {
    type AsmType = *const T;

    unsafe fn into_asm(self) -> Self::AsmType {
        unsafe { Arc::into_raw(Pin::into_inner_unchecked(self)) }
    }

    unsafe fn from_asm(asm_value: Self::AsmType) -> Self {
        let arc = unsafe { Arc::from_raw(asm_value) };
        debug_assert!(Arc::strong_count(&arc) > 0);
        unsafe { Pin::new_unchecked(arc) }
    }
}

unsafe impl<T: ?Sized> SharedWithAsm for Pin<Box<T>> {
    type AsmType = *mut T;

    unsafe fn into_asm(self) -> Self::AsmType {
        Box::into_raw(unsafe { Pin::into_inner_unchecked(self) })
    }

    unsafe fn from_asm(asm_value: Self::AsmType) -> Self {
        unsafe { Pin::new_unchecked(Box::from_raw(asm_value)) }
    }
}
