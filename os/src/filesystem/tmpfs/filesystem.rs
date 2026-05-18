use core::sync::atomic::AtomicU32;

use crate::{
    filesystem::vfs::{self, FileSystemOps, VfsError, VfsResult},
    memory::PAGE_BYTES,
};

pub struct TmpFs {
    pub(super) inode_id: AtomicU32,
}

impl FileSystemOps for TmpFs {
    fn writeback_page(
        &self,
        _inode: &vfs::INode,
        _page_index: u64,
        _page_data: &[u8; PAGE_BYTES],
    ) -> VfsResult<()> {
        Ok(())
    }

    fn read_to_page(
        &self,
        _inode: &vfs::INode,
        _page_index: u64,
        _buffer: &mut [core::mem::MaybeUninit<u8>; PAGE_BYTES],
    ) -> VfsResult<usize> {
        debug_assert!(false, "TmpFS 永远不应该有页缓存未命中");
        Err(VfsError::OperationNotSupported)
    }
}
