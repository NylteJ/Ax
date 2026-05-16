use core::{any::Any, fmt::Debug, ops::Deref, ptr::NonNull, sync::atomic::Ordering};

use alloc::boxed::Box;

use crate::filesystem::vfs::{
    INode, INodeDirtyState, INodeOps, VfsError, VfsResult,
    cache::{CachedValueArc, drop_cached_varc},
};

// #[derive(Debug)]
pub struct INodeArc {
    ptr: NonNull<INode>,
}

impl INodeArc {
    pub fn new<T: INodeOps>(inode: INode<T>) -> Self {
        debug_assert_eq!(inode.mem_refcount.load(Ordering::Relaxed), 1);
        Self {
            ptr: NonNull::from_ref(Box::leak(Box::new(inode))),
        }
    }

    pub fn downcast_ref<T: INodeOps>(&self) -> Option<&INode<T>> {
        if (&self.data.read().ops as &dyn Any).is::<T>() {
            Some(unsafe { self.ptr.cast().as_ref() })
        } else {
            None
        }
    }

    pub fn downcast_ref_or_vfserr<T: INodeOps>(&self) -> VfsResult<&INode<T>> {
        self.downcast_ref::<T>().ok_or(VfsError::FilesystemMismatch)
    }

    pub fn addr_eq(&self, other: &Self) -> bool {
        self.ptr.addr() == other.ptr.addr()
    }

    pub fn addr(&self) -> usize {
        self.ptr.addr().into()
    }

    /// # Safety
    /// `INodeArc` 在设计之初就隐含 `Pin` 的语义，加上此函数并不检查自己是否是对象的唯一引用，此调用非常不安全并且应当尽可能少地使用
    pub unsafe fn get_mut(&mut self) -> &mut INode {
        debug_assert_eq!(self.mem_refcount.load(Ordering::Relaxed), 1);
        unsafe { self.ptr.as_mut() }
    }
}

impl Debug for INodeArc {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("INodeArc").field("ptr", &self.ptr).finish()
    }
}

impl Clone for INodeArc {
    fn clone(&self) -> Self {
        self.mem_refcount.fetch_add(1, Ordering::Relaxed);
        Self { ptr: self.ptr }
    }
}

impl Drop for INodeArc {
    fn drop(&mut self) {
        unsafe {
            drop_cached_varc(self, |inode| {
                INodeDirtyState::from_bits_truncate(inode.dirty_state.load(Ordering::Relaxed))
                    .is_pure_clean()
                    && inode.data.write().ops.evictable_when_mem_ref_zero()
            })
        };
    }
}

impl Deref for INodeArc {
    type Target = INode;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

unsafe impl Send for INodeArc {}
unsafe impl Sync for INodeArc {}

impl CachedValueArc for INodeArc {
    unsafe fn from_exists_unchecked(cache: &Self::Target) -> Self {
        Self {
            ptr: NonNull::from_ref(cache),
        }
    }

    unsafe fn drop_inner_unchecked(&self) {
        drop(unsafe { Box::from_raw(self.ptr.as_ptr()) });
    }
}

#[derive(Debug)]
pub struct ICacheArc {
    ptr: NonNull<INode>,
}

impl ICacheArc {
    pub fn from_new(inode: &INodeArc) -> Self {
        Self { ptr: inode.ptr }
    }

    pub unsafe fn drop_manually(inode: &INode) {
        drop(Self {
            ptr: NonNull::from_ref(inode),
        });
    }
}

impl Deref for ICacheArc {
    type Target = INode;

    fn deref(&self) -> &<Self as Deref>::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl Drop for ICacheArc {
    fn drop(&mut self) {
        // 其它地方负责约束 ICacheArc 一定在引用计数归零时由回收进程在持有 ICache 写锁 + LRU 锁的情况下析构
        // 因此这里直接清理即可，而且不需要动 LRU

        debug_assert!(unsafe { *self.in_icache.get() });

        let barrier = self.mem_refcount.load(Ordering::Acquire);
        debug_assert_eq!(barrier, 0);

        drop(unsafe { Box::from_raw(self.ptr.as_ptr()) });
    }
}

unsafe impl Send for ICacheArc {}
unsafe impl Sync for ICacheArc {}
