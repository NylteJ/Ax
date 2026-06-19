use crate::{
    basic::bstr::ByteString,
    filesystem::vfs::{self, INodeOps, VfsError, VfsResult},
    syscall::filesystem::{KstatInner, Time},
};

pub(super) enum ProcFsINode {
    MemInfo,
}

impl INodeOps for ProcFsINode {
    fn create(
        &mut self,
        _mode: vfs::INodeMode,
        _dentry: vfs::DEntryArc,
    ) -> VfsResult<vfs::INodeArc> {
        Err(VfsError::NotADirectory)
    }

    fn lookup(
        &self,
        _name: &crate::basic::bstr::ByteStr,
        _dentry: vfs::DEntryArc,
    ) -> VfsResult<vfs::DEntryArc> {
        Err(VfsError::NotADirectory)
    }

    fn link(&mut self, _dentry: vfs::DEntryArc, _target: &vfs::INodeArc) -> VfsResult<()> {
        Err(VfsError::NotADirectory)
    }

    fn unlink(&mut self, _dentry: &vfs::DEntryArc) -> VfsResult<()> {
        Err(VfsError::NotADirectory)
    }

    fn mkdir(
        &mut self,
        _dentry: vfs::DEntryArc,
        _mode: vfs::INodeMode,
    ) -> VfsResult<vfs::INodeArc> {
        Err(VfsError::NotADirectory)
    }

    fn rmdir(&mut self, _dentry: &vfs::DEntryArc) -> VfsResult<()> {
        Err(VfsError::NotADirectory)
    }

    fn rename(
        &mut self,
        _new_node: Option<&mut vfs::INodeData>,
        _old_name: &crate::basic::bstr::ByteStr,
        _new_name: ByteString,
        _flags: vfs::INodeRenameFlags,
    ) -> vfs::VfsResult<()> {
        Err(VfsError::NotADirectory)
    }

    fn symlink(
        &mut self,
        _target: ByteString,
        _dentry: vfs::DEntryArc,
    ) -> vfs::VfsResult<vfs::INodeArc> {
        Err(VfsError::NotADirectory)
    }

    fn read_symlink(&self) -> vfs::VfsResult<ByteString> {
        Err(VfsError::NotADirectory)
    }

    fn get_kstat(&self) -> vfs::VfsResult<KstatInner> {
        Ok(KstatInner {
            st_uid: 0,
            st_gid: 0,
            st_rdev: 0,
            st_size: 0,
            st_blksize: 0,
            st_blocks: 0,
            st_atime: Time::from_ns(0),
            st_mtime: Time::from_ns(0),
            st_ctime: Time::from_ns(0),
            ..Default::default()
        })
    }

    fn evictable_when_mem_ref_zero(&mut self) -> bool {
        false
    }
}
