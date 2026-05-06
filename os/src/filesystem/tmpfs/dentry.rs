use core::{ops::Deref, pin::Pin};

use alloc::sync::Arc;

use crate::filesystem::{
    tmpfs::TmpFsFile,
    vfs::{self, DEntryArc, DEntryOps, FileFlags, VfsError, VfsResult},
};

pub struct TmpFsDentry {}

impl DEntryOps for TmpFsDentry {
    fn open(&self, flags: FileFlags) -> VfsResult<Pin<Arc<vfs::File>>> {
        let data = vfs::DEntryData::get_data(self);
        let inode = data
            .inode
            .as_ref()
            .ok_or(VfsError::InvalidArguments("无法在负 DEntry 上打开文件"))?
            .deref();
        let dentry =
            unsafe { DEntryArc::from_exists(vfs::DEntry::<TmpFsDentry>::get_dentry(self)) };
        Ok(vfs::File::new(dentry, inode, flags, TmpFsFile {}))
    }
}
