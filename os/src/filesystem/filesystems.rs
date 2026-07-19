use core::pin::Pin;

use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec::Vec,
};

use crate::{
    basic::{bstr::ByteStr, sync::SpinRwLock},
    devices::block_device::BlockDevice,
    filesystem::vfs::FileSystem,
};

pub struct FileSystemType<T: ?Sized + FileSystemTypeOps = dyn FileSystemTypeOps> {
    pub name: &'static ByteStr,
    pub(super) ops: T,
}

pub trait FileSystemTypeOps: Send + Sync {
    fn new_fs(&self, block_device: Option<&'static dyn BlockDevice>) -> Pin<Arc<FileSystem>>;
}

// (因为这里是相当冷的冷路径，我们选择方便但较为浪费资源的写法)
static FILESYSTEMS: SpinRwLock<Vec<Weak<FileSystem>>> = SpinRwLock::new(Vec::new());

impl<T: ?Sized + FileSystemTypeOps> FileSystemType<T> {
    pub fn new_fs(&self, block_device: Option<&'static dyn BlockDevice>) -> Pin<Arc<FileSystem>> {
        let fs = self.ops.new_fs(block_device);

        let fs_arc = unsafe { Pin::into_inner_unchecked(fs) };

        FILESYSTEMS.write().push(Arc::downgrade(&fs_arc));

        unsafe { Pin::new_unchecked(fs_arc) }
    }
}

pub fn foreach_filesystem(mut f: impl FnMut(&FileSystem)) {
    FILESYSTEMS
        .read()
        .iter()
        .filter_map(|weak| weak.upgrade())
        .for_each(|arc| f(&arc));
}

static FILE_SYSTEM_TYPES: SpinRwLock<Vec<&'static FileSystemType>> = SpinRwLock::new(Vec::new());

pub fn get_filesystem_type(name: &ByteStr) -> Option<&'static FileSystemType> {
    let types = FILE_SYSTEM_TYPES.read();
    types.iter().find(|t| t.name == name).copied()
}

pub fn register_filesystem_type(fs_type: Box<FileSystemType>) {
    let mut types = FILE_SYSTEM_TYPES.write();
    types.push(Box::leak(fs_type));
}
