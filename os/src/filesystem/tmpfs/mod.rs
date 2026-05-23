pub mod dentry;
pub mod file;
pub mod filesystem;
pub mod inode;

use core::{pin::Pin, sync::atomic::AtomicU32};

use alloc::{borrow::ToOwned, boxed::Box, sync::Arc};

use crate::{
    basic::bstr::ByteStr,
    filesystem::{
        filesystems::{FileSystemType, FileSystemTypeOps, register_filesystem_type},
        vfs::{self, DEntry},
    },
};

pub use dentry::*;
pub use file::*;
pub use filesystem::*;
pub use inode::*;

struct TmpFsType {}

impl FileSystemTypeOps for TmpFsType {
    fn new_fs(&self) -> Pin<Arc<vfs::FileSystem>> {
        vfs::FileSystem::new(
            |fs, _| {
                let root_inode = TmpFsINode::new_root(fs);
                let init_root_dentry = DEntry::new_root(
                    ByteStr::new("/").to_owned(),
                    root_inode.clone(),
                    fs,
                    TmpFsDentry {},
                );
                (root_inode, init_root_dentry)
            },
            TmpFs {
                inode_id: AtomicU32::new(1),
            },
        )
    }
}

pub fn register() {
    register_filesystem_type(Box::new(FileSystemType {
        name: "tmpfs",
        ops: TmpFsType {},
    }));
}
