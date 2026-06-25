use core::{mem::MaybeUninit, sync::atomic::AtomicU32};

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

    fn read_to_page(
        &self,
        _inode: &vfs::INode,
        _page_index: u64,
        buffer: &mut [MaybeUninit<u8>; PAGE_BYTES],
    ) -> VfsResult<usize> {
        // 正常情况下 TmpFs 不存在页缓冲未命中
        // 然而，如果通过 lseek 等方式，跳过中间部分直接延长文件长度，则可能导致此函数被调用
        // 此时写入全 0 即可，甚至不用检查文件大小
        buffer.fill(MaybeUninit::new(0));
        Ok(buffer.len())
    }
}
