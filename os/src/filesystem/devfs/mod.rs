//! 设备文件系统（devfs）
//!
//! 对应 `/dev` 目录，存储各个设备对应的设备文件。
//!
//! 我们对设备文件进行了很大的简化：
//! 1. devfs 不支持子目录、常规文件、命名管道、Socket、硬链接 等，也不支持 rename 等操作
//! 2. 其它文件系统不支持创建设备文件
//! 3. devfs 永远只能创建一份，即 `/dev`
//!
//! 对我们这类小型内核而言，这已经很够用了。
pub mod dentry;
pub mod file;
pub mod filesystem;
pub mod inode;

pub mod drivers;

use core::{pin::Pin, sync::atomic::AtomicU32};

use alloc::{borrow::ToOwned, boxed::Box, sync::Arc, vec::Vec};

use crate::{
    basic::bstr::ByteStr,
    devices::block_device::BlockDevice,
    filesystem::{
        devfs::{dentry::DevDEntry, filesystem::DevFs, inode::DevINode},
        filesystems::{FileSystemType, FileSystemTypeOps, register_filesystem_type},
        vfs,
    },
};

struct DevFsType;

impl FileSystemTypeOps for DevFsType {
    fn new_fs(&self, block_device: Option<&'static dyn BlockDevice>) -> Pin<Arc<vfs::FileSystem>> {
        debug_assert!(block_device.is_none(), "devfs 不应该绑定块设备");
        vfs::FileSystem::new(
            |fs, _| {
                let root_inode = vfs::INode::new_raw(0, fs, vfs::INodeData {
                    file_size: 0,
                    hard_links: 2,
                    mode: vfs::INodeMode::all_permitted(), // FIXME
                    page_cache: None,
                    ops: DevINode::Directory(Vec::new()),
                });
                let root_dentry = vfs::DEntry::new_root(
                    ByteStr::new("/(dev root)/").to_owned(),
                    root_inode.clone(),
                    fs,
                    DevDEntry,
                );
                (root_inode, root_dentry)
            },
            DevFs {
                inode_id: AtomicU32::new(1),
            },
        )
    }
}

pub fn register() {
    register_filesystem_type(Box::new(FileSystemType {
        name: "devfs",
        ops: DevFsType,
    }));
}
