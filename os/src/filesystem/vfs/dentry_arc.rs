use core::{ops::Deref, ptr::NonNull, sync::atomic::Ordering};

use alloc::boxed::Box;

use crate::filesystem::vfs::{
    DEntry, DEntryOps,
    cache::{CachedValue, CachedValueArc, drop_cached_varc},
};

#[derive(Debug)]
pub struct DEntryArc {
    pub(self) ptr: NonNull<DEntry>,
}

impl DEntryArc {
    pub fn new<T: DEntryOps + 'static>(val: DEntry<T>) -> Self {
        debug_assert_eq!(val.mem_refcount.load(Ordering::Relaxed), 1);
        Self {
            ptr: NonNull::from_ref(Box::leak(Box::new(val))),
        }
    }
}

impl DEntryArc {
    pub fn addr_eq(&self, other: &Self) -> bool {
        self.ptr.addr() == other.ptr.addr()
    }

    pub fn addr(&self) -> usize {
        self.ptr.addr().into()
    }

    pub fn ptr(&self) -> NonNull<DEntry> {
        self.ptr
    }
}

impl Clone for DEntryArc {
    fn clone(&self) -> Self {
        self.mem_refcount.fetch_add(1, Ordering::Relaxed);
        Self { ptr: self.ptr }
    }
}

impl Drop for DEntryArc {
    fn drop(&mut self) {
        unsafe { drop_cached_varc(self, |_| true) };
    }
}

impl Deref for DEntryArc {
    type Target = DEntry;

    fn deref(&self) -> &<Self as Deref>::Target {
        unsafe { self.ptr.as_ref() }
    }
}

unsafe impl Send for DEntryArc {}
unsafe impl Sync for DEntryArc {}

impl CachedValueArc for DEntryArc {
    unsafe fn from_exists_unchecked(cache: &Self::Target) -> Self {
        Self {
            ptr: NonNull::from_ref(cache),
        }
    }

    unsafe fn drop_inner_unchecked(&self) {
        drop(unsafe { Box::from_raw(self.ptr.as_ptr()) });
    }
}

/// 本质上存储的还是 `DEntry`，但用于 dcache 中
#[derive(Debug)]
pub struct DCacheArc {
    ptr: NonNull<DEntry<dyn DEntryOps>>,
}

impl DCacheArc {
    pub fn from_new(dentry: &DEntryArc) -> Self {
        Self { ptr: dentry.ptr }
    }

    pub unsafe fn drop_manually(dentry: &DEntry) {
        drop(Self {
            ptr: NonNull::from_ref(dentry),
        });
    }
}

impl Deref for DCacheArc {
    type Target = DEntry;

    fn deref(&self) -> &<Self as Deref>::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl Drop for DCacheArc {
    fn drop(&mut self) {
        // 其它地方负责约束 DCacheArc 一定在引用计数归零时由回收进程在持有 DCache 写锁 + LRU 锁的情况下析构
        // 因此这里直接清理即可，而且不需要动 LRU

        debug_assert!(unsafe { *self.in_dcache.get() });

        let barrier = self.mem_refcount.load(Ordering::Acquire);
        debug_assert_eq!(barrier, 0);

        drop(unsafe { Box::from_raw(self.ptr.as_ptr()) });
    }
}

unsafe impl Send for DCacheArc {}
unsafe impl Sync for DCacheArc {}
