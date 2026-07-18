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
        bstr::ByteStr,
        extend_lifetime,
        sync::{Mutex, MutexGuard},
    },
    dummy_filesystem_body, dummy_inode_body,
    filesystem::{
        get_new_anon_block_device_id,
        other::{DummyDEntryOps, DummyINodeOps},
        vfs::{self, VfsError, VfsResult},
    },
    multitask::{cur_task, signal::SignalId},
};

// ltp pipe11 要求缓冲区至少有 4096 字节
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
                cur_task().send_signal(SignalId::SIGPIPE);

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
                {
                    let buffer = pipe.buffer.lock();
                    if buffer.vacant_len() >= len {
                        break buffer;
                    }
                }
                check_pipe_alive(pipe)?;
                crate::syscall::basic::yield_(); // 假装睡眠
            };

            Ok(atomic_read(buffer, visitor, len)?.0)
        } else {
            loop {
                let buffer = loop {
                    {
                        let buffer = pipe.buffer.lock();
                        if buffer.vacant_len() > 0 {
                            break buffer;
                        }
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

    let inode = vfs::INode::new_uncached(
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
        DummyDEntryOps,
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
    dummy_filesystem_body!();
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
                ops: DummyINodeOps,
            });

            let root_dentry = vfs::DEntry::new_root(
                ByteStr::new("/(anon pipes)/").to_owned(),
                root_inode.clone(),
                fs,
                DummyDEntryOps,
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

#[derive(Default)]
struct PipeINode {
    buffer: Mutex<RingBuffer>,
    writer_count: AtomicU16,
    reader_count: AtomicU16,
}

impl vfs::INodeOps for PipeINode {
    dummy_inode_body!();
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
