use core::{
    bstr::ByteStr,
    cell::SyncUnsafeCell,
    hash::Hash,
    mem::MaybeUninit,
    num::NonZero,
    ops::Deref,
    pin::Pin,
    ptr::{NonNull, addr_eq},
};

use alloc::{bstr::ByteString, sync::Arc};
use derive_more::Debug;
use hashbrown::{Equivalent, HashMap};

use crate::{
    basic::sync::{Mutex, RwLock},
    filesystem::vfs::{
        DEntry, DEntryArc, DEntryLruNode, DEntryLruPtr, INodeArc, Mount, VfsError, VfsResult,
        dentry_arc::DCacheArc,
    },
};

/// 描述整个文件系统，对应 Linux 中的超级块（super block）
#[derive(Debug)]
pub struct FileSystem<T: ?Sized + FileSystemOps = dyn FileSystemOps> {
    pub(super) dcache: RwLock<HashMap<DCacheKey, DCacheArc>>,

    pub root_inode: INodeArc,
    pub init_root_dentry: DEntryArc,

    #[debug(skip)]
    pub lru_sentinel: Mutex<DEntryLruNode>,

    #[debug(skip)]
    _dummy: (),

    #[debug(skip)]
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

#[derive(Debug, Clone)]
pub struct Path(pub DEntryArc, pub Arc<Mount>);

impl Path {
    pub fn walk_step_back(&self) -> Self {
        self.walk_step_back_until_root()
            .unwrap_or_else(|| self.clone())
    }

    pub fn walk_step_back_until_root(&self) -> Option<Self> {
        let parent_fs_path;

        let path = if self.0.addr_eq(&self.1.mount_to) {
            parent_fs_path = self.1.mount_point.get_parent(); // 注意此时只是得到了父文件系统中的等价节点
            if parent_fs_path.0.addr_eq(&self.0) {
                return None;
            } else {
                &parent_fs_path
            }
        } else {
            self
        };

        let parent = path
            .0
            .data
            .lock()
            .parent
            .clone()
            .expect("非挂载点的根 DEntry 不应存在");

        debug_assert!(parent.data.lock().inode.is_some());

        Some(Self(parent, path.1.clone()))
    }
}

pub struct RootPath {
    path: SyncUnsafeCell<MaybeUninit<Path>>,
}

impl Deref for RootPath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        unsafe { self.path.get().as_ref_unchecked().assume_init_ref() }
    }
}

impl RootPath {
    pub fn init(&self, path: Path) {
        unsafe { self.path.get().as_mut_unchecked().write(path) };
    }
}

pub static ROOT_PATH: RootPath = RootPath {
    path: SyncUnsafeCell::new(MaybeUninit::uninit()),
};

/// 查找路径对应的 DEntry。有可能找到负 DEntry，但找到的负 DEntry 一定是最后一项。
///
/// 换言之，对于 `/a/b`，查找 `/a/b` 会是正 DEntry，`/a/b/c` 会是负 DEntry，`/a/b/c/d` 会直接返回错误。
pub fn walk_path(path: PathToWalk) -> VfsResult<WalkedPath> {
    let (mut cur_path, pstr) = match path {
        PathToWalk::Absolute(path) => {
            debug_assert!(path.first().is_none_or(|&x| x == b'/'));

            match path.len() {
                0 => return Err(VfsError::FileNotFound),
                1 => return Ok(WalkedPath::Positive(ROOT_PATH.clone())),
                _ => (ROOT_PATH.clone(), ByteStr::new(&path[1..])),
            }
        }
        PathToWalk::Relative(pstr, path) => {
            if pstr.is_empty() {
                return Err(VfsError::FileNotFound);
            }
            (path, pstr)
        }
    };

    // 现在两种情况统一为在 `current_dentry` 下查找非空相对路径 `path`

    let (dir, last) = pstr.rsplit_once(|&c| c == b'/').unwrap_or((b"", pstr));

    // `dir` 的部分可能为空且必须找到，`last` 不为空且可以不找到

    fn walk_step(fs: &FileSystem, cur_path: &Path, name: &ByteStr) -> VfsResult<(bool, Path)> {
        debug_assert_ne!(name, ".");

        if name == ".." {
            let prev_path = cur_path.walk_step_back();
            log::trace!(
                "walk_step: {} --prev--> {}",
                cur_path.0.data.lock().name,
                prev_path.0.data.lock().name
            );
            return Ok((true, prev_path));
        }

        let cached = fs.find_dentry(NonNull::from_ref(&cur_path.0), name);

        let dentry = if let Some(cached) = cached {
            cached
        } else {
            let current_inode = cur_path
                .0
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
                .lookup(name, cur_path.0.clone())?;
            dentry
        };

        let found = dentry.data.lock().inode.is_some();

        log::trace!(
            "walk_step: current_dentry={} name=`{name}` found={found} fs={:#x}",
            cur_path.0.data.lock().name,
            (fs as *const FileSystem).addr()
        );

        return Ok((found, Path(dentry, cur_path.1.clone())));
    }

    let mut fs = unsafe { cur_path.0.filesystem.as_ref() };

    if !dir.is_empty() {
        for name in dir
            .split(|&c| c == b'/')
            .map(ByteStr::new)
            .filter(|&name| name != ".")
        {
            let (found, path) = walk_step(fs, &cur_path, name)?;

            if !found {
                return Err(VfsError::FileNotFound);
            }

            // 因为我们这不是最后一步，因此如果是挂载点则要转到对应文件系统

            if let Some(mount) = path.0.data.lock().mount {
                let mnt = unsafe { mount.as_ref() }.get_cur_mount();
                cur_path = Path(mnt.mount_to.clone(), mnt);
                fs = unsafe { cur_path.0.filesystem.as_ref() };
            } else {
                cur_path = path;
            }
        }
    }

    if last == b"." {
        return Ok(WalkedPath::Positive(cur_path));
    }

    let (found, path) = walk_step(&mut fs, &cur_path, ByteStr::new(last))?;

    if found {
        Ok(WalkedPath::Positive(path))
    } else {
        Ok(WalkedPath::Negative(NPath(path)))
    }
}

pub enum WalkedPath {
    /// 正 DEntry，表示存在
    Positive(Path),
    /// 负 DEntry，表示最后一级不存在
    Negative(NPath),
}

pub struct NPath(pub Path);

impl WalkedPath {
    pub fn ensure(self) -> VfsResult<Path> {
        match self {
            WalkedPath::Positive(dentry) => Ok(dentry),
            WalkedPath::Negative(_) => Err(VfsError::FileNotFound),
        }
    }
    pub fn ensure_neg(self) -> VfsResult<NPath> {
        match self {
            WalkedPath::Positive(_) => Err(VfsError::FileAlreadyExists),
            WalkedPath::Negative(dentry) => Ok(dentry),
        }
    }
}

impl NPath {
    #[track_caller]
    pub fn dir_inode(&self) -> VfsResult<INodeArc> {
        self.0
            .0
            .data
            .lock()
            .parent
            .as_ref()
            .expect("起初有父节点的的 DEntry 应当一直有父节点（rename 是原子的）")
            .data
            .lock()
            .inode
            .clone()
            .ok_or(VfsError::FileNotFound) // 这个确实可能发生，比如试图创建 `/a/b.txt` 结果 `/a` 忽然被删了
    }
}

impl Deref for NPath {
    type Target = Path;

    fn deref(&self) -> &<Self as Deref>::Target {
        &self.0
    }
}

pub enum PathToWalk<'a> {
    Absolute(&'a ByteStr),
    Relative(&'a ByteStr, Path),
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
