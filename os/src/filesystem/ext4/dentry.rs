use core::{ops::Deref, pin::Pin};

use alloc::sync::Arc;

use crate::filesystem::{
    ext4::file::Ext4File,
    vfs::{self, DEntryOps, VfsResult},
};

pub struct Ext4DEntry;

impl DEntryOps for Ext4DEntry {
    fn open(
        &self,
        mount: Arc<vfs::Mount>,
        flags: vfs::FileFlags,
    ) -> VfsResult<Pin<Arc<vfs::File>>> {
        let inode = vfs::DEntryData::get_data(self)
            .inode
            .as_ref()
            .expect("VFS 层永远不应在负 DEntry 上调用 open")
            .deref();

        let dentry: &vfs::DEntry = vfs::DEntry::<Self>::get_dentry(self);
        let dentry = unsafe { vfs::get_varc_from_value(dentry) };

        Ok(vfs::File::new(
            vfs::Path(dentry, mount),
            inode,
            flags,
            Ext4File,
        ))
    }
}
