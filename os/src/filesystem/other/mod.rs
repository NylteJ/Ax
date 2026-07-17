pub mod epoll;
pub mod pipe;
pub mod socket;

struct DummyINodeOps;

impl super::vfs::INodeOps for DummyINodeOps {
    crate::dummy_inode_body!();
}

struct DummyDEntryOps;

impl super::vfs::DEntryOps for DummyDEntryOps {
    fn open(
        &self,
        mount: alloc::sync::Arc<super::vfs::Mount>,
        flags: super::vfs::FileFlags,
    ) -> super::vfs::VfsResult<super::vfs::FileArc> {
        unimplemented!()
    }
}
