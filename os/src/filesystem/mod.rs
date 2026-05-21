pub mod devfs;
pub mod ext4;
pub mod tmpfs;
pub mod vfs;

pub mod filesystems;

mod utils;

pub fn register_all() {
    tmpfs::register();
    ext4::register();
    devfs::register();
}

pub fn init_root(fs_type: &filesystems::FileSystemType) {
    let fs = fs_type.ops.new_fs();

    let root = fs.root_dentry.clone();

    // 对于根目录所在的文件系统，其根目录直接挂载到自身
    let root_circle_mount = unsafe { vfs::MountPoint::mount_root(fs) };

    debug_assert!(root.data.lock().mount.is_some());

    unsafe {
        crate::basic::UnsafeInited::init_unsafe(
            &vfs::ROOT_PATH,
            vfs::Path(root, root_circle_mount),
        );
    }
}
