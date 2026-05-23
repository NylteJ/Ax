use core::{
    cell::SyncUnsafeCell,
    marker::PhantomPinned,
    mem::replace,
    ptr::{NonNull, Pointee, addr_eq},
};

pub struct LruPtr<T: ?Sized> {
    ptr: NonNull<LruNodeInner<T>>,
    metadata: Option<<T as Pointee>::Metadata>,
}

impl<T: ?Sized> Clone for LruPtr<T> {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr.clone(),
            metadata: self.metadata.clone(),
        }
    }
}
impl<T: ?Sized> Copy for LruPtr<T> {}

unsafe impl<T: ?Sized> Send for LruPtr<T> {}
unsafe impl<T: ?Sized> Sync for LruPtr<T> {}

pub struct LruNodeInner<T: ?Sized> {
    pub next: LruPtr<T>,
    pub prev: LruPtr<T>,
    pub _phantom: PhantomPinned,
}

#[repr(transparent)]
pub struct LruNode<T: ?Sized> {
    pub lru: SyncUnsafeCell<LruNodeInner<T>>, // FIXME: 也许需要 UnsafePinned
}

impl<T: ?Sized> LruPtr<T> {
    pub fn from_value(dentry: NonNull<T>, offset: usize) -> Self {
        let (ptr, metadata) = dentry.to_raw_parts();
        Self {
            ptr: unsafe { ptr.byte_add(offset) }.cast(),
            metadata: Some(metadata),
        }
    }

    pub fn from_sentinel(sentinel: &mut LruNode<T>) -> Self {
        Self {
            ptr: sentinel.lru.get_mut().into(),
            metadata: None,
        }
    }

    pub fn dangling() -> Self {
        Self {
            ptr: NonNull::dangling(),
            metadata: None,
        }
    }

    pub fn as_value(&self, offset: usize) -> NonNull<T> {
        debug_assert!(self.metadata.is_some());
        let ptr = unsafe { self.ptr.byte_sub(offset) };
        NonNull::from_raw_parts(ptr, unsafe { self.metadata.unwrap_unchecked() })
    }
    pub fn as_node(&self) -> NonNull<LruNodeInner<T>> {
        self.ptr
    }

    pub fn addr_eq(&self, other: &Self) -> bool {
        addr_eq(self.ptr.as_ptr(), other.ptr.as_ptr())
    }
}

impl<T: ?Sized> LruNode<T> {
    pub fn new_unlinked() -> Self {
        Self {
            lru: SyncUnsafeCell::new(LruNodeInner {
                next: LruPtr::dangling(),
                prev: LruPtr::dangling(),
                _phantom: PhantomPinned,
            }),
        }
    }

    /// # Safety
    /// 该节点必须不位于任何 LRU 链表中，且此后不会再被移动。
    pub unsafe fn to_sentinel(&mut self) {
        let ptr = LruPtr::from_sentinel(self);
        let lru = self.lru.get_mut();
        lru.prev = ptr;
        lru.next = ptr;
    }

    /// 将自己从 LRU 链表中移除。
    ///
    /// # Safety
    /// 该节点目前必须位于 LRU 链表中，且调用者必须已持有该链表哨兵节点的独占锁。
    ///
    /// 值得强调的是，这隐含 self 不能是哨兵节点。
    pub unsafe fn lru_drop(&self, sentinel: &mut Self) {
        let lru = unsafe { self.lru.get().as_ref_unchecked() };

        let my_prev = lru.prev;
        let my_next = lru.next;

        #[expect(dropping_references)]
        drop(sentinel);

        debug_assert!(!my_prev.addr_eq(&LruPtr::dangling()));
        debug_assert!(!my_next.addr_eq(&LruPtr::dangling()));

        // SAFETY: 它们不可能是当前 LRU 节点的别名（因为总会有哨兵）
        unsafe { my_prev.as_node().as_mut() }.next = my_next;
        unsafe { my_next.as_node().as_mut() }.prev = my_prev;

        if cfg!(debug_assertions) {
            let lru = unsafe { self.lru.get().as_mut_unchecked() };
            lru.prev = LruPtr::dangling();
            lru.next = LruPtr::dangling();
        }
    }

    /// 将给定值的对应节点添加到 LRU 链表中，作为哨兵节点的下一个节点。
    ///
    /// # Safety
    /// 给定节点目前必须不位于任何 LRU 链表中。此节点必须是哨兵节点。偏移量必须正确。
    pub unsafe fn lru_add(&mut self, value: &T, offset: usize) {
        let self_ptr = LruPtr::from_sentinel(self);
        let sentinel = self.lru.get_mut();
        let lru = unsafe {
            NonNull::from_ref(value)
                .byte_add(offset)
                .cast::<Self>()
                .as_ref()
                .lru
                .get()
                .as_mut_unchecked()
        };

        // debug_assert!(lru.prev.addr_eq(&LruPtr::dangling()));
        // debug_assert!(lru.next.addr_eq(&LruPtr::dangling()));

        lru.prev = self_ptr;
        lru.next = sentinel.next;

        let value_ptr = LruPtr::from_value(NonNull::from_ref(value), offset);

        let sent_next = replace(&mut sentinel.next, value_ptr);

        // SAFETY: 它可能和 sentinel 别名，但我们之后就没有使用过 sentinel 了，所以不会有问题
        unsafe { sent_next.as_node().as_mut() }.prev = value_ptr;
    }
}
