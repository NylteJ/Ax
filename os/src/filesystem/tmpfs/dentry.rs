use core::{ops::Deref, pin::Pin};

use alloc::sync::Arc;

use crate::filesystem::{
    tmpfs::TmpFsFile,
    vfs::{self, DEntryOps, FileFlags, Mount, Path, VfsResult},
};

pub struct TmpFsDentry {}

impl DEntryOps for TmpFsDentry {
    fn open(&self, mount: Arc<Mount>, flags: FileFlags) -> VfsResult<Pin<Arc<vfs::File>>> {
        let data = vfs::DEntryData::get_data(self);
        let inode = data
            .inode
            .as_ref()
            .expect("VFS 层永远不应在负 DEntry 上调用 open")
            .deref();

        let dentry: &vfs::DEntry = vfs::DEntry::<TmpFsDentry>::get_dentry(self);
        let dentry = unsafe { vfs::get_varc_from_value(dentry) };

        Ok(vfs::File::new(
            Path(dentry, mount),
            inode,
            flags,
            TmpFsFile {},
        ))
    }
}
