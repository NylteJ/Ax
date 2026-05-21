use crate::filesystem::{
    devfs::drivers::Driver,
    vfs::{self, FileOps, VfsError, VfsResult, default_fseek},
};

pub enum DevFile {
    Directory,
    Device(&'static Driver),
}

impl FileOps for DevFile {
    fn read(&mut self, user_buf: *mut u8, len: u64) -> VfsResult<u64> {
        match self {
            DevFile::Directory => Err(VfsError::IsADirectory),
            DevFile::Device(driver) => (driver.read)(user_buf, len),
        }
    }

    fn write(&mut self, user_data: *const u8, len: u64) -> VfsResult<u64> {
        match self {
            DevFile::Directory => Err(VfsError::IsADirectory),
            DevFile::Device(driver) => (driver.write)(user_data, len),
        }
    }

    fn fseek(&mut self, seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        default_fseek(self, seek_from)
    }

    fn fsync(&mut self) -> VfsResult<()> {
        match self {
            DevFile::Directory => Err(VfsError::IsADirectory),
            DevFile::Device(driver) => (driver.fsync)(),
        }
    }

    fn get_dir(&self, pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        let Self::Directory = self else {
            return Err(VfsError::NotADirectory);
        };

        todo!()
    }
}
