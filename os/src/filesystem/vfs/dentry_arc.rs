use core::{
    ops::Deref,
    ptr::NonNull,
    sync::atomic::{self, Ordering},
};

use alloc::boxed::Box;

use crate::{
    filesystem::vfs::{DEntry, DEntryOps},
    println,
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

    pub fn from_cache(dcache_arc: &DCacheArc) -> Self {
        debug_assert!(dcache_arc.in_dcache);
        let old_ref = dcache_arc.mem_refcount.fetch_add(1, Ordering::Relaxed);

        if old_ref == 0 {
            unsafe { dcache_arc.lru_drop(&dcache_arc.get_fs().lru_sentinel.lock()) };
        }

        Self {
            ptr: dcache_arc.ptr,
        }
    }

    pub unsafe fn from_exists(dentry: &DEntry) -> Self {
        let old = dentry.mem_refcount.fetch_add(1, Ordering::Relaxed);
        debug_assert_ne!(old, 0);
        Self {
            ptr: NonNull::from_ref(dentry),
        }
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
        if !self.in_dcache {
            // 不在 dcache 中，按照普通 Arc 处理即可
            if self.mem_refcount.fetch_sub(1, Ordering::Release) == 1 {
                atomic::fence(Ordering::Acquire);
                drop(unsafe { Box::from_raw(self.ptr.as_ptr()) });
            }

            return;
        }

        let Err(_) =
            self.mem_refcount
                .try_update(Ordering::Release, Ordering::Relaxed, |ref_count| {
                    debug_assert!(ref_count > 0);
                    if ref_count == 1 {
                        None
                    } else {
                        Some(ref_count - 1)
                    }
                })
        else {
            return;
        };

        // 慢速路径，此时可能已经没有其它引用了。注意此时我们没有减少 self 对应的计数
        // 此时必须先获取 dcache 的写锁

        let _dcache_write_guard = self.get_fs().dcache.write();

        if self.mem_refcount.fetch_sub(1, Ordering::Release) == 1 {
            // 现在没人能从 dcache 中读缓存，因此，如果此时只有 self 一个引用，就不可能再产生其它引用
            let mut lru_guard = self.get_fs().lru_sentinel.lock();
            unsafe { self.lru_add(&mut lru_guard) };
            println!(
                "DEntry {:p} ({}) added to LRU list",
                self.ptr.as_ptr(),
                self.data.lock().name
            );
        }
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

/// 本质上存储的还是 `DEntry`，但用于 dcache 中
#[derive(Debug)]
pub struct DCacheArc {
    ptr: NonNull<DEntry<dyn DEntryOps>>,
}

impl DCacheArc {
    pub fn from_new(dentry: &DEntryArc) -> Self {
        debug_assert!(dentry.in_dcache);
        Self { ptr: dentry.ptr }
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

        debug_assert!(self.in_dcache);

        let barrier = self.mem_refcount.load(Ordering::Acquire);
        debug_assert_eq!(barrier, 0);

        drop(unsafe { Box::from_raw(self.ptr.as_ptr()) });
    }
}

unsafe impl Send for DCacheArc {}
unsafe impl Sync for DCacheArc {}
