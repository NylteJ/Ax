use core::{
    hash::Hash,
    hint::unreachable_unchecked,
    mem::{MaybeUninit, forget, offset_of, replace},
    num::NonZero,
    ops::Deref,
    pin::Pin,
    ptr::{NonNull, addr_eq},
    sync::atomic::Ordering,
};

use alloc::sync::Arc;
use derive_more::Debug;
use hashbrown::{Equivalent, HashMap};
use uninit::extension_traits::{MaybeUninitExt, Transpose};

use crate::{
    basic::{
        UnsafeInited,
        bstr::{ByteStr, ByteString},
        lru::{LruNode, LruPtr},
        sync::{Mutex, RwLock},
    },
    filesystem::{
        devfs::drivers::DeviceId,
        vfs::{
            DEntry, DEntryArc, DEntryData, INode, INodeArc, INodeDirtyState, INodeMode, Mount,
            VfsError, VfsResult, cache::get_value_arc_from_cache, dentry_arc::DCacheArc,
            inode_arc::ICacheArc,
        },
    },
    memory::{
        PAGE_BYTES,
        page_meta::{PageFlags, PageType},
    },
};

/// 描述整个文件系统，对应 Linux 中的超级块（super block）
#[derive(Debug)]
pub struct FileSystem<T: ?Sized + FileSystemOps = dyn FileSystemOps> {
    pub dev: DeviceId,

    pub(super) dcache: RwLock<HashMap<DCacheKey, DCacheArc>>,
    pub(super) icache: RwLock<HashMap<u32, ICacheArc>>,
    pub root_inode: INodeArc,
    pub root_dentry: DEntryArc,

    #[debug(skip)]
    pub dlru_sent: Mutex<LruNode<DEntry>>,
    #[debug(skip)]
    pub ilru_sent: Mutex<LruNode<INode>>,

    #[debug(skip)]
    pub dirty_sent: Mutex<LruNode<INode>>,

    #[debug(skip)]
    _dummy: (),

    #[debug(skip)]
    pub ops: T,
}

impl<T: FileSystemOps + 'static> FileSystem<T> {
    pub fn new(
        roots: fn(NonNull<FileSystem>, &T) -> (INodeArc, DEntryArc),
        dev: DeviceId,
        ops: T,
    ) -> Pin<Arc<FileSystem>> {
        let mut arc = Arc::new_cyclic(|weak| {
            let addr = NonNull::new(weak.as_ptr() as *mut FileSystem<T>).unwrap();
            let (root_inode, init_root_dentry) = roots(addr, &ops);
            root_inode.data.write().mode |= INodeMode::Directory; // 根目录一定要是目录 \o/
            FileSystem {
                dev,
                dcache: RwLock::new(HashMap::new()),
                icache: RwLock::new(HashMap::new()),
                root_inode,
                root_dentry: init_root_dentry,
                dlru_sent: Mutex::new(LruNode::new_unlinked()),
                ilru_sent: Mutex::new(LruNode::new_unlinked()),
                dirty_sent: Mutex::new(LruNode::new_unlinked()),
                _dummy: (),
                ops,
            }
        });

        unsafe {
            let fs = Arc::get_mut_unchecked(&mut arc);

            fs.dlru_sent.get_mut().to_sentinel();
            fs.ilru_sent.get_mut().to_sentinel();
            fs.dirty_sent.get_mut().to_sentinel();
            fs.icache
                .get_mut()
                .insert_unique_unchecked(fs.root_inode.id, ICacheArc::from_new(&fs.root_inode));
        }

        unsafe { Pin::new_unchecked(arc) }
    }
}

impl FileSystem {
    pub fn find_dentry<'a>(&self, parent: NonNull<DEntry>, name: &ByteStr) -> Option<DEntryArc> {
        let key = DCacheKeyRef { parent, name };
        let dcache_read_guard = self.dcache.read();
        let cache = dcache_read_guard.get(&key)?;
        Some(unsafe { get_value_arc_from_cache(cache.deref()) }) // 其内部会修改 LRU 链表
    }
    pub fn find_inode<'a>(&self, id: u32) -> Option<INodeArc> {
        let icache_read_guard = self.icache.read();
        let cache = icache_read_guard.get(&id)?;
        Some(unsafe { get_value_arc_from_cache(cache.deref()) }) // 其内部会修改 LRU 链表
    }

    pub unsafe fn remove_dcache_entry(
        dcache: &mut HashMap<DCacheKey, DCacheArc>,
        data: &DEntryData,
    ) -> DCacheArc {
        // TODO: 如果改成侵入式链表这里能免去查找开销
        dcache
            .remove(&DCacheKeyRef {
                parent: data
                    .parent
                    .as_ref()
                    .expect("无父节点的 DEntry 不应位于 DCache 中")
                    .ptr(),
                name: data.name.as_ref(),
            })
            .expect("LRU 链表 中的每一项应当都在 DCache 中")
    }
    pub unsafe fn remove_icache_entry(icache: &mut HashMap<u32, ICacheArc>, id: u32) -> ICacheArc {
        // TODO: 如果改成侵入式链表这里能免去查找开销
        icache
            .remove(&id)
            .expect("LRU 链表 中的每一项应当都在 ICache 中")
    }

    /// 回收至多 `max_count` 个 DEntry，返回实际回收的数量
    pub fn gc(&self, max_count: NonZero<usize>) -> usize {
        // 注意这里 DCache 锁必须一直持有，以防查找操作复活 DEntry
        let mut dcache_guard = self.dcache.write();
        let mut lru_guard = self.dlru_sent.lock();

        let sent = LruPtr::from_sentinel(&mut lru_guard);
        let lru = lru_guard.lru.get_mut();

        let first = lru.prev; // 别忘了我们是反着销毁的！

        const OFFSET: usize = offset_of!(DEntry, lru);

        let mut current_node = lru.prev;
        for _ in 0..max_count.get() {
            if current_node.addr_eq(&sent) {
                break;
            }

            let prev_node = unsafe { current_node.as_node().as_ref().prev };
            let node = unsafe { current_node.as_value(OFFSET).as_ref() };

            forget(unsafe { Self::remove_dcache_entry(&mut dcache_guard, &node.data.lock()) }); // 稍后统一销毁

            current_node = prev_node;
        }

        // 现在，current_node 是第一个未被回收的节点，我们需要将它和 sent 连接起来
        // 注意尽管 current_node 可能就是 sent，lru_guard 并不持有真正意义上的引用，因此不违反别名规则
        lru.prev = current_node;
        unsafe { current_node.as_node().as_mut() }.next = sent;

        // 接下来我们不再需要 DCache 锁或 LRU 锁，释放它们，然后真正销毁那些 DEntry
        // 销毁必须是延迟的，否则销毁过程中可能有新的 DEntry 因子 DEntry 全部被销毁而需要链入 LRU，导致死锁
        drop(lru_guard);
        drop(dcache_guard);

        // 注意最后一个要销毁的块的 prev 仍然指向 current_node（尽管它现在可能已经悬垂）
        let mut count = 0;
        let mut current = first;
        while !current.addr_eq(&current_node) {
            let prev = unsafe { current.as_node().as_ref() }.prev;

            unsafe { DCacheArc::drop_manually(current.as_value(OFFSET).as_ref()) };

            current = prev;
            count += 1;
        }

        count
    }

    /// 尽可能多地回收 DEntry，返回实际回收的数量
    ///
    /// 注意，由于回收 DEntry 这一过程可能导致新的 DEntry 进入 LRU 链表，它并不保证回收全部可回收的 DEntry。
    pub fn full_gc(&self) -> usize {
        self.gc(NonZero::<usize>::MAX)
    }

    pub fn writeback(&self, max_count: NonZero<usize>) {
        let mut list_guard = self.dirty_sent.lock();

        let sent = LruPtr::from_sentinel(&mut list_guard);
        let list = list_guard.lru.get_mut();

        let first = list.prev; // 别忘了我们是反着销毁的！

        const OFFSET: usize = offset_of!(INode, dirty);

        let mut count = 0;
        let mut current_node = list.prev;
        while count != max_count.get() {
            if current_node.addr_eq(&sent) {
                break;
            }

            let prev_node = unsafe { current_node.as_node().as_ref().prev };
            let node = unsafe { current_node.as_value(OFFSET).as_ref() };

            let old_state = node
                .dirty_state
                .swap(INodeDirtyState::WritingBack.bits(), Ordering::Relaxed);
            debug_assert_eq!(old_state, INodeDirtyState::Dirty.bits());

            current_node = prev_node;
            count += 1;
        }

        // 现在，current_node 是第一个未被回收的节点，我们需要将它和 sent 连接起来
        // 注意尽管 current_node 可能就是 sent，list_guard 并不持有真正意义上的引用，因此不违反别名规则
        list.prev = current_node;
        unsafe { current_node.as_node().as_mut() }.next = sent;

        // 现在释放脏 INode 链表锁
        drop(list_guard);

        let fs_ops = if count > 0 {
            &unsafe { first.as_value(OFFSET).as_ref().fs.as_ref() }.ops
        } else {
            return;
        };

        let mut re_dirty_sent = LruNode::new_unlinked();
        unsafe { re_dirty_sent.to_sentinel() };
        let re_dirty_sent_ptr = re_dirty_sent.lru.get_mut().next;

        current_node = first;
        for _ in 0..count {
            let node = unsafe { current_node.as_value(OFFSET).as_ref() };
            let node_addr = (node as *const INode).addr();

            log::trace!("正在回写 INode {} ({node_addr:#x})", node.id);

            let node_data_rguard = node.data.read();
            if let Some(page_cache) = &node_data_rguard.page_cache {
                let data = page_cache.data.write().dirty_sent.take();
                drop(node_data_rguard);

                for page in data {
                    let old_state = page
                        .flags
                        .swap(PageFlags::WritingBack.bits(), Ordering::Relaxed);
                    debug_assert_eq!(old_state, PageFlags::Dirty.bits());

                    let PageType::File { page_index, .. } = page.page_type else {
                        unsafe { unreachable_unchecked() };
                    };

                    log::trace!(
                        "正在回写 INode {} ({node_addr:#x}) 的 Page {}，物理页起始地址 {:#x}",
                        node.id,
                        page_index,
                        page.get_addr()
                    );

                    let writeback_result = fs_ops.writeback_page(node, page_index, unsafe {
                        page.visit_memory().transpose_ref().assume_init_by_ref()
                    });

                    if let Err(err) = writeback_result {
                        todo!("错误处理");
                    }
                }
            }

            // TODO: 元数据？

            // 回写完毕
            let old_state = node
                .dirty_state
                .fetch_and(!(INodeDirtyState::WritingBack.bits()), Ordering::Relaxed);
            let old_state = INodeDirtyState::from_bits_retain(old_state);

            let prev_node = unsafe { current_node.as_node().as_ref() }.prev;

            if old_state.contains(INodeDirtyState::Dirty) {
                // 再度变脏，需要挂回
                // 注意这里我们不考虑 mmap，因为 munmap/fsync 时我们总会再检测一次并统一写回（代价就是可能有冗余写回，以及最坏延迟略长）
                log::trace!("INode {} ({node_addr:#x}) 在回写后再度变脏，挂回", node.id);
                unsafe { re_dirty_sent.lru_add(node, OFFSET) };
            } else {
                drop(unsafe { get_value_arc_from_cache::<INodeArc>(node) }); // 触发可能的引用计数归零
            }

            current_node = prev_node;
        }

        // 挂回重新变脏的 INode
        let re_dirty_sent = re_dirty_sent.lru.get_mut();
        if !re_dirty_sent.next.addr_eq(&re_dirty_sent_ptr) {
            let mut list_guard = self.dirty_sent.lock();

            let mut fst = re_dirty_sent.next.as_node();
            let mut lst = re_dirty_sent.prev.as_node();

            unsafe { fst.as_mut() }.prev = sent;
            unsafe { lst.as_mut() }.next = list_guard.lru.get_mut().next;
            let sent_next = replace(&mut list_guard.lru.get_mut().next, re_dirty_sent.next);
            unsafe { sent_next.as_node().as_mut() }.prev = re_dirty_sent.next;
        }
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

pub static ROOT_PATH: UnsafeInited<Path> = unsafe { UnsafeInited::zeroed() };

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

    fn walk_step(fs: &mut &FileSystem, cur_path: &Path, name: &ByteStr) -> VfsResult<(bool, Path)> {
        debug_assert_ne!(name, ".");

        if name == ".." {
            let prev_path = cur_path.walk_step_back();
            log::trace!(
                "walk_step: {} --prev--> {}",
                { cur_path.0.data.lock().name.clone() },
                prev_path.0.data.lock().name,
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
                .ops
                .lookup(name, cur_path.0.clone())?;
            dentry
        };

        let found = dentry.data.lock().inode.is_some();

        log::trace!(
            "walk_step: current_dentry={} name=`{name}` found={found} fs={:#x}",
            cur_path.0.data.lock().name,
            (*fs as *const FileSystem).addr()
        );

        // 如果是挂载点则要转到对应文件系统

        let dentry = if let Some(mount) = { dentry.data.lock().mount } {
            debug_assert!(found);

            let mnt = unsafe { mount.as_ref() }.get_cur_mount();
            let mnt_to = mnt.mount_to.clone();
            *fs = unsafe { mnt_to.filesystem.as_ref() };

            log::trace!(
                "walk_path 穿越挂载点：{} ---> {}",
                { dentry.data.lock().name.clone() },
                mnt_to.data.lock().name,
            );

            mnt_to
        } else {
            dentry
        };

        return Ok((found, Path(dentry, cur_path.1.clone())));
    }

    let mut fs = unsafe { cur_path.0.filesystem.as_ref() };

    if !dir.is_empty() {
        for name in dir
            .split(|&c| c == b'/')
            .map(ByteStr::new)
            .filter(|&name| name != ".")
        {
            let (found, path) = walk_step(&mut fs, &cur_path, name)?;

            if !found {
                return Err(VfsError::FileNotFound);
            }

            cur_path = path;
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

impl<'a> PathToWalk<'a> {
    pub fn auto(path: &'a ByteStr, get_cur_path: impl FnOnce() -> Path) -> VfsResult<Self> {
        if *path.first().ok_or(VfsError::FileNotFound)? == b'/' {
            Ok(Self::Absolute(path))
        } else {
            Ok(Self::Relative(path, get_cur_path()))
        }
    }
}

pub trait FileSystemOps: Send + Sync {
    /// 写回页。此操作刻意设计成不持有任何锁，底层文件系统可以视情况同步，但其始终可以假设 page_data 不会被修改
    ///
    /// （是的，我们并没有任何措施避免它被修改，但这是 Rust 编译器无法识别的数据竞争，因此其后果只是单纯的数据损坏而
    /// 非时间旅行。由于用户在不调用 fsync 等函数时本来就不应指望数据被完好保存，一切都很完美）
    fn writeback_page(
        &self,
        inode: &INode,
        page_index: u64,
        page_data: &[u8; 4096],
    ) -> VfsResult<()>;

    /// 从底层设备读取到文件。此操作刻意设计成不持有任何锁，底层文件系统可以视情况同步，但其始终可以假设 page_data 无其它引用
    ///
    /// 此函数保证只在页缓存未命中时调用。实现必须从首个字节开始写入，并返回实际写入的字节数。
    fn read_to_page(
        &self,
        inode: &INode,
        page_index: u64,
        buffer: &mut [MaybeUninit<u8>; 4096],
    ) -> VfsResult<usize>;
}

static_assertions::const_assert_eq!(PAGE_BYTES, 4096); // 见 writeback_page 定义。这是 Rust 编译器的神奇妙妙小错误

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

unsafe impl Send for DCacheKey {}
unsafe impl Sync for DCacheKey {}

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
