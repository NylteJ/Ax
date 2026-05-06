use core::{
    bstr::ByteStr,
    hash::Hash,
    num::NonZero,
    pin::Pin,
    ptr::{NonNull, addr_eq},
};

use alloc::{bstr::ByteString, sync::Arc};
use hashbrown::{Equivalent, HashMap};

use crate::{
    basic::sync::{Mutex, RwLock},
    filesystem::vfs::{
        DEntry, DEntryArc, DEntryLruNode, DEntryLruPtr, INodeArc, VfsError, VfsResult,
        dentry_arc::DCacheArc,
    },
};

/// 描述整个文件系统，对应 Linux 中的超级块（super block）
pub struct FileSystem<T: ?Sized + FileSystemOps = dyn FileSystemOps> {
    pub(super) dcache: RwLock<HashMap<DCacheKey, DCacheArc>>,

    pub root_inode: INodeArc,
    pub init_root_dentry: DEntryArc,

    pub lru_sentinel: Mutex<DEntryLruNode>,

    _dummy: (),

    pub dynamic_data: T,
}

impl<T: FileSystemOps + 'static> FileSystem<T> {
    pub fn new(
        roots: fn(NonNull<FileSystem>) -> (INodeArc, DEntryArc),
        dynamic_data: T,
    ) -> Pin<Arc<FileSystem>> {
        let mut arc = Arc::new_cyclic(|weak| {
            let addr = NonNull::new(weak.as_ptr() as *mut FileSystem<T>).unwrap();
            let (root_inode, init_root_dentry) = roots(addr);
            FileSystem {
                dcache: RwLock::new(HashMap::new()),
                root_inode,
                init_root_dentry,
                lru_sentinel: Mutex::new(DEntryLruNode {
                    prev: DEntryLruPtr::dangling(),
                    next: DEntryLruPtr::dangling(),
                    _phantom: core::marker::PhantomPinned,
                }),
                _dummy: (),
                dynamic_data,
            }
        });

        let lru = Arc::get_mut(&mut arc).unwrap().lru_sentinel.get_mut();
        lru.prev = DEntryLruPtr::from_sentinel(lru);
        lru.next = lru.prev;

        unsafe { Pin::new_unchecked(arc) }
    }
}

impl FileSystem {
    pub fn find_dentry<'a>(&self, parent: NonNull<DEntry>, name: &ByteStr) -> Option<DEntryArc> {
        let key = DCacheKeyRef { parent, name };
        let dcache_read_guard = self.dcache.read();
        let cache = dcache_read_guard.get(&key)?;
        Some(DEntryArc::from_cache(cache)) // from_cache 内部会修改 LRU 链表
    }

    /// 查找路径对应的 DEntry。保证不会查到负 DEntry。
    pub fn walk_path(&self, path: &ByteStr, from: Option<DEntryArc>) -> VfsResult<DEntryArc> {
        if path.is_empty() {
            return Err(VfsError::FileNotFound);
        }

        let (mut current_dentry, skip) = if path.first() == Some(&b'/') {
            // 绝对路径
            (self.init_root_dentry.clone(), 1)
        } else if let Some(from) = from {
            // 相对路径
            (from, 0)
        } else {
            // 相对路径但没有指定起点，默认为 path 写错了
            return Err(VfsError::FileNotFound);
        };

        for name in path.split(|&b| b == b'/').skip(skip) {
            let name = ByteStr::new(name);

            if name == "." {
                continue;
            } else if name == ".." {
                let tmp = current_dentry
                    .data
                    .lock()
                    .parent
                    .clone()
                    .ok_or(VfsError::FileNotFound)?;
                current_dentry = tmp;
                continue;
            }

            let cached = self.find_dentry(NonNull::from_ref(&current_dentry), name);

            if let Some(cached) = cached {
                current_dentry = cached;
            } else {
                let current_inode = current_dentry
                    .data
                    .lock()
                    .inode
                    .as_ref()
                    .ok_or(VfsError::FileNotFound)?
                    .clone();
                let dentry = current_inode
                    .data
                    .read()
                    .dynamic_data
                    .lookup(name, current_dentry)?;
                current_dentry = dentry;
            }

            if current_dentry.data.lock().inode.is_none() {
                return Err(VfsError::FileNotFound);
            }
        }

        debug_assert!(current_dentry.data.lock().inode.is_some());
        Ok(current_dentry)
    }

    pub fn gc(&self, max_count: NonZero<usize>) {
        let mut dcache_guard = self.dcache.write();
        let mut lru_guard = self.lru_sentinel.lock();

        let sent = DEntryLruPtr::from_sentinel(&mut lru_guard);
        let mut current_node = lru_guard.prev;
        for _ in 0..max_count.get() {
            if current_node.addr_eq(&sent) {
                break;
            }

            let prev_node = unsafe { current_node.as_node().as_ref().prev };
            let node = unsafe { current_node.as_dentry().as_ref() };

            // TODO: 如果改成侵入式链表这里能免去查找开销
            let node_data_guard = node.data.lock();
            let old_dcache = dcache_guard
                .remove(&DCacheKeyRef {
                    parent: node_data_guard.parent.as_ref().expect("无父节点的 DEntry 不应位于 DCache 中，因此也不应位于 LRU 链表中（它应当和普通引用计数一样立即被删除）").ptr(),
                    name: node_data_guard.name.as_ref(),
                })
                .expect("LRU 链表 中的每一项应当都在 DCache 中");
            drop(node_data_guard); // 防止死锁
            drop(old_dcache);

            current_node = prev_node;
        }

        // 现在，current_node 是第一个未被回收的节点，我们需要将它和 sent 连接起来
        // 注意尽管 current_node 可能就是 sent，lru_guard 并不持有真正意义上的引用，因此不违反别名规则
        lru_guard.prev = current_node;
        unsafe { current_node.as_node().as_mut() }.next = sent;
    }

    pub fn full_gc(&self) {
        self.gc(NonZero::<usize>::MAX);
    }
}

pub trait FileSystemOps {}

#[derive(Debug)]
pub struct DCacheKey {
    pub parent: NonNull<DEntry>,
    pub name: ByteString,
}

impl Hash for DCacheKey {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.parent.to_raw_parts().0.hash(state);
        ByteStr::new(&self.name).hash(state);
    }
}

impl PartialEq for DCacheKey {
    fn eq(&self, other: &Self) -> bool {
        addr_eq(self.parent.as_ptr(), other.parent.as_ptr()) && self.name == other.name
    }
}

impl Eq for DCacheKey {}

pub struct DCacheKeyRef<'a> {
    pub parent: NonNull<DEntry>,
    pub name: &'a ByteStr,
}

impl Hash for DCacheKeyRef<'_> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.parent.to_raw_parts().0.hash(state);
        self.name.hash(state);
    }
}

impl Equivalent<DCacheKey> for DCacheKeyRef<'_> {
    fn equivalent(&self, key: &DCacheKey) -> bool {
        addr_eq(self.parent.as_ptr(), key.parent.as_ptr()) && self.name == key.name
    }
}
