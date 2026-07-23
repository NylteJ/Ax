use core::{
    mem::offset_of,
    pin::Pin,
    ptr::NonNull,
    sync::atomic::{AtomicU32, Ordering},
};

use alloc::{borrow::ToOwned, sync::Arc};

use crate::{
    basic::{UnsafeInited, bstr::ByteStr, extend_lifetime},
    dummy_filesystem_body, dummy_inode_body,
    filesystem::{
        get_new_anon_block_device_id,
        other::{DummyDEntryOps, DummyINodeOps},
        vfs::{self, VfsError, VfsResult},
    },
    network::socket::Socket,
};

pub struct SocketINode {
    /// 套接字，注意和 pipe 中的情况类似，我们总是绕过外部锁
    socket: Socket,
}

impl vfs::INodeOps for SocketINode {
    dummy_inode_body!();
}

pub struct SocketFile {}

impl vfs::FileOps for SocketFile {
    fn kread(
        &mut self,
        visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        todo!()
    }

    fn kwrite(
        &mut self,
        visitor: &mut dyn FnMut(&mut [core::mem::MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        todo!()
    }

    fn read(&mut self, _user_buf: *mut u8, _len: u64) -> VfsResult<u64> {
        unreachable!("外层会转发到 recv")
    }

    fn write(&mut self, _user_data: *const u8, _len: u64) -> VfsResult<u64> {
        unreachable!("外层会转发到 send")
    }

    fn fseek(&mut self, seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        Err(VfsError::IsPipeOrSocket)
    }

    fn fsync(&mut self) -> VfsResult<()> {
        Err(VfsError::InvalidArguments(
            "对 Socket 调用 fsync 是无意义的（系统调用应当返回 EINVAL）",
        ))
    }

    fn get_dir(&self, pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        Err(VfsError::NotADirectory)
    }

    fn poll(&self) -> VfsResult<vfs::PollStatus> {
        todo!()
    }
}

impl SocketFile {
    pub fn new(socket: Socket) -> vfs::FileArc {
        let fs = NonNull::from_ref(&**SOCKET_FS);

        let inode = vfs::INode::new_uncached(
            SOCKET_FS.ops.inode_id.fetch_add(1, Ordering::Relaxed),
            fs,
            vfs::INodeData {
                file_size: 0,
                hard_links: 1,
                mode: vfs::INodeMode::all_permitted() | vfs::INodeMode::Socket,
                page_cache: None,
                ops: SocketINode { socket },
            },
        );
        let inode_ref = unsafe { extend_lifetime(&*inode) };

        let dentry = vfs::DEntry::new_root(
            ByteStr::new("(socket)").to_owned(),
            inode,
            fs,
            DummyDEntryOps,
        );
        let path = vfs::Path(dentry, SOCKET_FS_MOUNT.clone());

        vfs::File::new(
            path,
            inode_ref,
            vfs::FileFlags::Read | vfs::FileFlags::Write,
            SocketFile {},
        )
    }

    pub fn from_dyn_file(file: &vfs::File) -> Option<&vfs::File<Self>> {
        if file.get_inode().fs.addr() == NonNull::from_ref(&**SOCKET_FS).addr() {
            let file: *const vfs::File<dyn vfs::FileOps> = file;
            let file: *const vfs::File<Self> = file.cast();
            Some(unsafe { file.as_ref_unchecked() })
        } else {
            None
        }
    }

    pub fn get_socket(file: &vfs::File<Self>) -> &Socket {
        static_assertions::assert_impl_all!(SocketINode: Send, Sync);

        unsafe {
            let inode = file.inode.cast::<vfs::INode<SocketINode>>().as_ref();

            inode
                .data
                .as_ptr()
                .byte_add(offset_of!(vfs::INodeData<SocketINode>, ops))
                .byte_add(offset_of!(SocketINode, socket))
                .cast::<Socket>()
                .as_ref_unchecked()
        }
    }
}

struct SocketFs {
    inode_id: AtomicU32,
}

impl vfs::FileSystemOps for SocketFs {
    dummy_filesystem_body!();
}

static SOCKET_FS: UnsafeInited<Pin<Arc<vfs::FileSystem<SocketFs>>>> =
    unsafe { UnsafeInited::uninit() };
static SOCKET_FS_MOUNT: UnsafeInited<Arc<vfs::Mount>> = unsafe { UnsafeInited::uninit() };

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
                ByteStr::new("/(sockets)/").to_owned(),
                root_inode.clone(),
                fs,
                DummyDEntryOps,
            );

            (root_inode, root_dentry)
        },
        get_new_anon_block_device_id(),
        SocketFs {
            inode_id: AtomicU32::new(1),
        },
    );

    let mount = unsafe { vfs::MountPoint::mount_root(fs.clone()) };

    unsafe { UnsafeInited::init_unsafe(&SOCKET_FS, fs) };
    unsafe { UnsafeInited::init_unsafe(&SOCKET_FS_MOUNT, mount) };
}
