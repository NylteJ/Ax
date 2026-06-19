use core::{mem::MaybeUninit, sync::atomic::AtomicU32};

use crate::filesystem::vfs::{self, FileSystemOps, VfsError, VfsResult};

pub struct ProcFs {
    pub counter: AtomicU32,
}

impl FileSystemOps for ProcFs {
    fn writeback_page(
        &self,
        _inode: &vfs::INode,
        _page_index: u64,
        _page_data: &[u8; 4096],
    ) -> VfsResult<()> {
        debug_assert!(false, "procfs 不应该有页缓存");
        Err(VfsError::OperationNotSupported)
    }

    fn read_to_page(
        &self,
        _inode: &vfs::INode,
        _page_index: u64,
        _buffer: &mut [MaybeUninit<u8>; 4096],
    ) -> VfsResult<usize> {
        debug_assert!(false, "procfs 不应该有页缓存");
        Err(VfsError::OperationNotSupported)
    }
}
