use core::{any::Any, bstr::ByteStr, mem::offset_of, ptr::NonNull, sync::atomic::AtomicU32};

use alloc::bstr::ByteString;
use bitflags::bitflags;

use crate::{
    basic::sync::RwLock,
    filesystem::vfs::{
        DEntryArc, FileSystem, FileSystemOps, PageCache, VfsResult, inode_arc::INodeArc,
    },
};

type HardLinksType = u32;

pub struct INodeData<T: ?Sized + INodeOps = dyn INodeOps> {
    pub file_size: u64,
    /// 硬链接数，即文件系统层面的引用计数。
    pub hard_links: HardLinksType,

    pub mode: INodeMode,

    pub page_cache: Option<PageCache>,

    pub dynamic_data: T,
}

pub struct INode<T: ?Sized + INodeOps = dyn INodeOps> {
    /// 在 FileSyetem 内唯一的编号，对应 Linux 中的 i_ino，具体分配方式由文件系统决定
    pub id: u32,
    /// 指向所属文件系统的指针。文件系统一定比所有下级元素活得都久，因此这个指针永远有效。
    pub fs: NonNull<FileSystem>,

    /// 引用计数，用于 `INodeArc`
    pub(super) mem_refcount: AtomicU32,

    _dummy: (),

    pub data: RwLock<INodeData<T>>,
}

bitflags! {
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
    }
}

impl<T: INodeOps> INode<T> {
    pub fn new(id: u32, fs: NonNull<FileSystem>, data: INodeData<T>) -> INodeArc {
        INodeArc::new(INode {
            id,
            fs,
            mem_refcount: AtomicU32::new(1),
            _dummy: (),
            data: RwLock::new(data),
        })
    }
}

pub trait INodeOps: Any {
    /// 在当前 INode 下创建一个普通文件节点，该节点原本对应的负 DEntry 是 `dentry`。返回创建的节点。
    fn create(&mut self, mode: INodeMode, dentry: DEntryArc) -> VfsResult<INodeArc>;

    /// 在当前 INode（对应 `dentry`）中查找名为 `name` 的子节点。
    /// 返回对应子节点对应的 DEntry（可能是负 DEntry）。
    fn lookup(&self, name: &ByteStr, dentry: DEntryArc) -> VfsResult<DEntryArc>;

    /// 在当前 INode 中创建一个硬链接，该链接原本对应的负 DEntry 是 `dentry`，指向目标节点 `target`。
    fn link(&mut self, dentry: DEntryArc, target: &INodeArc) -> VfsResult<()>;
    /// 在当前 INode 中删除对应 `dentry` 的子节点（不限于硬链接）。
    fn unlink(&mut self, dentry: &DEntryArc) -> VfsResult<()>;

    /// 在当前 INode 中创建一个目录节点，该节点原本对应的负 DEntry 是 `dentry`。返回创建的节点。
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

    /// 从当前 INode 的 `offset` 位置开始读取数据到内核态缓冲区 `buf` 中，返回实际读取的字节数。
    fn kernel_read(&self, offset: u64, buf: &mut [u8]) -> VfsResult<usize>;
    /// 从内核态缓冲区 `buf` 中写入数据到当前 INode 的 `offset` 位置，返回实际写入的字节数。
    fn kernel_write(&mut self, offset: u64, buf: &[u8]) -> VfsResult<usize>;

    /// 当内存引用计数归零时调用。若返回 `true` 则 INode 被销毁，反之将当前 INode 放入单独缓存中以备重用。
    ///
    /// 建议实现至少在硬链接计数也归零时返回 `true`。
    fn should_drop_when_mem_refcount_zero(&mut self) -> bool;
}

/// `should_drop_when_mem_refcount_zero` 的默认实现，只在 `hard_links` 也归零时销毁 INode。
pub fn default_should_drop_when_mem_refcount_zero<T: INodeOps>(inode: &mut T) -> bool {
    return INodeData::get_data(inode).hard_links == 0;
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct INodeRenameFlags: u8 {
        const NoReplace = 1 << 0;
        const Exchange = 1 << 1;
    }
}

impl<T: INodeOps> INode<T> {
    pub fn get_filesystem<FS: FileSystemOps>(this: &T) -> &FileSystem<FS> {
        let data = RwLock::from_value(NonNull::from_ref(INodeData::get_data(this)));
        let inode = unsafe { data.byte_sub(offset_of!(INode<T>, data)).cast::<INode<T>>() };
        unsafe { inode.as_ref().fs.cast().as_ref() }
    }
}

impl<T: INodeOps> INodeData<T> {
    const OFFSET: usize = offset_of!(INodeData<T>, dynamic_data);

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
