use core::sync::atomic::AtomicU32;

use crate::filesystem::vfs::FileSystemOps;

pub struct TmpFs {
    pub(super) inode_id: AtomicU32,
}

impl FileSystemOps for TmpFs {}
