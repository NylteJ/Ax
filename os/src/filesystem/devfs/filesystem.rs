use core::sync::atomic::AtomicU32;

use crate::{dummy_filesystem_body, filesystem::vfs::FileSystemOps};

pub struct DevFs {
    pub(super) inode_id: AtomicU32,
}

impl FileSystemOps for DevFs {
    dummy_filesystem_body!();
}
