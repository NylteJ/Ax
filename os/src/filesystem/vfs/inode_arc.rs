use core::{
    any::Any,
    fmt::Debug,
    marker::Unsize,
    ops::{CoerceUnsized, Deref},
    ptr::NonNull,
    sync::atomic::{self, Ordering},
};

use alloc::boxed::Box;

use crate::filesystem::vfs::{INode, INodeOps, VfsError, VfsResult};

// #[derive(Debug)]
pub struct INodeArc<Ops: ?Sized + INodeOps = dyn INodeOps> {
    ptr: NonNull<INode<Ops>>,
}

impl INodeArc {
    pub fn new<T: INodeOps>(inode: INode<T>) -> Self {
        debug_assert_eq!(inode.mem_refcount.load(Ordering::Relaxed), 1);
        Self {
            ptr: NonNull::from_ref(Box::leak(Box::new(inode))),
        }
    }

    pub fn downcast<T: INodeOps>(self) -> Result<INodeArc<T>, Self> {
        if (&self.data.read().dynamic_data as &dyn Any).is::<T>() {
            Ok(INodeArc::<T> {
                ptr: self.ptr.cast(),
            })
        } else {
            Err(self)
        }
    }

    pub fn downcast_or_vfserr<T: INodeOps>(self) -> VfsResult<INodeArc<T>> {
        self.downcast::<T>()
            .map_err(|_| VfsError::FilesystemMismatch)
    }

    pub fn downcast_ref<T: INodeOps>(&self) -> Option<&INode<T>> {
        if (&self.data.read().dynamic_data as &dyn Any).is::<T>() {
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

impl<Ops: ?Sized + INodeOps> Debug for INodeArc<Ops> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("INodeArc").field("ptr", &self.ptr).finish()
    }
}

impl<T: ?Sized + INodeOps + Unsize<U>, U: ?Sized + INodeOps> CoerceUnsized<INodeArc<U>>
    for INodeArc<T>
{
}

impl<Ops: ?Sized + INodeOps> Clone for INodeArc<Ops> {
    fn clone(&self) -> Self {
        self.mem_refcount.fetch_add(1, Ordering::Relaxed);
        Self { ptr: self.ptr }
    }
}

impl<Ops: ?Sized + INodeOps> Drop for INodeArc<Ops> {
    fn drop(&mut self) {
        if self.mem_refcount.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }

        atomic::fence(Ordering::Acquire);

        let my_data = unsafe { self.ptr.as_mut() }.data.get_mut();

        if my_data.dynamic_data.should_drop_when_mem_refcount_zero() {
            drop(unsafe { Box::from_raw(self.ptr.as_ptr()) });
        } else {
            // todo!();
            // FIXME
            crate::println!("[TODO] INodeArc {:p} leaked", self.ptr.as_ptr());
        }
    }
}

impl<Ops: ?Sized + INodeOps> Deref for INodeArc<Ops> {
    type Target = INode<Ops>;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}
