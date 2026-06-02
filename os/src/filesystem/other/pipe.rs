use core::{
    mem::MaybeUninit,
    pin::Pin,
    ptr::NonNull,
    sync::atomic::{AtomicU32, Ordering},
};

use alloc::borrow::ToOwned;
use ringbuf::{
    Arc,
    traits::{Consumer as C2, Observer, Producer as P2, Split},
};
use uninit::extension_traits::MaybeUninitExt;

use crate::{
    basic::{
        UnsafeInited,
        bstr::{ByteStr, ByteString},
        extend_lifetime,
    },
    filesystem::{
        get_new_anon_block_device_id,
        vfs::{self, VfsError, VfsResult},
    },
};

const BUFFER_BYTES: usize = 4096;

type RingBuffer = ringbuf::StaticRb<u8, BUFFER_BYTES>;

type Producer = <RingBuffer as ringbuf::traits::Split>::Prod;
type Consumer = <RingBuffer as ringbuf::traits::Split>::Cons;

pub enum PipeFile {
    Reader(Consumer),
    Writer(Producer),
}

// SAFETY: FileOps 的所有操作均要求提供可变引用，因此访问总是独占的
unsafe impl Send for PipeFile {}
unsafe impl Sync for PipeFile {}

impl vfs::FileOps for PipeFile {
    fn kread(
        &mut self,
        visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        let Self::Reader(reader) = self else {
            return Err(VfsError::BadPipeEnd);
        };

        while reader.is_empty() {
            if !reader.write_is_held() {
                return Ok(0);
            }
            crate::syscall::basic::yield_(); // 假装睡眠
        }

        let (first_slice, second_slice) = reader.occupied_slices();

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
        unsafe { reader.advance_read_index(full_count as usize) };

        debug_assert_ne!(full_count, 0);
        Ok(full_count)
    }

    fn kwrite(
        &mut self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        let Self::Writer(writer) = self else {
            return Err(VfsError::BadPipeEnd);
        };

        fn check_pipe_alive(writer: &Producer) -> VfsResult<()> {
            if !writer.read_is_held() {
                log::error!("TODO: SIGPIPE");
                Err(VfsError::BrokenPipe)
            } else {
                Ok(())
            }
        }

        check_pipe_alive(writer)?;

        let len_required = len;
        let mut len = len as usize;

        let atomic = len <= BUFFER_BYTES;
        if atomic {
            while writer.vacant_len() < len {
                check_pipe_alive(writer)?;
                crate::syscall::basic::yield_(); // 假装睡眠
            }
        }

        loop {
            let (first_slice, second_slice) = writer.vacant_slices_mut();

            let first_count = len.min(first_slice.len());
            let first_actual = visitor(&mut first_slice[..first_count])?;

            let second_count = (len - first_count).min(second_slice.len());
            let second_actual = if first_actual == first_count as u64 {
                visitor(&mut second_slice[..second_count])?
            } else {
                0
            };

            let full_count = (first_actual + second_actual) as usize;

            unsafe { writer.advance_write_index(full_count) };
            len -= full_count;

            if len == 0 || second_actual != second_count as u64 || second_count == 0 {
                break;
            }

            debug_assert!(!atomic);
            while writer.vacant_len() == 0 {
                check_pipe_alive(writer)?;
                crate::syscall::basic::yield_(); // 假装睡眠
            }
        }

        Ok(len_required - len as u64)
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
        fn poll_impl(observer: &impl Observer) -> vfs::PollStatus {
            // TODO: 注意，我们自己实现了 Send 和 Sync，但此处只接收不可变引用，因此我们必须自行保证不发生并发访问
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

        Ok(match self {
            PipeFile::Reader(reader) => {
                let base = if reader.write_is_held() {
                    vfs::PollStatus::empty()
                } else {
                    vfs::PollStatus::POLLHUP
                };
                base | poll_impl(reader)
            }
            PipeFile::Writer(writer) => {
                let base = if writer.read_is_held() {
                    vfs::PollStatus::empty()
                } else {
                    vfs::PollStatus::POLLERR
                };
                base | poll_impl(writer)
            }
        })
    }
}

impl PipeFile {
    /// 创建一个匿名管道，返回 (读端, 写端)
    pub fn new_anonymous() -> (vfs::FileArc, vfs::FileArc) {
        let ring_buffer = RingBuffer::default();

        let (writer, reader) = ring_buffer.split();

        let fs = NonNull::from_ref(&**PIPE_FS);

        let inode = vfs::INode::new_raw(
            PIPE_FS.ops.inode_id.fetch_add(1, Ordering::Relaxed),
            fs,
            vfs::INodeData {
                file_size: 0,
                hard_links: 0,
                mode: vfs::INodeMode::all_permitted() | vfs::INodeMode::Pipe,
                page_cache: None,
                ops: PipeINode,
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

        let reader = vfs::File::new(
            path.clone(),
            inode_ref,
            vfs::FileFlags::Read,
            Self::Reader(reader),
        );
        let writer = vfs::File::new(path, inode_ref, vfs::FileFlags::Write, Self::Writer(writer));

        (reader, writer)
    }
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
                ops: PipeINode,
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

struct PipeINode;

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

    fn get_kstat(&self) -> VfsResult<crate::syscall::filesystem::KstatInner> {
        unimplemented!()
    }

    fn evictable_when_mem_ref_zero(&mut self) -> bool {
        true
    }
}

struct PipeDEntry;

impl vfs::DEntryOps for PipeDEntry {
    fn open(&self, _mount: Arc<vfs::Mount>, _flags: vfs::FileFlags) -> VfsResult<vfs::FileArc> {
        unimplemented!()
    }
}
