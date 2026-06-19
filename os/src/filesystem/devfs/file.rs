use core::{any::Any, mem::MaybeUninit};

use crate::filesystem::{
    devfs::DevINode,
    vfs::{self, FileOps, VfsError, VfsResult, default_fseek},
};

pub struct DevDirFile;

impl FileOps for DevDirFile {
    fn kread(
        &mut self,
        _visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        _len: u64,
    ) -> VfsResult<u64> {
        Err(VfsError::IsADirectory)
    }

    fn kwrite(
        &mut self,
        _visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        _len: u64,
    ) -> VfsResult<u64> {
        Err(VfsError::IsADirectory)
    }

    fn fseek(&mut self, seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        default_fseek(self, seek_from)
    }

    fn fsync(&mut self) -> VfsResult<()> {
        Err(VfsError::IsADirectory)
    }

    fn get_dir(&self, pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        let this = vfs::File::from_dyn(self);

        let inode = this.inode;
        debug_assert!((&this.get_inode().data.read().ops as &dyn Any).is::<DevINode>());
        let inode = unsafe { inode.cast::<vfs::INode<DevINode>>().as_ref() };

        let inode_data_guard = inode.data.read();
        let DevINode::Directory(dir_vec) = &inode_data_guard.ops else {
            return Err(VfsError::NotADirectory);
        };

        Ok(dir_vec
            .get(pos as usize)
            .map(|(name, _)| vfs::IterateDirData {
                name: name.clone(),
                next_pos: pos + 1,
                inode_id: inode.id,
                entry_type: match inode.data.read().ops {
                    DevINode::DeviceFile(..) => vfs::IterateDirEntryType::RegularFile,
                    DevINode::Directory(..) => vfs::IterateDirEntryType::Directory,
                    DevINode::Symlink(..) => vfs::IterateDirEntryType::Symlink,
                },
            }))
    }

    fn poll(&self) -> VfsResult<vfs::PollStatus> {
        Ok(vfs::PollStatus::regular_io_ready())
    }
}
