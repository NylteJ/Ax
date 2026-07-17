use core::sync::atomic::AtomicU32;

use crate::{dummy_filesystem_body, filesystem::vfs::FileSystemOps};

pub struct ProcFs {
    pub counter: AtomicU32,
}

impl FileSystemOps for ProcFs {
    dummy_filesystem_body!();
}
