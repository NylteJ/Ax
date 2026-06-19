use alloc::sync::Arc;

use crate::filesystem::{
    procfs::{impls::meminfo::MemInfoFile, inode::ProcFsINode},
    vfs::{self, DEntryOps, VfsResult},
};

pub struct ProcFsDentry;

impl DEntryOps for ProcFsDentry {
    fn open(
        &self,
        mount: Arc<vfs::Mount>,
        flags: vfs::FileFlags,
    ) -> VfsResult<crate::filesystem::vfs::FileArc> {
        let inode = vfs::DEntryData::get_data(self)
            .inode
            .as_ref()
            .expect("VFS 层永远不应在负 DEntry 上调用 open")
            .downcast_ref::<ProcFsINode>()
            .expect("VFS 层永远不应在错误的 inode 类型上调用 open");

        let dentry: &vfs::DEntry = vfs::DEntry::<Self>::get_dentry(self);
        let dentry = unsafe { vfs::get_varc_from_value(dentry) };

        Ok(vfs::File::new(
            vfs::Path(dentry, mount),
            inode,
            flags,
            match inode.data.read().ops {
                ProcFsINode::MemInfo => MemInfoFile,
            },
        ))
    }
}
