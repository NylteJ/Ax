use core::{
    mem::ManuallyDrop,
    ops::{Deref, DerefPure},
    ptr::NonNull,
    sync::atomic::Ordering,
};

use alloc::{alloc::AllocError, boxed::Box};
use intrusive_collections::{
    Adapter, DefaultLinkOps, LinkOps, LinkedListLink, PointerOps, container_of, offset_of,
};

use crate::{
    basic::shared_with_asm::SharedWithAsm,
    multitask::{task::Task, task_manager::unregister_task},
};

#[derive(Debug)]
pub struct TaskArc {
    ptr: NonNull<Task>,
}
unsafe impl Send for TaskArc {}
unsafe impl Sync for TaskArc {}

impl Deref for TaskArc {
    type Target = Task;

    fn deref(&self) -> &Self::Target {
        let task = unsafe { self.ptr.as_ref() };
        debug_assert!(task.refcount.load(Ordering::Relaxed) > 0);
        task
    }
}
#[cfg(not(debug_assertions))]
unsafe impl DerefPure for TaskArc {}

unsafe impl SharedWithAsm for TaskArc {
    type AsmType = *const Task;

    unsafe fn into_asm(self) -> Self::AsmType {
        let this = ManuallyDrop::new(self);
        this.ptr.as_ptr()
    }

    unsafe fn from_asm(asm_value: Self::AsmType) -> Self {
        Self {
            ptr: unsafe { NonNull::new_unchecked(asm_value as *mut Task) },
        }
    }
}

impl Clone for TaskArc {
    fn clone(&self) -> Self {
        let old_ref = self.refcount.fetch_add(1, Ordering::Relaxed);
        debug_assert!(old_ref > 0);
        Self { ptr: self.ptr }
    }
}

impl Drop for TaskArc {
    fn drop(&mut self) {
        let task: &Task = self; // 防止与 debug 检查冲突

        let old_ref = task.refcount.fetch_sub(1, Ordering::Release);
        debug_assert!(old_ref > 0);

        if old_ref != 1 {
            return;
        }

        let need_drop = unregister_task(|| task.refcount.load(Ordering::Acquire) == 0, task.tid);

        if need_drop {
            drop(unsafe { Box::from_raw(self.ptr.as_ptr()) });
        }
    }
}

#[derive(Clone, Copy)]
pub struct TaskArcPointerOps;
unsafe impl PointerOps for TaskArcPointerOps {
    type Value = Task;
    type Pointer = TaskArc;

    unsafe fn from_raw(&self, value: *const Self::Value) -> Self::Pointer {
        unsafe { TaskArc::from_asm(value) }
    }
    fn into_raw(&self, ptr: Self::Pointer) -> *const Self::Value {
        unsafe { ptr.into_asm() }
    }
}

#[derive(Clone, Copy)]
pub struct TaskAdapter {
    link_ops: <LinkedListLink as DefaultLinkOps>::Ops,
    pointer_ops: TaskArcPointerOps,
}
unsafe impl Send for TaskAdapter {}
unsafe impl Sync for TaskAdapter {}

impl TaskAdapter {
    pub const NEW: Self = TaskAdapter {
        link_ops: <LinkedListLink as DefaultLinkOps>::NEW,
        pointer_ops: TaskArcPointerOps,
    };
    #[inline]
    pub fn new() -> Self {
        Self::NEW
    }
}

unsafe impl Adapter for TaskAdapter {
    type LinkOps = <LinkedListLink as DefaultLinkOps>::Ops;
    type PointerOps = TaskArcPointerOps;

    #[inline]
    unsafe fn get_value(
        &self,
        link: <Self::LinkOps as LinkOps>::LinkPtr,
    ) -> *const <Self::PointerOps as PointerOps>::Value {
        container_of!(link.as_ptr(), Task, children_link)
    }
    #[inline]
    unsafe fn get_link(
        &self,
        value: *const <Self::PointerOps as PointerOps>::Value,
    ) -> <Self::LinkOps as LinkOps>::LinkPtr {
        // We need to do this instead of just accessing the field directly
        // to strictly follow the stack borrow rules.
        let ptr = unsafe { (value as *const u8).add(offset_of!(Task, children_link)) };
        unsafe { core::ptr::NonNull::new_unchecked(ptr as *mut _) }
    }
    #[inline]
    fn link_ops(&self) -> &Self::LinkOps {
        &self.link_ops
    }
    #[inline]
    fn link_ops_mut(&mut self) -> &mut Self::LinkOps {
        &mut self.link_ops
    }
    #[inline]
    fn pointer_ops(&self) -> &Self::PointerOps {
        &self.pointer_ops
    }
}

impl TaskArc {
    pub fn try_new(mut task: Task) -> Result<Self, AllocError> {
        debug_assert!(*task.refcount.get_mut() == 1);
        let boxed = Box::try_new(task)?;
        let ptr = Box::into_raw(boxed);
        Ok(Self {
            ptr: unsafe { NonNull::new_unchecked(ptr) },
        })
    }
}
