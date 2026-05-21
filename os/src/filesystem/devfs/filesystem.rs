use core::sync::atomic::AtomicU32;

use crate::{
    filesystem::vfs::{self, FileSystemOps, VfsError, VfsResult},
    memory::PAGE_BYTES,
};

pub struct DevFs {
    pub(super) inode_id: AtomicU32,
}

impl FileSystemOps for DevFs {
    fn writeback_page(
        &self,
        _inode: &vfs::INode,
        _page_index: u64,
        _page_data: &[u8; PAGE_BYTES],
    ) -> VfsResult<()> {
        debug_assert!(false, "DevFS 不应该有页缓存");
        Err(VfsError::OperationNotSupported)
    }

    fn read_to_page(
        &self,
        _inode: &vfs::INode,
        _page_index: u64,
        _buffer: &mut [core::mem::MaybeUninit<u8>; PAGE_BYTES],
    ) -> VfsResult<usize> {
        debug_assert!(false, "DevFS 不应该有页缓存");
        Err(VfsError::OperationNotSupported)
    }
}
