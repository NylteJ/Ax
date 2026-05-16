use core::{bstr::ByteStr, pin::Pin};

use alloc::{borrow::ToOwned, boxed::Box, sync::Arc};

use crate::{
    devices::BLOCK_DEVICES,
    filesystem::{
        ext4::{dentry::Ext4DEntry, filesystem::Ext4Fs, inode::Ext4INode},
        filesystems::{FileSystemType, FileSystemTypeOps, register_filesystem_type},
        vfs,
    },
};

mod dentry;
mod file;
mod filesystem;
mod inode;

pub struct Ext4FsType;

impl FileSystemTypeOps for Ext4FsType {
    fn new_fs(&self) -> Pin<Arc<vfs::FileSystem>> {
        // TODO: 改为传入块设备引用可能会更好
        vfs::FileSystem::new(
            |fs, ext4| {
                let root_inode = Ext4INode::new_root(fs, &ext4.ext4);
                let init_root_dentry = vfs::DEntry::new_root(
                    ByteStr::new("/").to_owned(),
                    root_inode.clone(),
                    fs,
                    Ext4DEntry,
                );
                (root_inode, init_root_dentry)
            },
            Ext4Fs::new(BLOCK_DEVICES[0]),
        )
    }
}

pub fn register() {
    register_filesystem_type(Box::new(FileSystemType {
        name: "ext4",
        ops: Ext4FsType,
    }));
}
