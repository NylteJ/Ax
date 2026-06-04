use alloc::sync::Arc;

use crate::filesystem::{
    devfs::{drivers::find_driver, file::DevDirFile, inode::DevINode},
    vfs::{self, DEntryOps, VfsError, VfsResult},
};

pub struct DevDEntry;

impl DEntryOps for DevDEntry {
    fn open(&self, mount: Arc<vfs::Mount>, flags: vfs::FileFlags) -> VfsResult<vfs::FileArc> {
        let inode = vfs::DEntryData::get_data(self)
            .inode
            .as_ref()
            .expect("VFS 层应当保证不 open 负 DEntry")
            .downcast_ref::<DevINode>()
            .expect("VFS 层应当保证不传递非 DevINode");

        let dentry: &vfs::DEntry = vfs::DEntry::<Self>::get_dentry(self);
        let dentry = unsafe { vfs::get_varc_from_value(dentry) };

        let path = vfs::Path(dentry, mount);

        let file = match &inode.data.read().ops {
            DevINode::Directory(_) => vfs::File::new(path, inode, flags, DevDirFile),
            DevINode::DeviceFile(device_type, device_id) => {
                let driver = find_driver(device_type, device_id).ok_or(VfsError::DeviceNotFound)?;
                vfs::File::new(path, inode, flags, driver)
            }
            DevINode::Symlink(x) => *x,
        };

        Ok(file)
    }
}
