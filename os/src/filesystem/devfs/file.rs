use core::mem::MaybeUninit;

use crate::filesystem::vfs::{self, FileOps, VfsError, VfsResult, default_fseek};

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
        todo!()
    }

    fn poll(&self) -> VfsResult<vfs::PollStatus> {
        Ok(vfs::PollStatus::regular_io_ready())
    }
}
