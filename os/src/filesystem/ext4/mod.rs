use core::pin::Pin;

use alloc::{borrow::ToOwned, boxed::Box, sync::Arc};

use crate::{
    basic::bstr::ByteStr,
    devices::block_device::BlockDevice,
    filesystem::{
        devfs::drivers::DeviceId,
        ext4::{dentry::Ext4DEntry, filesystem::Ext4Fs, inode::Ext4INode},
        filesystems::{FileSystemType, FileSystemTypeOps, register_filesystem_type},
        get_new_anon_block_device_id, vfs,
    },
};

mod dentry;
mod file;
mod filesystem;
mod inode;

pub struct Ext4FsType;

impl FileSystemTypeOps for Ext4FsType {
    fn new_fs(&self, block_device: Option<&'static dyn BlockDevice>) -> Pin<Arc<vfs::FileSystem>> {
        let block_device = block_device.expect("Ext4 文件系统必须与某个块设备绑定");

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
            DeviceId(8, get_new_anon_block_device_id().1), // FIXME
            Ext4Fs::new(block_device),
        )
    }
}

pub fn register() {
    register_filesystem_type(Box::new(FileSystemType {
        name: "ext4",
        ops: Ext4FsType,
    }));
}
