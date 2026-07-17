use core::{
    mem::{MaybeUninit, offset_of},
    pin::Pin,
    ptr::NonNull,
    sync::atomic::Ordering,
};

use alloc::sync::Arc;
use bitflags::bitflags;
use uninit::extension_traits::MaybeUninitExt;

use crate::{
    basic::{bstr::ByteString, extend_lifetime, sync::Mutex},
    filesystem::vfs::{INode, INodeDirtyState, Path, VfsError, VfsResult},
    memory::{
        PAGE_BYTES,
        uaccess::{copy_from_user_uninit, copy_to_user},
    },
    syscall::filesystem::IoctlCommand,
};

/// 描述打开的文件，也许类似句柄的概念？
pub struct File<T: ?Sized + FileOps = dyn FileOps> {
    /// 指向对应 INode 的指针，此值自创建后便不会改变。其有效性由 DEntry 的 Arc 间接保证。
    pub inode: NonNull<INode>,
    pub data: Mutex<FileData<T>>,
}

unsafe impl<T: ?Sized + FileOps> Send for File<T> where Mutex<FileData<T>>: Send + Sync {}
unsafe impl<T: ?Sized + FileOps> Sync for File<T> where Mutex<FileData<T>>: Send + Sync {}

pub struct FileData<T: ?Sized + FileOps = dyn FileOps> {
    pub path: Path,
    pub flags: FileFlags,
    pub pos: u64,

    ops: T,
}

bitflags! {
    #[derive(Clone, Copy)]
    pub struct FileFlags: u8 {
        const Read = 1 << 0;
        const Write = 1 << 1;
        const Append = 1 << 2;
    }
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
    ///
    /// 你可能更希望调用 `INodeData::iterate_dir`。
    fn get_dir(&self, pos: u64) -> VfsResult<Option<IterateDirData>>;

    fn poll(&self) -> VfsResult<PollStatus>;

    fn ioctl(&mut self, cmd: IoctlCommand, arg: *mut ()) -> VfsResult<usize> {
        Err(VfsError::OperationNotSupported)
    }
}

pub enum SeekFrom {
    Start(u64),
    Current(i64),
    End(i64),
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct PollStatus: i16 {
        /// 有数据可读
        const POLLIN = 1 << 0;
        /// 有紧急数据可读（例如，TCP socket 上的带外数据；处于数据包模式的伪终端主设备侧从设备侧状态发生变化）
        const POLLPRI = 1 << 1;
        /// 现在写入不会阻塞
        const POLLOUT = 1 << 2;
        /// 错误条件（仅输出）
        const POLLERR = 1 << 3;
        /// 挂起（仅输出）
        const POLLHUP = 1 << 4;
        /// 无效请求：fd 未打开（仅输出）
        const POLLNVAL = 1 << 5;
        /// 等同于 POLLIN
        const POLLRDNORM = 1 << 6;
        /// 可读取优先级带数据（在 Linux 上通常未使用）
        const POLLRDBAND = 1 << 7;
        /// 等同于 POLLOUT
        const POLLWRNORM = 1 << 8;
        /// 可写入优先级数据
        const POLLWRBAND = 1 << 9;
        /// 不使用
        const POLLMSG = 1 << 10;
        /// 流式 socket 的对端关闭了连接，或关闭了连接写入那一半。
        ///
        /// Linux 扩展。
        const POLLRDHUP = 1 << 12;

        const Readable = Self::POLLIN.bits() | Self::POLLRDNORM.bits();
        const Writable = Self::POLLOUT.bits() | Self::POLLWRNORM.bits();
    }
}

impl PollStatus {
    /// 通常文件的可读可写状态（可读、可写，没有紧急数据）
    pub fn regular_io_ready() -> Self {
        Self::Readable | Self::Writable
    }
}

#[derive(Debug, Clone, Copy)]
pub enum IterateDirEntryType {
    RegularFile,
    Directory,
    Symlink,
    Unknown,
}

#[derive(Debug)]
pub struct IterateDirData {
    pub name: ByteString,
    pub next_pos: u64,
    pub inode_id: u32,
    pub entry_type: IterateDirEntryType,
}

impl<T: FileOps + 'static> File<T> {
    pub fn from_dyn(this: &T) -> &File<T> {
        unsafe {
            Mutex::from_value(
                NonNull::from_ref(this)
                    .byte_sub(offset_of!(FileData<T>, ops))
                    .cast::<FileData<T>>(),
            )
            .byte_sub(offset_of!(File<T>, data))
            .cast()
            .as_ref()
        }
    }

    pub fn from_dyn_mut(this: &mut T) -> &mut File<T> {
        let mut ptr = NonNull::from_ref(Self::from_dyn(this));
        #[expect(dropping_references)]
        drop(this);
        unsafe { ptr.as_mut() }
    }

    pub fn new(path: Path, inode: &INode, flags: FileFlags, ops: T) -> FileArc {
        Arc::pin(File {
            inode: NonNull::from_ref(inode),
            data: Mutex::new(FileData {
                path,
                flags,
                pos: 0,
                ops,
            }),
        })
    }
}

pub type FileArc<T = dyn FileOps> = Pin<Arc<File<T>>>;

impl<T: ?Sized + FileOps> FileData<T> {
    /// 从当前目录当前位置开始读取数据到内核态缓冲区 `k_buffer_uninit` 中，返回读取完毕部分的 slice 以及条目数。
    ///
    /// 其会以 `Dirent` 的格式写入数据。如果缓冲区小到读不完一条 `Dirent` 则返回 `Err`。因此，返回空 slice
    /// 则说明目录已被遍历完毕。
    ///
    /// 会增加 `self.pos`，但返回 Err 时不会，即便已经读取了部分目录项。
    pub fn iterate_dir<'a>(
        &mut self,
        k_buffer_uninit: &'a mut [MaybeUninit<u8>],
    ) -> VfsResult<(&'a mut [u8], u64)> {
        use crate::syscall::filesystem::DirentHead;

        const DIRENT_ALIGNMENT: usize = 8;

        debug_assert!(k_buffer_uninit.as_ptr().is_aligned_to(DIRENT_ALIGNMENT));

        let mut k_buffer_rest = k_buffer_uninit.len();
        let mut k_buffer_ptr = k_buffer_uninit.as_mut_ptr();
        let mut count = 0;
        let mut pos = self.pos;

        while let Some(IterateDirData {
            name,
            next_pos,
            inode_id,
            entry_type,
        }) = self.ops.get_dir(pos)?
        {
            debug_assert!(k_buffer_ptr.is_aligned_to(DIRENT_ALIGNMENT));

            let dirent_size_unaligned = size_of::<DirentHead>() + name.len() + 1; // 注意要带末尾的 '\0'
            let dirent_size = dirent_size_unaligned.next_multiple_of(DIRENT_ALIGNMENT);

            if dirent_size > k_buffer_rest {
                if count == 0 {
                    return Err(VfsError::InvalidArguments(
                        "提供的缓冲区太小，无法容纳一个目录项",
                    ));
                }

                break;
            }

            let d_type = match entry_type {
                IterateDirEntryType::RegularFile => 8,
                IterateDirEntryType::Directory => 4,
                IterateDirEntryType::Symlink => 10,
                IterateDirEntryType::Unknown => 0,
            };

            unsafe {
                k_buffer_ptr.cast::<DirentHead>().write(DirentHead {
                    d_ino: inode_id as u64,
                    d_off: next_pos as i64,
                    d_reclen: dirent_size as core::ffi::c_ushort,
                    d_type,
                })
            };
            k_buffer_ptr = unsafe { k_buffer_ptr.add(size_of::<DirentHead>()) };

            unsafe { k_buffer_ptr.copy_from_nonoverlapping(name.as_ptr().cast(), name.len()) };
            k_buffer_ptr = unsafe { k_buffer_ptr.add(name.len()) };

            unsafe { k_buffer_ptr.cast::<u8>().write(0) }; // 写入末尾的 '\0'
            k_buffer_ptr = unsafe { k_buffer_ptr.add(1) };

            debug_assert_eq!(
                k_buffer_ptr.align_offset(DIRENT_ALIGNMENT),
                dirent_size - dirent_size_unaligned
            );
            k_buffer_ptr = unsafe { k_buffer_ptr.add(dirent_size - dirent_size_unaligned) };

            k_buffer_rest -= dirent_size;
            count += 1;
            pos = next_pos;
        }

        if count == 0 {
            return Ok((&mut [], 0));
        }
        self.pos = pos;
        let filled_len = k_buffer_uninit.len() - k_buffer_rest;
        Ok((
            unsafe { k_buffer_uninit[..filled_len].assume_init_by_mut() },
            count,
        ))
    }

    /// 从文件的当前位置开始读取最多 `len` 字节的数据到用户空间地址 `user_buf` 中。
    ///
    /// 返回成功读取的字节数，若用户提供的缓冲区无效则返回 Err（即便已经读取了部分字节）。
    pub fn read(&mut self, user_buf: *mut u8, len: u64) -> VfsResult<u64> {
        if !self.flags.contains(FileFlags::Read) {
            return Err(VfsError::BadFileOpenFlags);
        }

        self.ops.read(user_buf, len)
    }
    /// 从文件的当前位置开始写入 `len` 字节的来自用户的数据。
    ///
    /// 返回成功写入的字节数，若用户提供的缓冲区无效则返回 Err（即便已经写入了部分字节）。
    pub fn write(&mut self, user_data: *const u8, len: u64) -> VfsResult<u64> {
        if !self.flags.contains(FileFlags::Write) {
            return Err(VfsError::BadFileOpenFlags);
        }

        let ret = self.ops.write(user_data, len)?;
        self.mark_inode_dirty();
        Ok(ret)
    }

    /// 从文件的当前位置开始读取最多 `len` 字节的数据，数据会被传入 `visitor` 中。返回成功读取的字节数。
    pub fn kread(
        &mut self,
        mut visitor: impl FnMut(&[u8]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        self.ops.kread(&mut visitor, len)
    }
    /// 从文件的当前位置开始写入最多 `len` 字节的数据，数据会通过 `visitor` 获取。返回成功写入的字节数。
    pub fn kwrite(
        &mut self,
        mut visitor: impl FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        let ret = self.ops.kwrite(&mut visitor, len)?;
        self.mark_inode_dirty();
        Ok(ret)
    }

    fn mark_inode_dirty(&self) {
        let inode = self.path.0.data.lock().inode.as_ref().unwrap().clone();
        let prev = inode
            .dirty_state
            .fetch_or(INodeDirtyState::Dirty.bits(), Ordering::Relaxed);
        if INodeDirtyState::from_bits_retain(prev).is_pure_clean() {
            unsafe {
                inode
                    .fs
                    .as_ref()
                    .dirty_sent
                    .lock()
                    .lru_add(&inode, offset_of!(INode, dirty))
            };
            log::trace!(
                "INode {} ({:#x}) 变脏，已加入脏页链表",
                inode.id,
                inode.addr()
            );
        }
    }

    pub fn fseek(&mut self, seek_from: SeekFrom) -> VfsResult<u64> {
        self.ops.fseek(seek_from)
    }

    pub fn fsync(&mut self) -> VfsResult<()> {
        self.ops.fsync()
    }

    pub fn poll(&self) -> VfsResult<PollStatus> {
        self.ops.poll()
    }

    pub fn ioctl(&mut self, cmd: IoctlCommand, arg: *mut ()) -> VfsResult<usize> {
        self.ops.ioctl(cmd, arg)
    }

    /// 获取 ops。注意 ops 上的函数并不为直接调用设计，随意使用它有可能违反内存安全。
    pub unsafe fn get_ops(&mut self) -> &mut T {
        &mut self.ops
    }
}

pub fn default_fseek<T: FileOps + 'static>(this: &mut T, seek_from: SeekFrom) -> VfsResult<u64> {
    let inode = File::from_dyn(this).inode;
    let pos = &mut File::from_dyn_mut(this).data.get_mut().pos;
    match seek_from {
        SeekFrom::Start(new_pos) => *pos = new_pos,
        SeekFrom::Current(offset) => {
            *pos = pos
                .checked_add_signed(offset)
                .ok_or(VfsError::ArithmeticOverflow)?
        }
        SeekFrom::End(offset) => {
            let file_size = unsafe { inode.as_ref() }.data.read().file_size;
            *pos = file_size
                .checked_add_signed(offset)
                .ok_or(VfsError::ArithmeticOverflow)?
        }
    }
    Ok(*pos)
}

impl File {
    /// 获取文件指定页偏移处的页缓存页，注意文件末尾页可能只有前半截
    ///
    /// 若对应页暂不位于页缓存中，则从文件中读取，过程中可能发生 I/O
    pub unsafe fn get_page_cache(inode: &INode, page_index: u64) -> VfsResult<&[u8]> {
        let inode_data_rguard = inode.data.read();
        let page_cache = inode_data_rguard
            .page_cache
            .as_ref()
            .expect("读取的文件必须有页缓存");

        let bytes;

        let page = match page_cache.get(page_index) {
            Some(cache_page) => {
                bytes = PAGE_BYTES
                    .min(inode_data_rguard.file_size as usize - page_index as usize * PAGE_BYTES);

                cache_page
            }
            None => {
                let fs = unsafe { inode.fs.as_ref() };
                let page_guard = page_cache.alloc_without_insert(page_index);

                bytes = fs
                    .ops
                    .read_to_page(&inode, page_index, unsafe { page_guard.visit_memory_mut() })?;

                page_cache.insert(page_index, page_guard)
            }
        };

        Ok(unsafe { extend_lifetime(page.visit_memory()[..bytes].assume_init_by_ref()) })
    }

    pub fn mark_page_cache_dirty(inode: &INode, page_index: u64) {
        let inode_data_rguard = inode.data.read();
        let page_cache = inode_data_rguard.page_cache.as_ref().expect("必须有页缓存");

        let page = page_cache.get(page_index).expect("必须有对应页");

        unsafe { page.pagecache_mark_dirty(page_cache) };
    }
}

impl<T: ?Sized + FileOps> File<T> {
    pub fn get_inode(&self) -> &INode {
        unsafe { self.inode.as_ref() }
    }
}
