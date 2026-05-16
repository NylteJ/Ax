use core::{
    mem::{MaybeUninit, offset_of},
    pin::Pin,
    ptr::NonNull,
    sync::atomic::Ordering,
};

use alloc::{bstr::ByteString, sync::Arc};
use bitflags::bitflags;

use crate::{
    basic::sync::Mutex,
    filesystem::vfs::{INode, INodeDirtyState, Path, VfsError, VfsResult},
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

    pub ops: T,
}

bitflags! {
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
    fn read(&mut self, user_buf: *mut u8, len: u64) -> VfsResult<u64>;
    /// 从文件的当前位置开始写入 `len` 字节的来自用户的数据。
    ///
    /// 返回成功写入的字节数，若用户提供的缓冲区无效则返回 Err（即便已经写入了部分字节）。
    fn write(&mut self, user_data: *const u8, len: u64) -> VfsResult<u64>;

    /// 根据 `seek_from` 移动文件的当前位置。返回修改后的当前位置。
    fn fseek(&mut self, seek_from: SeekFrom) -> VfsResult<u64>;
    /// 将文件对应的 INode 的所有更改（无论是否来自此 File）同步到磁盘。
    fn fsync(&mut self) -> VfsResult<()>;

    /// 获取当前目录 `pos` 位置的对应项，不改变自己的 `pos`。
    ///
    /// 你可能更希望调用 `INodeData::iterate_dir`。
    fn get_dir(&self, pos: u64) -> VfsResult<Option<IterateDirData>>;
}

pub enum SeekFrom {
    Start(u64),
    Current(i64),
    End(i64),
}

pub struct IterateDirData {
    pub name: ByteString,
    pub next_pos: i64,
    pub inode_id: u32,
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
        drop(this);
        unsafe { ptr.as_mut() }
    }

    pub fn new(path: Path, inode: &INode, flags: FileFlags, ops: T) -> Pin<Arc<File>> {
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

impl<T: ?Sized + FileOps> FileData<T> {
    /// 从当前目录当前位置开始读取数据到内核态缓冲区 `k_buffer_uninit` 中，返回读取完毕部分的 slice 以及条目数。
    ///
    /// 其会以 `Dirent` 的格式写入数据。如果缓冲区小到读不完一条 `Dirent` 则返回 `Err`。因此，返回空 slice
    /// 则说明目录已被遍历完毕。
    ///
    /// 会增加 `self.pos`，但返回 Err 时不会，即便已经读取了部分目录项。
    ///
    /// 特别注意，返回 slice 会抛去缓冲区前部可能存在的不对齐填充，因此请始终使用返回值。
    pub fn iterate_dir<'a>(
        &mut self,
        k_buffer_uninit: &'a mut [MaybeUninit<u8>],
    ) -> VfsResult<(&'a mut [u8], u64)> {
        use crate::syscall::filesystem::DirentHead;

        const DIRENT_ALIGNMENT: usize = 8;

        let base_off = k_buffer_uninit.as_ptr().align_offset(DIRENT_ALIGNMENT);

        let mut k_buffer_rest = k_buffer_uninit.len();
        let mut k_buffer_ptr = k_buffer_uninit.as_mut_ptr();
        let mut count = 0;

        while let Some(IterateDirData {
            name,
            next_pos,
            inode_id,
        }) = self.ops.get_dir(self.pos + count)?
        {
            let dirent_size = size_of::<DirentHead>() + name.len() + 1; // 注意要带末尾的 '\0
            let start_off = k_buffer_ptr.align_offset(DIRENT_ALIGNMENT);
            let end_off = start_off + dirent_size;

            if end_off > k_buffer_rest {
                if count == 0 {
                    return Err(VfsError::InvalidArguments(
                        "提供的缓冲区太小，无法容纳一个目录项",
                    ));
                }

                break;
            }

            k_buffer_ptr = unsafe { k_buffer_ptr.add(start_off) };

            unsafe {
                k_buffer_ptr.cast::<DirentHead>().write(DirentHead {
                    d_ino: inode_id as u64,
                    d_off: next_pos,
                    d_reclen: dirent_size as core::ffi::c_ushort,
                    d_type: 0, // TODO
                })
            };
            k_buffer_ptr = unsafe { k_buffer_ptr.add(size_of::<DirentHead>()) };

            unsafe { k_buffer_ptr.copy_from_nonoverlapping(name.as_ptr().cast(), name.len()) };
            k_buffer_ptr = unsafe { k_buffer_ptr.add(name.len()) };

            unsafe { k_buffer_ptr.cast::<u8>().write(0) }; // 写入末尾的 '\0'
            k_buffer_ptr = unsafe { k_buffer_ptr.add(1) };

            k_buffer_rest -= end_off;
            count += 1;
        }

        if count == 0 {
            return Ok((&mut [], 0));
        }
        self.pos += count;
        let filled_len = k_buffer_uninit.len() - k_buffer_rest;
        Ok((
            unsafe { k_buffer_uninit[base_off..filled_len].assume_init_mut() },
            count,
        ))
    }

    /// 从文件的当前位置开始读取最多 `len` 字节的数据到用户空间地址 `user_buf` 中。
    ///
    /// 返回成功读取的字节数，若用户提供的缓冲区无效则返回 Err（即便已经读取了部分字节）。
    pub fn read(&mut self, user_buf: *mut u8, len: u64) -> VfsResult<u64> {
        self.ops.read(user_buf, len)
    }
    /// 从文件的当前位置开始写入 `len` 字节的来自用户的数据。
    ///
    /// 返回成功写入的字节数，若用户提供的缓冲区无效则返回 Err（即便已经写入了部分字节）。
    pub fn write(&mut self, user_data: *const u8, len: u64) -> VfsResult<u64> {
        let ret = self.ops.write(user_data, len)?;

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

        Ok(ret)
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
                .ok_or(VfsError::InvalidArguments("文件位置偏移量溢出"))?
        }
        SeekFrom::End(offset) => {
            let file_size = unsafe { inode.as_ref() }.data.read().file_size;
            *pos = file_size
                .checked_add_signed(offset)
                .ok_or(VfsError::InvalidArguments("文件位置偏移量溢出"))?
        }
    }
    Ok(*pos)
}
