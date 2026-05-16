use core::sync::atomic::AtomicU32;

use crate::{
    filesystem::vfs::{self, FileSystemOps, VfsResult},
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
}
