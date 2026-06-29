use core::{mem::MaybeUninit, pin::Pin, ptr::addr_eq};

use alloc::{borrow::ToOwned, sync::Arc};
use hashbrown::HashMap;

use crate::{
    basic::{UnsafeInited, bstr::ByteStr},
    dummy_inode_body,
    filesystem::{
        get_new_anon_block_device_id,
        vfs::{self, FileOps, VfsError, VfsResult},
    },
    syscall::epoll::EPollEvent,
};

pub struct EPollFile {
    pub entries: HashMap<usize, EPollEvent>,
}

impl FileOps for EPollFile {
    fn kread(
        &mut self,
        _visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        _len: u64,
    ) -> VfsResult<u64> {
        unimplemented!()
    }

    fn kwrite(
        &mut self,
        _visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        _len: u64,
    ) -> VfsResult<u64> {
        unimplemented!()
    }

    fn read(&mut self, _user_buf: *mut u8, _len: u64) -> VfsResult<u64> {
        Err(VfsError::OperationNotSupported)
    }

    fn write(&mut self, _user_data: *const u8, _len: u64) -> VfsResult<u64> {
        Err(VfsError::OperationNotSupported)
    }

    fn fseek(&mut self, _seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        Err(VfsError::IsPipeOrSocket)
    }

    fn fsync(&mut self) -> VfsResult<()> {
        Ok(())
    }

    fn get_dir(&self, _pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        Err(VfsError::OperationNotSupported)
    }

    fn poll(&self) -> VfsResult<vfs::PollStatus> {
        todo!()
    }
}

impl EPollFile {
    pub fn new() -> vfs::FileArc {
        vfs::File::new(
            EPOLL_PATH.clone(),
            &EPOLL_INODE,
            vfs::FileFlags::Read | vfs::FileFlags::Write,
            Self {
                entries: HashMap::new(),
            },
        )
    }

    pub fn from_dyn_file(file: &vfs::File) -> Option<&vfs::File<Self>> {
        if addr_eq(file.get_inode(), &**EPOLL_INODE) {
            let file: *const vfs::File<dyn FileOps> = file;
            let file: *const vfs::File<Self> = file.cast();
            Some(unsafe { file.as_ref_unchecked() })
        } else {
            None
        }
    }
}

struct EPollFs;
impl vfs::FileSystemOps for EPollFs {
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
        _buffer: &mut [MaybeUninit<u8>; 4096],
    ) -> VfsResult<usize> {
        unimplemented!()
    }
}

struct EPollINode;
impl vfs::INodeOps for EPollINode {
    dummy_inode_body!();
}

struct EPollDEntry;
impl vfs::DEntryOps for EPollDEntry {
    fn open(&self, _mount: Arc<vfs::Mount>, _flags: vfs::FileFlags) -> VfsResult<vfs::FileArc> {
        unimplemented!()
    }
}

static EPOLL_FS: UnsafeInited<Pin<Arc<vfs::FileSystem<EPollFs>>>> =
    unsafe { UnsafeInited::uninit() };
static EPOLL_INODE: UnsafeInited<vfs::INodeArc> = unsafe { UnsafeInited::uninit() };
static EPOLL_PATH: UnsafeInited<vfs::Path> = unsafe { UnsafeInited::uninit() };

pub(in super::super) fn init() {
    let fs = vfs::FileSystem::new(
        |fs, _| {
            let inode = vfs::INode::new_raw(0, fs, vfs::INodeData {
                file_size: 0,
                hard_links: 1,
                mode: vfs::INodeMode::all_permitted(),
                page_cache: None,
                ops: EPollINode,
            });
            let dentry =
                vfs::DEntry::new_root(ByteStr::new(b"").to_owned(), inode.clone(), fs, EPollDEntry);

            unsafe { UnsafeInited::init_unsafe(&EPOLL_INODE, inode.clone()) };

            (inode, dentry)
        },
        get_new_anon_block_device_id(),
        EPollFs,
    );

    let mount = unsafe { vfs::MountPoint::mount_root(fs.clone()) };

    unsafe { UnsafeInited::init_unsafe(&EPOLL_PATH, vfs::Path(fs.root_dentry.clone(), mount)) };

    unsafe { UnsafeInited::init_unsafe(&EPOLL_FS, fs) };
}
