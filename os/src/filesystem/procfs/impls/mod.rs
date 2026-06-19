pub mod meminfo;
mod simple;

#[macro_export]
macro_rules! impl_file {
    (for $typename: ty: kread: { $($kread: tt)* }, kwrite: { $($kwrite: tt)* }) => {
        impl $crate::filesystem::vfs::FileOps for $typename {
            $( $kread )*

            $( $kwrite )*

            fn fseek(&mut self, seek_from: $crate::filesystem::vfs::SeekFrom) -> $crate::filesystem::vfs::VfsResult<u64> {
                $crate::filesystem::vfs::default_fseek(self, seek_from)
            }

            fn fsync(&mut self) -> $crate::filesystem::vfs::VfsResult<()> {
                Ok(())
            }

            fn get_dir(&self, _pos: u64) -> $crate::filesystem::vfs::VfsResult<Option<$crate::filesystem::vfs::IterateDirData>> {
                Err($crate::filesystem::vfs::VfsError::NotADirectory)
            }

            fn poll(&self) -> $crate::filesystem::vfs::VfsResult<$crate::filesystem::vfs::PollStatus> {
                Ok($crate::filesystem::vfs::PollStatus::regular_io_ready())
            }
        }
    };
}
