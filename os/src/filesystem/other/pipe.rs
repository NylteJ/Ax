use core::{
    mem::{MaybeUninit, offset_of},
    pin::Pin,
    ptr::NonNull,
    sync::atomic::{AtomicU16, AtomicU32, Ordering},
};

use alloc::borrow::ToOwned;
use ringbuf::{
    Arc,
    traits::{Consumer, Observer, Producer},
};
use uninit::extension_traits::MaybeUninitExt;

use crate::{
    basic::{
        UnsafeInited,
        bstr::{ByteStr, ByteString},
        extend_lifetime,
        sync::{Mutex, MutexGuard},
    },
    filesystem::{
        get_new_anon_block_device_id,
        vfs::{self, VfsError, VfsResult},
    },
    memory::PAGE_BYTES,
    multitask::{current_task, signal::SignalId},
    syscall::filesystem::{KstatInner, Time},
};

const BUFFER_BYTES: usize = 4096;

type RingBuffer = ringbuf::LocalRb<ringbuf::storage::Array<u8, BUFFER_BYTES>>;

pub struct PipeReadFile;
pub struct PipeWriteFile;

fn get_pipe(inode_op: &(impl vfs::FileOps + 'static)) -> &PipeINode {
    let this = vfs::File::from_dyn(inode_op);
    debug_assert!((&this.get_inode().data.read().ops as &dyn core::any::Any).is::<PipeINode>());
    let inode = unsafe { this.inode.cast::<vfs::INode<PipeINode>>().as_ref() };
    PipeINode::get_dyn(inode)
}

fn poll_impl(observer: &impl Observer) -> vfs::PollStatus {
    let mut ret = vfs::PollStatus::empty();

    let vacant_len = observer.vacant_len();

    if vacant_len != 0 {
        ret |= vfs::PollStatus::Writable;
    }

    debug_assert_eq!(BUFFER_BYTES, observer.capacity().get());
    if vacant_len != BUFFER_BYTES {
        ret |= vfs::PollStatus::Readable;
    }

    ret
}

impl vfs::FileOps for PipeReadFile {
    fn kread(
        &mut self,
        visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        let pipe = get_pipe(self);

        let buffer = loop {
            {
                let buffer = pipe.buffer.lock();
                if !buffer.is_empty() {
                    break buffer;
                }
            }

            if pipe.writer_count.load(Ordering::Acquire) == 0 {
                return Ok(0);
            }

            crate::syscall::basic::yield_(); // 假装睡眠
        };

        let (first_slice, second_slice) = buffer.occupied_slices();

        let first_count = (len as usize).min(first_slice.len());
        let first_src = unsafe { first_slice[..first_count].assume_init_by_ref() };
        let first_actual = visitor(first_src)?;

        let second_actual = if first_actual == first_count as u64 {
            let second_count = (len as usize - first_count).min(second_slice.len());
            let second_src = unsafe { second_slice[..second_count].assume_init_by_ref() };
            visitor(second_src)?
        } else {
            0
        };

        let full_count = first_actual + second_actual;

        // 由于存储的类型是字节，这里直接削减长度即可
        static_assertions::assert_not_impl_any!(u8: Drop);
        unsafe { buffer.advance_read_index(full_count as usize) };

        debug_assert_ne!(full_count, 0);
        Ok(full_count)
    }

    fn kwrite(
        &mut self,
        _visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        _len: u64,
    ) -> VfsResult<u64> {
        Err(VfsError::BadPipeEnd)
    }

    fn fseek(&mut self, _seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        Err(VfsError::IsPipeOrSocket)
    }

    fn fsync(&mut self) -> VfsResult<()> {
        Ok(())
    }

    fn get_dir(&self, _pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        Err(VfsError::NotADirectory)
    }

    fn poll(&self) -> VfsResult<vfs::PollStatus> {
        let pipe = get_pipe(self);

        let base = if pipe.writer_count.load(Ordering::Acquire) == 0 {
            vfs::PollStatus::POLLHUP
        } else {
            vfs::PollStatus::empty()
        };
        Ok(base | poll_impl(&*pipe.buffer.lock()))
    }
}

impl Drop for PipeReadFile {
    fn drop(&mut self) {
        let pipe = get_pipe(self);

        pipe.reader_count.fetch_sub(1, Ordering::Release);
    }
}

impl vfs::FileOps for PipeWriteFile {
    fn kread(
        &mut self,
        _visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        _len: u64,
    ) -> VfsResult<u64> {
        Err(VfsError::BadPipeEnd)
    }

    fn kwrite(
        &mut self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        fn check_pipe_alive(pipe: &PipeINode) -> VfsResult<()> {
            if pipe.writer_count.load(Ordering::Acquire) == 0 {
                // 此时还需要发送 SIGPIPE
                let cur_task = unsafe { current_task().as_ref_unchecked() };
                cur_task.send_signal(SignalId::SIGPIPE);

                Err(VfsError::BrokenPipe)
            } else {
                Ok(())
            }
        }

        fn atomic_read(
            mut buffer: MutexGuard<RingBuffer>,
            visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
            len: usize,
        ) -> VfsResult<(u64, bool)> {
            let (first_slice, second_slice) = buffer.vacant_slices_mut();

            let first_count = len.min(first_slice.len());
            let first_actual = visitor(&mut first_slice[..first_count])?;

            let second_count = (len - first_count).min(second_slice.len());
            let second_actual = if first_actual == first_count as u64 {
                visitor(&mut second_slice[..second_count])?
            } else {
                0
            };

            let full_count = (first_actual + second_actual) as usize;

            unsafe { buffer.advance_write_index(full_count) };

            Ok((
                full_count as u64,
                second_actual != second_count as u64 || second_count == 0,
            ))
        }

        let pipe = get_pipe(self);

        check_pipe_alive(pipe)?;

        let len_required = len;
        let mut len = len as usize;

        let atomic = len <= BUFFER_BYTES;
        if atomic {
            let buffer = loop {
                let buffer = pipe.buffer.lock();
                if buffer.vacant_len() >= len {
                    break buffer;
                }
                check_pipe_alive(pipe)?;
                crate::syscall::basic::yield_(); // 假装睡眠
            };

            Ok(atomic_read(buffer, visitor, len)?.0)
        } else {
            loop {
                let buffer = loop {
                    let buffer = pipe.buffer.lock();
                    if buffer.vacant_len() > 0 {
                        break buffer;
                    }
                    check_pipe_alive(pipe)?;
                    crate::syscall::basic::yield_(); // 假装睡眠
                };

                let (full_count, visitor_broken) = atomic_read(buffer, visitor, len)?;

                len -= full_count as usize;

                if len == 0 || visitor_broken {
                    break;
                }
            }

            Ok(len_required - len as u64)
        }
    }

    fn fseek(&mut self, _seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        Err(VfsError::IsPipeOrSocket)
    }

    fn fsync(&mut self) -> VfsResult<()> {
        Ok(())
    }

    fn get_dir(&self, _pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        Err(VfsError::NotADirectory)
    }

    fn poll(&self) -> VfsResult<vfs::PollStatus> {
        let pipe = get_pipe(self);

        let base = if pipe.reader_count.load(Ordering::Acquire) == 0 {
            vfs::PollStatus::POLLERR
        } else {
            vfs::PollStatus::empty()
        };
        Ok(base | poll_impl(&*pipe.buffer.lock()))
    }
}

impl Drop for PipeWriteFile {
    fn drop(&mut self) {
        let pipe = get_pipe(self);

        pipe.writer_count.fetch_sub(1, Ordering::Release);
    }
}

/// 创建一个匿名管道，返回 (读端, 写端)
pub fn new_anonymous_pipes() -> (vfs::FileArc, vfs::FileArc) {
    let fs = NonNull::from_ref(&**PIPE_FS);

    let inode = vfs::INode::new_raw(
        PIPE_FS.ops.inode_id.fetch_add(1, Ordering::Relaxed),
        fs,
        vfs::INodeData {
            file_size: 0,
            hard_links: 1,
            mode: vfs::INodeMode::all_permitted() | vfs::INodeMode::Pipe,
            page_cache: None,
            ops: PipeINode {
                buffer: Mutex::default(),
                writer_count: AtomicU16::new(1),
                reader_count: AtomicU16::new(1),
            },
        },
    );
    let inode_ref = unsafe { extend_lifetime(&*inode) };

    let dentry = vfs::DEntry::new_root(
        ByteStr::new("(anonymous pipe)").to_owned(),
        inode,
        fs,
        PipeDEntry,
    );
    let path = vfs::Path(dentry, PIPE_FS_MOUNT.clone());

    let reader = vfs::File::new(path.clone(), inode_ref, vfs::FileFlags::Read, PipeReadFile);
    let writer = vfs::File::new(path, inode_ref, vfs::FileFlags::Write, PipeWriteFile);

    (reader, writer)
}

struct PipeFs {
    inode_id: AtomicU32,
}

impl vfs::FileSystemOps for PipeFs {
    fn writeback_page(
        &self,
        _inode: &vfs::INode,
        _page_index: u64,
        _page_data: &[u8; 4096],
    ) -> VfsResult<()> {
        unimplemented!()
    }

    fn read_to_page(
        &self,
        _inode: &vfs::INode,
        _page_index: u64,
        _buffer: &mut [core::mem::MaybeUninit<u8>; 4096],
    ) -> VfsResult<usize> {
        unimplemented!()
    }
}

static PIPE_FS: UnsafeInited<Pin<Arc<vfs::FileSystem<PipeFs>>>> = unsafe { UnsafeInited::uninit() };
static PIPE_FS_MOUNT: UnsafeInited<Arc<vfs::Mount>> = unsafe { UnsafeInited::uninit() };

pub(in super::super) fn init() {
    let fs = vfs::FileSystem::new(
        |fs, _| {
            let root_inode = vfs::INode::new_raw(0, fs, vfs::INodeData {
                file_size: 0,
                hard_links: 2,
                mode: vfs::INodeMode::all_permitted(),
                page_cache: None,
                ops: PipeINode::default(),
            });

            let root_dentry = vfs::DEntry::new_root(
                ByteStr::new("/(anon pipes)/").to_owned(),
                root_inode.clone(),
                fs,
                PipeDEntry,
            );

            (root_inode, root_dentry)
        },
        get_new_anon_block_device_id(),
        PipeFs {
            inode_id: AtomicU32::new(1),
        },
    );

    let mount = unsafe { vfs::MountPoint::mount_root(fs.clone()) };

    unsafe { UnsafeInited::init_unsafe(&PIPE_FS, fs) };
    unsafe { UnsafeInited::init_unsafe(&PIPE_FS_MOUNT, mount) };
}

// dummy 根节点和普通管道（无论具名还是匿名）共用，因为 dummy 根节点只有一个，浪费并不大 (FIXME?)
#[derive(Default)]
struct PipeINode {
    buffer: Mutex<RingBuffer>,
    writer_count: AtomicU16,
    reader_count: AtomicU16,
}

impl vfs::INodeOps for PipeINode {
    fn create(
        &mut self,
        _mode: vfs::INodeMode,
        _dentry: vfs::DEntryArc,
    ) -> VfsResult<vfs::INodeArc> {
        unimplemented!()
    }

    fn lookup(&self, _name: &ByteStr, _dentry: vfs::DEntryArc) -> VfsResult<vfs::DEntryArc> {
        unimplemented!()
    }

    fn link(&mut self, _dentry: vfs::DEntryArc, _target: &vfs::INodeArc) -> VfsResult<()> {
        unimplemented!()
    }

    fn unlink(&mut self, _dentry: &vfs::DEntryArc) -> VfsResult<()> {
        unimplemented!()
    }

    fn mkdir(
        &mut self,
        _dentry: vfs::DEntryArc,
        _mode: vfs::INodeMode,
    ) -> VfsResult<vfs::INodeArc> {
        unimplemented!()
    }

    fn rmdir(&mut self, _dentry: &vfs::DEntryArc) -> VfsResult<()> {
        unimplemented!()
    }

    fn rename(
        &mut self,
        _new_node: Option<&mut vfs::INodeData>,
        _old_name: &ByteStr,
        _new_name: ByteString,
        _flags: vfs::INodeRenameFlags,
    ) -> VfsResult<()> {
        unimplemented!()
    }

    fn symlink(
        &mut self,
        _target: ByteString,
        _dentry: vfs::DEntryArc,
    ) -> VfsResult<vfs::INodeArc> {
        unimplemented!()
    }

    fn read_symlink(&self) -> VfsResult<ByteString> {
        unreachable!("不可能是符号链接，此请求在 VFS 层就应被拦截")
    }

    fn get_kstat(&self) -> VfsResult<KstatInner> {
        Ok(KstatInner {
            st_uid: 0, // FIXME
            st_gid: 0, // FIXME
            st_rdev: 0,
            st_size: 0,
            st_blksize: PAGE_BYTES as u32,
            st_blocks: 0,
            st_atime: Time::from_sec(0), // FIXME
            st_mtime: Time::from_sec(0), // FIXME
            st_ctime: Time::from_sec(0), // FIXME
            ..Default::default()
        })
    }

    fn evictable_when_mem_ref_zero(&mut self) -> bool {
        true
    }
}

impl PipeINode {
    fn get_dyn(outer: &vfs::INode<Self>) -> &Self {
        // 绕过碍事的外层 RwLock
        static_assertions::assert_impl_all!(PipeINode: Send, Sync);
        unsafe {
            outer
                .data
                .as_ptr()
                .byte_add(offset_of!(vfs::INodeData<Self>, ops))
                .cast::<Self>()
                .as_ref_unchecked()
        }
    }
}

struct PipeDEntry;

impl vfs::DEntryOps for PipeDEntry {
    fn open(&self, _mount: Arc<vfs::Mount>, _flags: vfs::FileFlags) -> VfsResult<vfs::FileArc> {
        unimplemented!()
    }
}
