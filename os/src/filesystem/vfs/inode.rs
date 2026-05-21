use core::{
    any::Any,
    bstr::ByteStr,
    cell::SyncUnsafeCell,
    mem::{forget, offset_of},
    ops::Deref,
    ptr::NonNull,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
};

use alloc::bstr::ByteString;
use bitflags::bitflags;

use crate::{
    basic::{lru::LruNode, sync::RwLock},
    filesystem::{
        devfs::drivers::DeviceId,
        vfs::{
            DEntryArc, FileSystem, FileSystemOps, PageCache, VfsError, VfsResult,
            cache::CachedValue,
            inode_arc::{ICacheArc, INodeArc},
        },
    },
};

type HardLinksType = u32;

pub struct INodeData<T: ?Sized + INodeOps = dyn INodeOps> {
    pub file_size: u64,
    /// 硬链接数，即文件系统层面的引用计数。
    ///
    /// 注意底层文件系统可以不维护它，此时其值可为 0 ~ 2 内的任何一个值。
    pub hard_links: HardLinksType,

    pub mode: INodeMode,

    pub page_cache: Option<PageCache>,

    pub(in super::super) ops: T,
}

pub struct INode<T: ?Sized + INodeOps = dyn INodeOps> {
    /// 在 FileSyetem 内唯一的编号，对应 Linux 中的 i_ino，具体分配方式由文件系统决定
    pub id: u32,
    /// 指向所属文件系统的指针。文件系统一定比所有下级元素活得都久，因此这个指针永远有效。
    pub fs: NonNull<FileSystem>,

    /// 引用计数，用于 `INodeArc`，其值为 0 时 INode 被链入 LRU 链表或回收（取决于 `in_icache`）
    pub(super) mem_refcount: AtomicU32,

    /// 是否位于 icache 中，若不位于则行为退化为普通引用计数，否则引用计数归零时其仅会被放入 LRU 链表
    ///
    /// 其值由 ICache 锁保护。
    pub(super) in_icache: SyncUnsafeCell<bool>,

    /// LRU 链表中，指向上一个/下一个节点的指针，受 LRU 锁保护
    pub(super) lru: LruNode<INode>,
    /// 指示其是否位于 LRU 链表中，受 LRU 锁保护
    pub(super) in_lru: SyncUnsafeCell<bool>,

    pub(super) dirty_state: AtomicU8,
    pub(super) dirty: LruNode<INode>,

    _dummy: (),

    pub data: RwLock<INodeData<T>>,
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct INodeMode: u16 {
        const UserExecute   = 0o100;
        const UserWrite     = 0o200;
        const UserRead      = 0o400;
        const GroupExecute  = 0o010;
        const GroupWrite    = 0o020;
        const GroupRead     = 0o040;
        const OthersExecute = 0o001;
        const OthersWrite   = 0o002;
        const OthersRead    = 0o004;

        const Directory = 0x4000;
        const RegularFile = 0x8000;
    }
}

impl INodeMode {
    pub fn all_permitted() -> Self {
        Self::from_bits_retain(0o777)
    }
}

bitflags! {
    pub struct INodeDirtyState: u8 {
        const Dirty = 1 << 0;
        const WritingBack = 1 << 1;
    }
}

impl INodeDirtyState {
    pub fn is_pure_clean(&self) -> bool {
        return !self.intersects(Self::Dirty | Self::WritingBack);
    }
}

impl<T: INodeOps> INode<T> {
    pub fn new(id: u32, fs: NonNull<FileSystem>, data: INodeData<T>) -> INodeArc {
        let ret = Self::new_raw(id, fs, data);

        let mut icache_write_guard = unsafe { ret.fs.as_ref() }.icache.write();

        debug_assert!(!icache_write_guard.contains_key(&id));
        unsafe { icache_write_guard.insert_unique_unchecked(id, ICacheArc::from_new(&ret)) };

        ret
    }

    pub fn new_raw(id: u32, fs: NonNull<FileSystem>, data: INodeData<T>) -> INodeArc {
        INodeArc::new(INode {
            id,
            fs,
            mem_refcount: AtomicU32::new(1),
            in_icache: SyncUnsafeCell::new(true),
            in_lru: SyncUnsafeCell::new(false),
            lru: LruNode::new_unlinked(),
            dirty: LruNode::new_unlinked(),
            dirty_state: AtomicU8::new(0),
            _dummy: (),
            data: RwLock::new(data),
        })
    }
}

pub trait INodeOps: Any {
    /// 在当前 INode 下创建一个普通文件节点，该节点原本对应的负 DEntry 是 `dentry`。返回创建的节点。
    fn create(&mut self, mode: INodeMode, dentry: DEntryArc) -> VfsResult<INodeArc>;

    /// 在当前 INode（对应 `dentry`）中查找名为 `name` 的子节点。
    /// 返回对应子节点对应的 DEntry（可能是负 DEntry，这也意味着它永远不应该返回 FileNotFound）。
    fn lookup(&self, name: &ByteStr, dentry: DEntryArc) -> VfsResult<DEntryArc>;

    /// 在当前 INode 中创建一个硬链接，该链接原本对应的负 DEntry 是 `dentry`，指向目标节点 `target`。
    fn link(&mut self, dentry: DEntryArc, target: &INodeArc) -> VfsResult<()>;
    /// 在当前 INode 中删除对应 `dentry` 的子节点（不限于硬链接）。
    fn unlink(&mut self, dentry: &DEntryArc) -> VfsResult<()>;

    /// 在当前 INode 中创建一个目录节点，该节点原本对应的负 DEntry 是 `dentry`。返回创建的节点。
    ///
    /// 它会在创建后自动将新节点放入 `dentry` 中。
    fn mkdir(&mut self, dentry: DEntryArc, mode: INodeMode) -> VfsResult<INodeArc>;
    /// 在当前 INode 中删除对应 `dentry` 的空目录节点。
    fn rmdir(&mut self, dentry: &DEntryArc) -> VfsResult<()>;

    /// 将当前 INode 中名为 `old_name` 的子节点重命名为 `new_name`，若 `new_node` 不为 None 则还会将其移动到 `new_node` 下。
    /// 找不到节点则失败。
    ///
    /// 其等价于 `unlink` + `link`，但保证操作的原子性。
    ///
    /// 由 VFS 层保证 old_name 和 new_name 不是 `.` 或 `..`，且 `new_node` 不为 `self` 的子目录。
    fn rename(
        &mut self,
        new_node: Option<&mut INodeData>,
        old_name: &ByteStr,
        new_name: ByteString,
        flags: INodeRenameFlags,
    ) -> VfsResult<()>;

    /// 创建一个类型为 `kind` 的特殊文件节点（如设备、管道等），该节点原本对应的负 DEntry 是 `dentry`。返回创建的节点。
    ///
    /// 此功能是可选的，底层文件系统默认不实现它。
    fn make_node(
        &mut self,
        kind: SpecialINodeType,
        mode: INodeMode,
        dentry: DEntryArc,
    ) -> VfsResult<INodeArc> {
        Err(VfsError::OperationNotSupported)
    }

    /// 当内存引用计数归零时调用。若返回 `true` 则 INode 可以被销毁或压入 LRU 链表。实现无需顾虑页缓存问题。
    ///
    /// 建议实现至少在硬链接计数也归零时返回 `true`。
    fn evictable_when_mem_ref_zero(&mut self) -> bool;
}

/// `evictable_when_mem_ref_zero` 的默认实现，只在 `hard_links` 也归零时销毁 INode。
pub fn default_evictable_when_mem_ref_zero<T: INodeOps>(inode: &mut T) -> bool {
    return INodeData::get_data(inode).hard_links == 0;
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct INodeRenameFlags: u8 {
        const NoReplace = 1 << 0;
        const Exchange = 1 << 1;
    }
}

pub enum SpecialINodeType {
    FIFO,
    BlockDevice(DeviceId),
    CharDevice(DeviceId),
}

impl<T: INodeOps> INode<T> {
    pub fn get_filesystem<FS: FileSystemOps>(this: &T) -> &FileSystem<FS> {
        let data = RwLock::from_value(NonNull::from_ref(INodeData::get_data(this)));
        let inode = unsafe { data.byte_sub(offset_of!(INode<T>, data)).cast::<INode<T>>() };
        unsafe { inode.as_ref().fs.cast().as_ref() }
    }
    pub fn get_id(this: &T) -> u32 {
        let data = RwLock::from_value(NonNull::from_ref(INodeData::get_data(this)));
        let inode = unsafe { data.byte_sub(offset_of!(INode<T>, data)).cast::<INode<T>>() };
        unsafe { inode.as_ref().id }
    }
}

impl<T: INodeOps> INodeData<T> {
    const OFFSET: usize = offset_of!(INodeData<T>, ops);

    /// 从 `dynamic` 字段的引用反推回整个 `INodeData` 的引用
    ///
    /// （是的，它并非 100% 安全，但那种反例有点反人类了我觉得一般人写不出来）
    pub fn get_data(dynamic: &T) -> &INodeData<T> {
        let dynamic = NonNull::from_ref(dynamic);
        unsafe { dynamic.byte_sub(Self::OFFSET).cast().as_ref() }
    }
    /// 从 `dynamic` 字段的可变借用反推回整个 `INodeData` 的可变借用
    ///
    /// （是的，它并非 100% 安全，但那种反例有点反人类了我觉得一般人写不出来）
    pub fn get_data_mut(dynamic: &mut T) -> &mut INodeData<T> {
        let dynamic = NonNull::from_mut(dynamic);
        unsafe { dynamic.byte_sub(Self::OFFSET).cast().as_mut() }
    }
}

impl<T: ?Sized + INodeOps> INodeData<T> {
    /// 在当前 INode 下创建一个普通文件节点，该节点原本对应的负 DEntry 是 `dentry`。返回创建的节点。
    ///
    /// 和 [`INodeOps::create`] 不同，它会自动将新节点放入 `dentry` 中。
    pub fn create(&mut self, mode: INodeMode, dentry: DEntryArc) -> VfsResult<INodeArc> {
        let new_node = self.ops.create(mode, dentry.clone())?;
        dentry.data.lock().inode.replace(new_node.clone());
        Ok(new_node)
    }
    /// 在当前 INode 下创建一个类型为 `kind` 的特殊文件节点（如设备、管道等），该节点原本对应的负 DEntry 是 `dentry`。返回创建的节点。
    ///
    /// 和 [`INodeOps::make_node`] 不同，它会自动将新节点放入 `dentry` 中。
    pub fn make_node(
        &mut self,
        kind: SpecialINodeType,
        mode: INodeMode,
        dentry: DEntryArc,
    ) -> VfsResult<INodeArc> {
        let new_node = self.ops.make_node(kind, mode, dentry.clone())?;
        dentry.data.lock().inode.replace(new_node.clone());
        Ok(new_node)
    }

    /// 在当前 INode 中创建一个硬链接，该链接原本对应的负 DEntry 是 `dentry`，指向目标节点 `target`。
    pub fn link(&mut self, dentry: DEntryArc, target: &INodeArc) -> VfsResult<()> {
        self.ops.link(dentry, target)
    }
    /// 在当前 INode 中删除对应 `dentry` 的子节点（不限于硬链接）。
    pub fn unlink(&mut self, dentry: &DEntryArc) -> VfsResult<()> {
        self.ops.unlink(dentry)?;
        unhash(dentry.deref());
        Ok(())
    }

    /// 在当前 INode 中创建一个目录节点，该节点原本对应的负 DEntry 是 `dentry`。返回创建的节点。
    ///
    /// 和 `INodeOps::mkdir` 不同，它会自动将新节点放入 `dentry` 中。
    pub fn mkdir(&mut self, dentry: DEntryArc, mode: INodeMode) -> VfsResult<INodeArc> {
        let new_node = self.ops.mkdir(dentry.clone(), mode)?;
        dentry.data.lock().inode.replace(new_node.clone());
        Ok(new_node)
    }
    /// 在当前 INode 中删除对应 `dentry` 的空目录节点。
    pub fn rmdir(&mut self, dentry: &DEntryArc) -> VfsResult<()> {
        if self.hard_links > 2 {
            return Err(VfsError::DirectoryNotEmpty);
        }

        self.ops.rmdir(dentry)?;
        unhash(dentry.deref());
        // dentry.data.lock().children().for_each(unhash);
        Ok(())
    }
}

fn unhash(entry: &(impl CachedValue + ?Sized)) {
    let mut cache_write_guard = entry.cache_wlock();

    let in_cache = unsafe { entry.in_cache().as_mut_unchecked() };
    if *in_cache {
        let cache = entry.remove_from_cache(&mut cache_write_guard);

        crate::println!("{:p} unhashed", entry);

        if entry.mem_refcount().load(Ordering::Relaxed) == 0 {
            // 无引用的纯缓存情形（注意 dcache_write_guard 不能提前 drop，否则清理线程可能与这里发生竞态）
            {
                let mut lru_sent_mtx_guard = entry.lru_sent_lock();
                let old_in_lru = unsafe { entry.in_lru().replace(false) };
                if old_in_lru {
                    unsafe { entry.my_lru().lru_drop(&mut lru_sent_mtx_guard) };
                }
            }
            drop(cache_write_guard); // 避免死锁
            drop(cache);
        } else {
            // 还在使用，此时设置 in_cache 就够了
            *in_cache = false;
            forget(cache);
        }
    }
}

unsafe impl CachedValue for INode {
    const DEBUG_NAME: &'static str = "INode";

    fn mem_refcount(&self) -> &AtomicU32 {
        &self.mem_refcount
    }

    fn in_cache(&self) -> *mut bool {
        self.in_icache.get()
    }

    fn my_lru(&self) -> &LruNode<Self> {
        &self.lru
    }

    fn in_lru(&self) -> *mut bool {
        self.in_lru.get()
    }

    fn lru_sent_lock(&self) -> impl Deref<Target = LruNode<Self>> + core::ops::DerefMut {
        unsafe { self.fs.as_ref() }.ilru_sent.lock()
    }

    type CacheKey = u32;

    type CacheValue = ICacheArc;

    fn cache_wlock(
        &self,
    ) -> impl Deref<Target = hashbrown::HashMap<Self::CacheKey, Self::CacheValue>> + core::ops::DerefMut
    {
        unsafe { self.fs.as_ref() }.icache.write()
    }

    fn remove_from_cache(
        &self,
        cache: &mut hashbrown::HashMap<Self::CacheKey, Self::CacheValue>,
    ) -> Self::CacheValue {
        unsafe { FileSystem::remove_icache_entry(cache, self.id) }
    }
}
