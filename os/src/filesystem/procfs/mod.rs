use core::{pin::Pin, sync::atomic::AtomicU32};

use alloc::{borrow::ToOwned, boxed::Box, sync::Arc, vec::Vec};

use crate::{
    basic::bstr::ByteStr,
    devices::block_device::BlockDevice,
    filesystem::{
        filesystems::{FileSystemType, FileSystemTypeOps, register_filesystem_type},
        get_new_anon_block_device_id,
        procfs::{
            dir::{ProcFsDirDEntry, ProcFsDirINode},
            filesystem::ProcFs,
        },
        vfs,
    },
};

mod dentry;
mod dir;
mod filesystem;
mod impls;
mod inode;

pub struct ProcFsType;

impl FileSystemTypeOps for ProcFsType {
    fn new_fs(&self, block_device: Option<&'static dyn BlockDevice>) -> Pin<Arc<vfs::FileSystem>> {
        debug_assert!(block_device.is_none(), "procfs 不应该绑定块设备");
        vfs::FileSystem::new(
            |fs, _| {
                let root_inode = vfs::INode::new_raw(0, fs, vfs::INodeData {
                    file_size: 0,
                    hard_links: 2,
                    mode: vfs::INodeMode::all_permitted(),
                    page_cache: None,
                    ops: ProcFsDirINode { data: Vec::new() },
                });
                let root_dentry = vfs::DEntry::new_root(
                    ByteStr::new("/(proc root)/").to_owned(),
                    root_inode.clone(),
                    fs,
                    ProcFsDirDEntry,
                );
                (root_inode, root_dentry)
            },
            get_new_anon_block_device_id(),
            ProcFs {
                counter: AtomicU32::new(1),
            },
        )
    }
}

pub fn register() {
    register_filesystem_type(Box::new(FileSystemType {
        name: ByteStr::new("procfs"),
        ops: ProcFsType,
    }));
}
