use core::{
    cell::SyncUnsafeCell,
    hash::Hasher,
    marker::{PhantomData, PhantomPinned},
    mem::{offset_of, replace},
    ops::{Deref, DerefMut},
    pin::Pin,
    ptr::{DynMetadata, NonNull, addr_eq, dangling, metadata},
    sync::atomic::AtomicU32,
};

use alloc::{bstr::ByteString, sync::Arc};

use crate::{
    basic::sync::Mutex,
    filesystem::vfs::{
        DCacheKey, DEntryArc, File, FileFlags, FileSystem, INodeArc, VfsResult,
        dentry_arc::DCacheArc,
    },
    println,
};

/// DEntry 的多态部分
pub trait DEntryOps: Send + Sync {
    /// 打开当前 DEntry 对应的文件。
    fn open(&self, flags: FileFlags) -> VfsResult<Pin<Arc<File>>>;
}

#[derive(Clone, Copy)]
struct DEntryLinkedListPtr {
    ptr: NonNull<DEntryLinkedListNode>,
    metadata: DynMetadata<dyn DEntryOps>,
}

impl DEntryLinkedListPtr {
    pub fn from_dentry(dentry: NonNull<DEntry>) -> Self {
        let (ptr, metadata) = dentry.to_raw_parts();
        Self {
            ptr: unsafe { ptr.byte_add(offset_of!(DEntry, peer)) }.cast(),
            metadata,
        }
    }
    pub fn from_sentinel(sentinel: &DEntryData) -> Self {
        let (ptr, metadata) = NonNull::from_ref(sentinel).to_raw_parts();
        Self {
            ptr: unsafe { ptr.byte_add(offset_of!(DEntryData, children_sentinel)) }.cast(),
            metadata,
        }
    }

    pub fn dangling<T: DEntryOps + 'static>() -> Self {
        Self {
            ptr: NonNull::dangling(),
            metadata: metadata(dangling::<DEntry<T>>() as *const DEntry),
        }
    }

    pub fn as_dentry(&self) -> NonNull<DEntry> {
        let ptr = unsafe { self.ptr.byte_sub(offset_of!(DEntry, peer)) };
        NonNull::from_raw_parts(ptr, self.metadata)
    }
    pub fn as_node(&self) -> NonNull<DEntryLinkedListNode> {
        self.ptr
    }

    pub fn addr_eq(&self, other: &Self) -> bool {
        addr_eq(self.ptr.as_ptr(), other.ptr.as_ptr())
    }
}

pub struct DEntryLinkedListNode {
    pub next: DEntryLinkedListPtr,
    pub prev: DEntryLinkedListPtr,
    pub _phantom: PhantomPinned,
}

#[derive(Clone, Copy)]
pub struct DEntryLruPtr {
    ptr: NonNull<DEntryLruNode>,
    metadata: Option<DynMetadata<dyn DEntryOps>>,
}

impl DEntryLruPtr {
    pub fn from_dentry(dentry: NonNull<DEntry>) -> Self {
        let (ptr, metadata) = dentry.to_raw_parts();
        Self {
            ptr: unsafe { ptr.byte_add(offset_of!(DEntry, lru)) }.cast(),
            metadata: Some(metadata),
        }
    }

    pub fn from_sentinel(sentinel: &mut DEntryLruNode) -> Self {
        Self {
            ptr: sentinel.into(),
            metadata: None,
        }
    }

    pub fn dangling() -> Self {
        Self {
            ptr: NonNull::dangling(),
            metadata: None,
        }
    }

    pub fn as_dentry(&self) -> NonNull<DEntry> {
        debug_assert!(self.metadata.is_some());
        let ptr = unsafe { self.ptr.byte_sub(offset_of!(DEntry, lru)) };
        NonNull::from_raw_parts(ptr, unsafe { self.metadata.unwrap_unchecked() })
    }
    pub fn as_node(&self) -> NonNull<DEntryLruNode> {
        self.ptr
    }

    pub fn addr_eq(&self, other: &Self) -> bool {
        addr_eq(self.ptr.as_ptr(), other.ptr.as_ptr())
    }
}

pub struct DEntryLruNode {
    pub next: DEntryLruPtr,
    pub prev: DEntryLruPtr,
    pub _phantom: PhantomPinned,
}

pub struct DEntryData<T: ?Sized + DEntryOps = dyn DEntryOps> {
    pub parent: Option<DEntryArc>,
    pub name: ByteString,

    /// 子 DEntry 侵入式双向链表的哨兵节点
    pub children_sentinel: DEntryLinkedListNode,

    /// 关联的 INode，为 None 表示该 dentry 是负 dentry（标识文件不存在的 dentry）
    pub inode: Option<INodeArc>,

    pub dynamic_data: T,
}

/// 目录项，对应 Linux VFS 中的 dentry
///
/// 所有 DEntry 架构成一颗所有权倒置的树，父节点持有所有子节点的无所有权引用，子节点持有父节点的强引用
pub struct DEntry<T: ?Sized + DEntryOps = dyn DEntryOps> {
    /// 侵入式双向链表中，指向上一个/下一个兄弟节点的指针
    ///
    /// 读取或修改它前，必须获取父节点的锁（换言之，逻辑上它被父节点的 data 锁保护）。
    /// 若为根节点则此值无效。
    peer: SyncUnsafeCell<DEntryLinkedListNode>,

    /// 内存中的引用计数，其值为 0 时 DEntry 被链入 LRU 链表或回收（取决于 `in_dcache`）
    pub(super) mem_refcount: AtomicU32,
    /// 是否位于 dcache 中，若不位于则行为退化为普通引用计数，否则引用计数归零时其仅会被放入 LRU 链表
    ///
    /// 常量。这意味着它要么永远位于 dcache 中，要么永远不位于 dcache 中（例如根节点）。
    pub(super) in_dcache: bool,

    /// LRU 链表中，指向上一个/下一个节点的指针
    ///
    /// 和 `peer` 类似，它受 LRU 锁保护
    pub(super) lru: SyncUnsafeCell<DEntryLruNode>,

    /// 所属的文件系统，其保证有效且不会改变
    pub filesystem: NonNull<FileSystem>,

    /// 实际数据
    ///
    /// 这部分才是真正属于当前 DEntry 的，`peer` 逻辑上属于其父节点
    pub data: Mutex<DEntryData<T>>,
}

impl DEntry {
    pub unsafe fn attach_to_parent(self: &Pin<Arc<Self>>, parent: DEntryArc) {
        let my_peer = &self.peer;
        let my_ptr = DEntryLinkedListPtr::from_dentry(NonNull::from_ref(self.as_ref().get_ref()));
        let _parent_nonull = NonNull::from_ref(&parent);

        {
            let mut parent_mtx_guard = parent.data.lock();

            unsafe { Self::append_to_parent_list(my_peer, my_ptr, parent_mtx_guard.deref_mut()) };
        }

        let mut my_data_mtx_guard = self.data.lock();

        my_data_mtx_guard.parent = Some(parent);
    }
}

impl<T: ?Sized + DEntryOps> DEntry<T> {
    /// 将当前 DEntry 从父 DEntry 的子 DEntry 链表中移除，并解除父子关系
    ///
    /// # Panics
    /// 若当前节点为根节点则 Panic
    ///
    /// # Safety
    /// 此函数执行完毕后，除非再次将其附着到某个 DEntry 下，否则不能再通过它访问其父节点和兄弟节点
    pub unsafe fn remove_from_parent(&self) {
        let mut my_mtx_guard = self.data.lock();

        let parent = my_mtx_guard
            .parent
            .as_ref()
            .expect("当前节点必须不为根节点");

        // 锁定父节点
        let parent_mtx_guard = parent.data.lock();

        let my_prev;
        let my_next;

        {
            // SAFETY：已经锁定父节点，这意味着不可能有其它人能够写 self.peer
            let my_node = unsafe { self.peer.get().as_ref_unchecked() };

            my_prev = my_node.prev;
            my_next = my_node.next;
        }

        let mut prev_node = my_prev.as_node();
        let mut next_node = my_next.as_node();

        #[expect(dropping_references)]
        drop(self);

        // SAFETY：已经锁定父节点，这意味着不可能有其它人能够访问 self.peer；
        //         对于我们自己的别名，my_node 和 self 的确可能和它们别名，但它们都被 drop 了
        unsafe { prev_node.as_mut() }.next = my_next;
        unsafe { next_node.as_mut() }.prev = my_prev;

        drop(parent_mtx_guard);

        my_mtx_guard.parent = None;
    }

    pub fn get_fs(&self) -> &FileSystem {
        unsafe { self.filesystem.as_ref() }
    }
}

impl<T: ?Sized + DEntryOps> Drop for DEntry<T> {
    fn drop(&mut self) {
        unsafe { self.remove_from_parent() };
        println!(
            "DEntry {:p} ({}) dropped",
            self as *const Self,
            self.data.get_mut().name
        );
    }
}

impl DEntryData {
    /// 遍历所有子 DEntry（含负 dentry），返回迭代器
    pub fn children<'a>(&'a self) -> impl Iterator<Item = &'a DEntry> {
        struct Iter<'a> {
            current: DEntryLinkedListPtr,
            sentinel: DEntryLinkedListPtr,
            _phantom: PhantomData<&'a DEntry>,
        }

        impl<'a> Iterator for Iter<'a> {
            type Item = &'a DEntry;

            fn next(&mut self) -> Option<Self::Item> {
                if self.current.addr_eq(&self.sentinel) {
                    return None;
                }

                // SAFETY: 只要链表完好，就不会有问题
                let current_ref = unsafe { self.current.as_dentry().as_ref() };

                // SAFETY: 我们已经持有父节点的 Data 的引用，这隐含了获取锁
                self.current = unsafe { self.current.as_node().as_ref() }.next;

                Some(current_ref)
            }
        }

        Iter {
            current: self.children_sentinel.next,
            sentinel: DEntryLinkedListPtr::from_sentinel(self),
            _phantom: PhantomData,
        }
    }
}

impl<T: DEntryOps + 'static> DEntry<T> {
    #[must_use]
    pub fn lock_and_new(
        parent: DEntryArc,
        name: ByteString,
        inode: Option<INodeArc>,
        filesystem: &FileSystem,
        dynamic_data: T,
    ) -> DEntryArc {
        let dangling = DEntryLinkedListPtr::dangling::<T>();

        let ret = DEntryArc::new(Self {
            peer: SyncUnsafeCell::new(DEntryLinkedListNode {
                next: dangling,
                prev: dangling,
                _phantom: PhantomPinned,
            }),
            lru: SyncUnsafeCell::new(DEntryLruNode {
                next: DEntryLruPtr::dangling(),
                prev: DEntryLruPtr::dangling(),
                _phantom: PhantomPinned,
            }),
            mem_refcount: AtomicU32::new(1),
            in_dcache: true,
            filesystem: NonNull::from_ref(filesystem),
            data: Mutex::new(DEntryData {
                parent: Some(parent),
                name,
                inode: inode.clone(),
                children_sentinel: DEntryLinkedListNode {
                    next: dangling,
                    prev: dangling,
                    _phantom: PhantomPinned,
                },
                dynamic_data,
            }),
        });

        let mut ret_mtx_guard = ret.data.lock();

        {
            let ret_sent_ptr = DEntryLinkedListPtr::from_sentinel(ret_mtx_guard.deref());

            ret_mtx_guard.children_sentinel.next = ret_sent_ptr;
            ret_mtx_guard.children_sentinel.prev = ret_sent_ptr;
        }

        let ret_ptr = DEntryLinkedListPtr::from_dentry(NonNull::from_ref(&ret));

        let parent = ret_mtx_guard.parent.as_ref().unwrap();

        // 开始修改父节点的子 DEntry 链表
        let mut parent_mtx_guard = parent.data.lock();

        unsafe { Self::append_to_parent_list(&ret.peer, ret_ptr, parent_mtx_guard.deref_mut()) };

        drop(parent_mtx_guard); // 对侵入式链表的修改已完成，可以释放父节点的锁了

        let key = DCacheKey {
            parent: NonNull::from_ref(parent),
            name: ret_mtx_guard.name.clone(),
        };
        drop(ret_mtx_guard);

        // 现在才能将节点加入 dcache
        let mut dcache_guard = ret.get_fs().dcache.write();
        dcache_guard
            .entry(key)
            .and_modify(|negative| {
                let mut neg_data = negative.data.lock();
                debug_assert!(neg_data.inode.is_none());
                neg_data.inode = inode;
            })
            .or_insert_with(|| DCacheArc::from_new(&ret));
        drop(dcache_guard);

        ret
    }

    #[must_use]
    pub fn new_root(
        name: ByteString,
        inode: INodeArc,
        filesystem: NonNull<FileSystem>,
        dynamic_data: T,
    ) -> DEntryArc {
        let dangling = DEntryLinkedListPtr::dangling::<T>();

        let ret = DEntryArc::new(Self {
            peer: SyncUnsafeCell::new(DEntryLinkedListNode {
                next: dangling,
                prev: dangling,
                _phantom: PhantomPinned,
            }),
            lru: SyncUnsafeCell::new(DEntryLruNode {
                next: DEntryLruPtr::dangling(),
                prev: DEntryLruPtr::dangling(),
                _phantom: PhantomPinned,
            }),
            mem_refcount: AtomicU32::new(1),
            in_dcache: false, // 根节点不在 dcache 中
            filesystem,
            data: Mutex::new(DEntryData {
                parent: None,
                name,
                inode: Some(inode),
                children_sentinel: DEntryLinkedListNode {
                    next: dangling,
                    prev: dangling,
                    _phantom: PhantomPinned,
                },
                dynamic_data,
            }),
        });

        {
            let mut ret_mtx_guard = ret.data.lock();

            let ret_sent_ptr = DEntryLinkedListPtr::from_sentinel(ret_mtx_guard.deref());
            let sent = &mut ret_mtx_guard.children_sentinel;
            sent.prev = ret_sent_ptr;
            sent.next = ret_sent_ptr;
        }

        // 根节点不在 dcache 中

        ret
    }
}

impl<T: ?Sized + DEntryOps> DEntry<T> {
    unsafe fn append_to_parent_list(
        my_peer: &SyncUnsafeCell<DEntryLinkedListNode>,
        my_ptr: DEntryLinkedListPtr,
        parent: &mut DEntryData,
    ) {
        let parent_ptr = DEntryLinkedListPtr::from_sentinel(&parent);

        // 将父节点的 prev 指针指向自己，并保存原先的 prev 指针
        let parent_prev = replace(&mut parent.children_sentinel.prev, my_ptr);

        // 注意，此处 parent_prev.next 可能也指向父节点，因此不能形成引用，只能直接通过裸指针读写
        let parent_prev_node = parent_prev.as_node();
        unsafe {
            parent_prev_node
                .byte_add(offset_of!(DEntryLinkedListNode, next))
                .cast::<DEntryLinkedListPtr>()
                .write(my_ptr);
        }

        let my_peer = my_peer.get();

        // 注意，由于先修改了前后节点的指针，这两个修改也必须位于临界区内
        unsafe {
            my_peer
                .byte_add(offset_of!(DEntryLinkedListNode, prev))
                .cast::<DEntryLinkedListPtr>()
                .write(parent_prev);
            my_peer
                .byte_add(offset_of!(DEntryLinkedListNode, next))
                .cast::<DEntryLinkedListPtr>()
                .write(parent_ptr);
        }
    }
}

impl DEntry {
    pub unsafe fn lru_drop(&self, _sentinel: &DEntryLruNode) {
        let lru = unsafe { self.lru.get().as_ref_unchecked() };

        let my_prev = lru.prev;
        let my_next = lru.next;

        // SAFETY: 和上面不同，它们不可能是当前 LRU 节点的别名
        unsafe { my_prev.as_node().as_mut() }.next = my_next;
        unsafe { my_next.as_node().as_mut() }.prev = my_prev;
    }
    pub unsafe fn lru_add(&self, sentinel: &mut DEntryLruNode) {
        let lru = unsafe { self.lru.get().as_mut_unchecked() };

        lru.prev = DEntryLruPtr::from_sentinel(sentinel);
        lru.next = sentinel.next;

        let sent_next = replace(
            &mut sentinel.next,
            DEntryLruPtr::from_dentry(NonNull::from_ref(self)),
        );

        drop(sentinel);

        // SAFETY: 它可能和 sentinel 别名，但 sentinel 已经 dropped 了
        unsafe { sent_next.as_node().as_mut() }.prev =
            DEntryLruPtr::from_dentry(NonNull::from_ref(self));
    }
}

impl<T: DEntryOps> DEntryData<T> {
    pub fn get_data(dynamic: &T) -> &DEntryData<T> {
        let dynamic = NonNull::from_ref(dynamic);
        unsafe {
            dynamic
                .byte_sub(offset_of!(DEntryData<T>, dynamic_data))
                .cast()
                .as_ref()
        }
    }
}

impl<T: DEntryOps> DEntry<T> {
    pub fn get_dentry(dynamic: &T) -> &DEntry<T> {
        unsafe {
            Mutex::from_value(NonNull::from_ref(DEntryData::get_data(dynamic)))
                .byte_sub(offset_of!(DEntry<T>, data))
                .cast()
                .as_ref()
        }
    }
}
