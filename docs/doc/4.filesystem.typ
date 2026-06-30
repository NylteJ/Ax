= 文件系统模块 <filesystem>
此模块负责为应用程序和内核提供读写文件系统的能力，也是“一切皆文件”的 Unix 哲学得以实现的基础。

Ax 架构了完备的虚拟文件系统（VFS），并适配了 ext4 磁盘文件系统和 tmpfs 内存文件系统，以及 devfs、procfs、pipefs、eventpollfs 这几个伪文件系统。

== VFS
Ax 在学习了 Linux VFS 中的许多思想后，架构了十分完善的独有 VFS，其能统一各个文件系统的操作，并替底层文件系统执行页缓存等通用逻辑，是文件系统模块的精华所在。

Ax VFS 主要由 `FileSystemType`、`FileSystem`、`INode`、`DEntry`、`File`、`PageCache`、`Mount` 和 `MountPoint` 组成，大部分结构都借助了 Rust 的动态大小类型（DST）支持，支持多态和具体文件系统自定义字段的同时避免了额外的堆分配或虚派发开销。

=== Rust DST
和一般的面向对象语言与 C++ 不同，Rust 不支持继承，而是鼓励程序员用组合替代继承，使用面向接口的编程方法。然而，VFS 的各个对象存在非常多的共有字段和方法，VFS 的许多机制也必须在特定对象上进行才能保证安全性，将共有字段和方法全部下放到具体文件系统中，是极为冗余且低效的。为此，必须要有某种方法，能从 trait object 中获取这些共有字段。

许多往届作品选择在 trait 中添加一个接口，具体文件系统须在自己的类中添加这些共有字段，并实现这个接口来返回它们。然而，这会在底层增加一次极难优化的虚派发。VFS 层需要极其频繁地使用这些共有字段，频繁调用虚函数，或是不断传递这些共有字段，都是非常低效的。

另一个方法是使用一个泛型结构体，其包含这些共有字段以及一个 `Box`，指向文件系统私有数据以及对应虚表。但这会带来一次额外的堆分配和间接访问，所以仍然不够好。

Ax 创新性地利用 Rust 中的 DST 特性，用泛型结构体表示对象，其包含共有字段，以及位于末尾的动态大小字段。这允许创建 `INode<TmpFsINode>`，并将其转换为 `&INode<dyn vfs::InodeOps>`。这样，只有一次堆分配，而且不会产生任何额外间接引用或虚调用，实现了零成本抽象#footnote[这实际上是在手动模拟 C++ 的继承机制。]。

这样，每个 VFS 对象被拆分成基础结构体和多态 trait 两部分（如 `struct FileSystemType<T>` 和 `trait FileSystemTypeOps`），具体文件系统只要实现多态 trait，其它的事 VFS 都会自行做好。

=== `FileSystemType`
此类型描述一个文件系统类别，如 ext4、tmpfs、devfs。其结构体和 trait 的声明如下：
```rust
pub struct FileSystemType<T: ?Sized + FileSystemTypeOps = dyn FileSystemTypeOps> {
    pub name: &'static ByteStr,
    pub(super) ops: T,
}

pub trait FileSystemTypeOps: Send + Sync {
    fn new_fs(&self, block_device: Option<&'static dyn BlockDevice>) -> Pin<Arc<FileSystem>>;
}
```

每类文件系统只会存在这一个类型。将此对象注册进 VFS 的全局列表后，便能利用字符串找到对应的文件系统类型，并能结合可选的块设备创建一个文件系统实例。

=== `FileSystem`
此类型描述一个文件系统实例。在有的内核中，它叫超级块（Super Block）。其结构体和 trait 的声明如下（有删节以便于排版）：
```rust
pub struct FileSystem<T: ?Sized + FileSystemOps = dyn FileSystemOps> {
    pub dev: DeviceId,

    pub(super) dcache: RwLock<HashMap<DCacheKey, DCacheArc>>,
    pub(super) icache: RwLock<HashMap<u32, ICacheArc>>,
    pub root_inode: INodeArc,
    pub root_dentry: DEntryArc,

    pub dlru_sent: Mutex<LruNode<DEntry>>,
    pub ilru_sent: Mutex<LruNode<INode>>,

    pub dirty_sent: Mutex<LruNode<INode>>,

    _dummy: (),

    pub ops: T,
}

pub trait FileSystemOps: Send + Sync {
    fn writeback_page(
        &self,
        inode: &INode,
        page_index: u64,
        page_data: &[u8; 4096],
    ) -> VfsResult<()>;

    fn read_to_page(
        &self,
        inode: &INode,
        page_index: u64,
        buffer: &mut [MaybeUninit<u8>; 4096],
    ) -> VfsResult<usize>;
}
```

其提供了底层的写回和填充页缓存的能力。除此之外，它包含了 `INode` 和 `DEntry` 的缓存与 LRU 列表，以及脏 `INode` 列表和根节点对应的 `INode` 与 `DEntry`。

==== `INode` 缓存（ICache）与脏 `INode` 列表
除部分显式声明为 `uncached` 的 `INode` 外，所有 `INode` 都位于 `INode` 缓存哈希表中，以 `INode` ID 为键。保证 `INode` ID 唯一性是底层文件系统的工作。

由于 ext4 等底层文件系统通常只使用 ID 引用 `INode`，当需要根据 ID 操作某个 `INode` 时，底层文件系统便可以先搜索 `INode` 缓存，只在缓存未命中时真正触发磁盘读取。

每个 `INode` 都存在一个可驱逐属性，指示内存引用计数归零时其是否可以从内存中移除。如果计数归零时其不能被驱逐，则其会继续驻留在内存中。如果它仍位于缓存中，则它仍然可以被查找操作复活。

如果内存引用计数归零时 `INode` 可驱逐，对于不在 `INode` 缓存中的 `INode`，其会被即刻释放。否则，它会被挂入 `INode` LRU 列表中#footnote[此过程会先持有 `INode` 缓存的写锁，以避免竞态。]。如果一个计数原本为 0 的 `INode` 因查找而复活，则它会被从 `INode` LRU 列表中取下。

`INode` 可驱逐的必要条件是 `INode` 非脏。如果一个 `INode` 自读取过后被写入过，无论写入的是 `INode` 本身还是其页缓存，它都会变为脏的。状态原子地由干净变脏时，它会被挂入脏 `INode` 列表中。`INode` 可驱逐的其它条件可以由具体文件系统自行定义。

写回时，内核会一次性取下所有的脏 `INode`，然后在不持有锁的情况下，逐个将它们的状态变为写回中并写回。写回完成后，内核会原子地改变 `INode` 状态。若 `INode` 在此过程中又因写入变脏，则它会重新变成脏的并挂到脏 `INode` 列表中，否则它现在干净。如果 `INode` 写回后变得干净，内核会尝试重新触发其引用计数归零效果。

回收时，内核会获取 `INode` 缓存写锁，然后遍历 `INode` LRU 列表。不难发现，此时其中的 `INode` 都是干净而且引用计数为 0 的，这意味着它们都可以被回收。此时内核会逐个销毁它们。

==== `DEntry` 缓存（DCache）
`DEntry` 缓存和 `DEntry` LRU 列表的工作方式和 `INode` 版本的几乎完全一致，但 `DEntry` 没有脏的概念，而且引用计数为 0 时总是可以无副作用地销毁（等价于总是可驱逐），因此情况要较为简化。

`DEntry` 缓存以父 `DEntry` 的地址和子 `DEntry` 的名称为键，包括了正 `DEntry` 和负 `DEntry`。前者指示一个存在对应 `INode` 的目录项，后者指示一个不存在对应 `INode` 的“目录项”，代表一次失败的查找。负 `DEntry` 的存在让内核在失败路径也能享受到缓存的便利，提高性能的同时降低了拒绝服务攻击的风险。

==== 文件系统列表
所有创建的文件系统本身也会被放到到一个全局列表中，以便内核在内存不足时针对所有现存文件系统执行回写和清理操作。

=== `INode`
此类型描述一个文件、目录或设备文件。基本上，它描述一个文件系统意义上的对象。其结构体和 trait 的声明如下（有删节以便于排版）：
```rust
pub struct INodeData<T: ?Sized + INodeOps = dyn INodeOps> {
    pub file_size: u64,
    /// 硬链接数，即文件系统层面的引用计数。
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

    fn symlink(&mut self, target: ByteString, dentry: DEntryArc) -> VfsResult<INodeArc>;
    fn read_symlink(&self) -> VfsResult<ByteString>;

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

    fn get_kstat(&self) -> VfsResult<KstatInner>;

    /// 当内存引用计数归零时调用。若返回 `true` 则 INode 可以被销毁或压入 LRU 链表。实现无需顾虑页缓存问题。
    ///
    /// 建议实现至少在硬链接计数也归零时返回 `true`。
    fn evictable_when_mem_ref_zero(&mut self) -> bool;

    fn truncate_shrink_size(&mut self, new_size: u64) {}
}
```

`INode` 的大部分字段和一部分方法先前介绍 `INode` 缓存和脏列表时已经介绍过，剩余方法基本只是转发到对应底层文件系统。

这里 Ax 将 `INode` 的一部分可变数据用单一锁保护到 `INodeData` 中，这是因为那部分数据本就不能孤立变化，也就没有必要单设一系列锁。

=== `DEntry`
该类型描述一个目录下的条目（正 `DEntry`），或者一次失败的查找（负 `DEntry`）。它基本完全是 VFS 层的概念，底层文件系统几乎不需要自定义其字段，只要实现其方法即可。其结构体和 trait 的声明如下（有删节以便于排版）：
```rust
pub struct DEntryData<T: ?Sized + DEntryOps = dyn DEntryOps> {
    pub parent: Option<DEntryArc>,
    pub name: ByteString,

    /// 子 DEntry 侵入式双向链表的哨兵节点
    pub children_sentinel: DEntryLinkedListNode,

    /// 关联的 INode，为 None 表示该 dentry 是负 dentry（标识文件不存在的 dentry）
    pub inode: Option<INodeArc>,

    /// 挂载点，我们不持有它的所有权，但其保证自己有效
    pub mount: Option<NonNull<MountPoint>>,

    pub ops: T,
}

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
    /// 其值由 DCache 锁保护。
    pub(super) in_dcache: SyncUnsafeCell<bool>,

    /// LRU 链表中，指向上一个/下一个节点的指针，受 LRU 锁保护
    pub(super) lru: LruNode<DEntry>,
    /// 指示其是否位于 LRU 链表中，受 LRU 锁保护
    pub(super) in_lru: SyncUnsafeCell<bool>,

    /// 所属的文件系统，其保证有效且不会改变
    pub filesystem: NonNull<FileSystem>,

    /// 实际数据
    pub data: Mutex<DEntryData<T>>,
}

pub trait DEntryOps: Send + Sync {
    /// 打开当前 DEntry 对应的文件。
    fn open(&self, mount: Arc<Mount>, flags: FileFlags) -> VfsResult<FileArc>;
}
```

`INode` 的大部分字段先前介绍 `DEntry` 缓存时已经介绍过。

所有 `DEntry` 架构成了一颗所有权倒置的树，父节点持有所有子节点的无所有权引用，子节点持有父节点的强引用。这是因为子 `DEntry` 存在时内核必须保证其父目录的 `DEntry` 也存在，从而可以沿着它向上构建出它的路径。此外，为避免悬垂引用，子 `DEntry` 销毁前，也会从父 `DEntry` 的子 `DEntry` 列表中删除自身。

=== `File`
该类型描述一个打开的文件句柄。其一定对应一个正 `DEntry`。文件读写等操作都是在这一层进行的。其结构体和 trait 的声明如下（有删节以便于排版）：
```rust
pub struct File<T: ?Sized + FileOps = dyn FileOps> {
    /// 指向对应 INode 的指针，此值自创建后便不会改变。其有效性由 DEntry 的 Arc 间接保证。
    pub inode: NonNull<INode>,
    pub data: Mutex<FileData<T>>,
}

pub struct FileData<T: ?Sized + FileOps = dyn FileOps> {
    pub path: Path,
    pub flags: FileFlags,
    pub pos: u64,

    ops: T,
}

pub trait FileOps: Send + Sync {
    /// 从文件的当前位置开始读取最多 `len` 字节的数据到用户空间地址 `user_buf` 中。
    ///
    /// 返回成功读取的字节数，若用户提供的缓冲区无效则返回 Err（即便已经读取了部分字节）。
    fn read(&mut self, mut user_buf: *mut u8, len: u64) -> VfsResult<u64> {
        self.kread(
            &mut |data| {
                copy_to_user(user_buf, data)?;
                user_buf = unsafe { user_buf.add(data.len()) };
                Ok(data.len() as u64)
            },
            len,
        )
    }
    /// 从文件的当前位置开始写入 `len` 字节的来自用户的数据。
    ///
    /// 返回成功写入的字节数，若用户提供的缓冲区无效则返回 Err（即便已经写入了部分字节）。
    fn write(&mut self, mut user_data: *const u8, len: u64) -> VfsResult<u64> {
        self.kwrite(
            &mut |buf| {
                copy_from_user_uninit(buf, user_data)?;
                user_data = unsafe { user_data.add(buf.len()) };
                Ok(buf.len() as u64)
            },
            len,
        )
    }

    /// 从文件的当前位置开始读取最多 `len` 字节的数据，数据会被传入 `visitor` 中。返回成功读取的字节数。
    fn kread(
        &mut self,
        visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64>;
    /// 从文件的当前位置开始写入最多 `len` 字节的数据，数据会通过 `visitor` 获取。返回成功写入的字节数。
    fn kwrite(
        &mut self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64>;

    /// 根据 `seek_from` 移动文件的当前位置。返回修改后的当前位置。
    fn fseek(&mut self, seek_from: SeekFrom) -> VfsResult<u64>;
    /// 将文件对应的 INode 的所有更改（无论是否来自此 File）同步到磁盘。
    fn fsync(&mut self) -> VfsResult<()>;

    /// 获取当前目录 `pos` 位置的对应项，不改变自己的 `pos`。
    fn get_dir(&self, pos: u64) -> VfsResult<Option<IterateDirData>>;

    fn poll(&self) -> VfsResult<PollStatus>;

    fn ioctl(&mut self, cmd: IoctlCommand, arg: *mut ()) -> VfsResult<usize> {
        Err(VfsError::OperationNotSupported)
    }
}
```

其读写接口分用户和内核两种。绝大部分情况下用户态读写都可以使用默认的实现转发到内核态读写，但少数只允许内核读写，或不允许读写因此希望内核态读写直接崩溃的场景下它们也会不同。

可以看到，用于遍历目录的接口也位于这一层，这是因为目录本质上也可以视作一种文件。

=== `PageCache`
此类型用于描述页缓存。`INode` 可能有也可能没有对应的页缓存。

页缓存使用 `xarray` 库中的 `XArray` 进行存储。这是一种专用于页缓存的数据结构，曾经被用在 Linux 的页缓存中，能高效地依据文件页号索引对应的 `PageMetaGuard`（#ref(<page.meta>, supplement: []) 节）。

`PageCache` 支持高效的添加、删除和查找操作，这为高效的文件读写和写回操作打下了坚实的基础。

=== `Mount`、`MountPoint`
这两个类型用于描述挂载。`MountPoint` 描述一个挂载点，是父文件目录的概念。一个目录，无论进行了多少次挂载，始终只会存在一个 `MountPoint`。`Mount` 则用于描述一次挂载，是子文件系统的概念。它们的定义如下：
```rust
pub struct MountPoint {
    parent: Path,
    mounts: RwLock<IntrusiveLinkedList<MountAdapter>>,
}

pub struct Mount {
    pub sub_fs: Pin<Arc<FileSystem>>,

    /// 挂载到的子文件系统目录项，注意不一定是根目录
    pub mount_to: DEntryArc,

    pub mount_point: Arc<MountPoint>,

    link: LinkedListLink,
}
```

可以看到，Ax 支持将非根目录挂载到其它文件系统，也支持将文件系统的某个目录挂载到自身#footnote[实际上，`/lib64` 和 `/usr/lib64` 就是 `/lib` 通过这种方式挂载的结果。]。

此外，一个 `MountPoint` 可以拥有多个 `Mount`，这是由多次挂载到同一目录实现的。然而，只有最后一个挂载会实际生效。只有最后一个挂载被解除后，原来的挂载才能重新显现出来。

=== `Path`
其定义如下：
```rust
pub struct Path(pub DEntryArc, pub Arc<Mount>);
```

可以看到，它实际就是 `DEntry` 和 `Mount` 的复合。

它在内核中被广泛用于描述一个路径。单纯的 `DEntry` 不足以做到这点，是因为一个 `DEntry` 有多种触达方式。例如，如果 `/a` 挂载到了 `/b`，那 `/a/c` 和 `/b/c` 就会对应一个 `DEntry`，但它们显然不是一个路径。为了 `..` 和其它回溯目录树的方法能够正确运作，`Path` 必须额外记录路径上的最后一个挂载。这样，前面的例子中，`/a/c` 记录的挂载对应挂载点 `/b`，而 `/b/c` 记录的挂载是该文件系统根目录对应的挂载，它们往回一级后，`/a/c` 变为 `/b`，VFS 发现它就是记录的挂载对应的目录，于是跨越该挂载点并对应修改记录的挂载，从而正确得到 `/a`。

=== 路径查找（Path Walk）
这一过程是由 VFS 完成的。首先，它接受一个枚举：
```rust
pub enum PathToWalk<'a> {
    Absolute(&'a ByteStr),
    Relative(&'a ByteStr, Path),
}
```

该枚举在类型层面分隔了绝对路径和相对路径。然后，VFS 会据此找到起始目录（对于绝对路径，就是根目录），并将绝对路径转换为相对路径。之后，VFS 会不断依据 `DEntry` 缓存和 `INode` 的 `lookup` 操作，一步步往下查找。

如果遇到挂载点，它会穿越挂载点并对应修改 `Path` 的 `Arc<Mount>`。

如果遇到符号链接，取决于参数，它可能会穿越符号链接。这通过递归路径查找实现。当然，它会依据设定的递归上限，在恰当的时候终止查找。

如果查找过程中遇到了负 `DEntry` 或不存在的条目，如果这不是路径的最后一项，查找会直接结束。反之，查找会以返回含负 `DEntry` 的 `Path` 结尾。

不难发现，在前面完备的 VFS 框架下，路径查找操作变得和具体文件系统完全无关。只要它提供了 `lookup` 函数，就允许进行路径查找。这也侧面反映了完备 VFS 在开发和未来扩展中能带来的好处。

== ext4 磁盘文件系统
ext4 是目前 Linux 主要使用的日志文件系统。Ax 也选择了它作为磁盘文件系统。

=== `ext4_rs` 库及对其的修改
Ax 对 ext4 的支持基于 `ext4_rs` 库，但在原库的基础上进行了极为深度的修改。

==== 非 UTF-8 文件名支持
`ext4_rs` 默认使用 Rust 的 `String` 和 `&str` 操纵字符串。然而，这两种类型只支持 UTF-8 字符串，但 ext4 文件系统支持用任意不含 `'\0'` 和 `'/'` 的字节串作为文件（夹）名。为解决这一问题，Ax 修改了 `ext4_rs` 库，将其所有 `String` 和 `&str` 分别修改为了 `ByteString` 和 `ByteStr`，以支持非 UTF-8 文件名，并与 VFS 层更加兼容。

==== 磁盘写入漏洞修复
原版库在写回文件内容时存在漏洞，有可能直接损坏整个文件系统。Ax 借助生成式大语言模型修复了这个漏洞。

==== 无需重复触发写回和读取的内部 API
原版库几乎会对每个文件操作触发一次磁盘读和磁盘写。若不加修改，VFS 层的各个缓存机制会彻底失效。因此 Ax 将库中的内部函数或函数的内部部分提取出来直接调用，让库在 `INode` 缓存的数据上进行文件操作，从而避免了多余的读写。

=== INode 缓存
为避免不必要的磁盘 I/O，Ax 的 ext4 实现在每个 `INode` 中附加了磁盘 `INode` 数据。只有在从磁盘中读取一个现有 `INode` 时会发生磁盘读取，其余时候都只在缓存的 `INode` 数据上进行操作，并只在写回时触发磁盘写入。

== tmpfs 内存文件系统
这一文件系统完全架构在内存中，重启会丢失其中的全部数据。但也因为无需与持久化存储交互，它的读写速度很快，因此适于存放临时数据。

由于 VFS 已经是一个几乎完整的内存文件系统了，tmpfs 实际要做的事并不多。例如，由于 VFS 的页缓存已经完成了所有的功能，tmpfs 唯一要做的事就是告诉 VFS 页缓存不存在时底层文件系统也没有更多数据。

=== 文件树管理
tmpfs 在目录 `INode` 中存有一个 `Vec<(ByteString, INodeArc)>`，表示该目录下除 `.` 和 `..` 外的所有目录项的名字及其对应 `INode`。其中的元素按其名字升序排列，以便依据文件名执行二分查找。

文件树使用有序数组而非树结构的原因是，Ax 支持 Linux `getdents64` 系统调用，这一调用要求内核对每个目录项返回一个整数，用于描述这个目录项在目录中的位置。应用程序可以利用 `lseek` 等系统调用将当前文件的迭代位置调整到这些位置上。这就要求文件树能使用某种 64 位整数进行索引。树结构几乎无法实现这点#footnote[节点的地址不可行，因为内核无法保证或验证用户传入的地址合法。]。此时，综合最优的结构便是有序数组，可以直接用数组下标作为索引，且能轻易检查出非法下标，从而避免非法访问。

`INode` 中的文件树和 VFS `DEntry` 树是平行关系。例如，如果某个文件暂时没有任何任务引用，且内核进行过内存回收，则可能存在某个 `INode` 没有对应的 `DEntry`。如果存在失败的查找，则也可能存在某个 `DEntry` 没有对应的 `INode`。

== 伪文件系统
这几种文件系统的主要目的是让应用程序能以统一的文件操作 API 访问数据。它们几乎不提供正常文件系统应当具备的功能。

=== devfs
这一文件系统提供了到机器连接的真实或虚拟的硬件设备的访问能力。

它只提供设备文件、文件夹和符号链接三种对象。它们已经足够描述所有的设备。
==== `/dev/urandom`
此设备提供了不可预测的随机数访问能力。其使用基于物理世界的不可预测性获取到的真随机数据作为初始随机种子，并借助加密安全的伪随机数生成引擎（CSPRNG）生成后续随机数，从而获取持续的不可预测随机数。

此设备在系统启动时，会采样多次 CPU 时钟刻，利用执行速率的不确定性，获得无法被预测的初始随机种子。随后，设备会在发生读取时使用基于 `ChaCha8` 算法的 CSPRNG 生成后续随机数返回。

此设备也支持写入。写入的数据会作为外部熵添加到随机数引擎中。Ax 通过将外部数据与现有随机数引擎生成的随机数据使用异或操作混合的方式，保证无论何时都不损失熵，且随机数引擎状态始终对外界不可见。

`execve` 系统调用时内核传递的辅助向量中，`AT_RANDOM` 对应的 16 字节随机数据就是内核用它生成的。

==== `/dev/tty`
此设备提供了对终端的访问能力。

从此设备读取时，内核会检查终端有无输入，有则读取它并将其放入缓存中，否则阻塞。缓存是行缓存机制，若读到回车，即便缓存未满也会返回内容。这样，程序能够从终端得到输入，进而实现人机交互。

向此设备写入时，内核会逐字节地将字符写入终端。

向终端写入和从终端读取的具体方法依平台而定，文件系统模块只调用硬件抽象层提供的接口。

==== `/dev/null`
此设备提供了一个始终为空的文件。

从此设备读取时，总是会立刻返回 0，表示没有任何内容。

向此设备写入时，写入总是能成功，但始终为无操作。

`/dev/null` 会默默吞噬所有输入，因此很适合用作表示“我不需要输出”的垃圾桶。

==== `/dev/zero`
此设备提供了一个拥有无数个 0 字节的文件。

从此设备读取时，读取总是成功，读取到的所有字节都是 0。实践中，它会直接将零页分批返回给调用方。

向此设备写入时，和 `/dev/null` 类似，始终是无操作。

`/dev/zero` 可以用于快速零填充数据，也可以像 `/dev/null` 一样用作垃圾桶，尽管没有那么直观。

=== procfs
这一文件系统提供了另一种访问进程或系统相关信息的渠道。整体上，它的速度比相关系统调用慢，但往往能提供更多更全面的信息。

它对用户侧只读，用户程序不能在其中创建对象，也不能重命名或删除其中的对象。其中的绝大多数文件也是只读的。

=== pipefs
pipefs 是所有管道（无论是否匿名）文件所在的文件系统，其在整个内核中只有一个实例。对于匿名管道，它提供了一个接口，用于新增一个管道并返回它的读端、写端文件。

无论管道有多少个读写端，同一个管道都共用同一个 pipefs `INode`，其中包括了这个管道的环形缓冲区，以及现存的读端、写端数目。

从读端读取时，如果环形缓冲区中存在数据，就会读取数据并返回，无论读取了多少。否则，如果当前管道不存在现存写端，读取操作会返回 0，表示管道已经结束。如果还存在写端，读取将会阻塞，内核会转而调度其它任务。

向写端写入时，如果当前管道不存在现存读端，写入操作会返回错误。否则，如果待写入数据量不多于于某个阈值（初赛版本是 4096 字节），内核保证写入是原子的，会一直等待到缓冲区的剩余空间至少有这么多后再一次性写入。如果数据量多余该阈值，内核会分多次写入。无论如何，所有数据写入完毕，或者读端全部关闭前，写入不会返回。

这一文件系统只是为了满足 VFS 形式上的要求而存在的，不支持任何文件系统操作。

=== eventpollfs
eventpollfs 是所有 epoll 文件所在的文件系统，其在整个内核中只有一个实例。

比 pipefs 更进一步的是，它只有一个 `INode`，所有文件共享一个全局 `INode`，且此 `INode` 没有任何实际意义。epoll 文件的所有数据都存储在对应的 `File` 结构体中。

这一文件系统只是为了满足 VFS 形式上的要求而存在的，不支持任何文件系统操作。
