use core::pin::Pin;

use alloc::{boxed::Box, sync::Arc, vec::Vec};

use crate::{
    basic::{bstr::ByteStr, sync::RwLock},
    devices::block_device::BlockDevice,
    filesystem::vfs::FileSystem,
};

pub struct FileSystemType<T: ?Sized + FileSystemTypeOps = dyn FileSystemTypeOps> {
    pub name: &'static ByteStr,
    pub ops: T,
}

pub trait FileSystemTypeOps: Send + Sync {
    fn new_fs(&self, block_device: Option<&'static dyn BlockDevice>) -> Pin<Arc<FileSystem>>;
}

static FILE_SYSTEM_TYPES: RwLock<Vec<&'static FileSystemType>> = RwLock::new(Vec::new());

pub fn get_filesystem_type(name: &ByteStr) -> Option<&'static FileSystemType> {
    let types = FILE_SYSTEM_TYPES.read();
    types.iter().find(|t| t.name == name).copied()
}

pub fn register_filesystem_type(fs_type: Box<FileSystemType>) {
    let mut types = FILE_SYSTEM_TYPES.write();
    types.push(Box::leak(fs_type));
}
