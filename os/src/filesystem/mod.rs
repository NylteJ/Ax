pub mod devfs;
pub mod ext4;
pub mod procfs;
pub mod tmpfs;
pub mod vfs;

pub mod filesystems;

mod other;
mod utils;

pub use other::epoll::EPollFile;
pub use other::pipe::new_anonymous_pipes;
pub use other::socket::SocketFile;

pub fn init() {
    tmpfs::register();
    ext4::register();
    devfs::register();
    procfs::register();

    other::pipe::init();
    other::epoll::init();
    other::socket::init();
}

pub fn init_root(
    fs_type: &filesystems::FileSystemType,
    block_device: &'static dyn crate::devices::block_device::BlockDevice,
) {
    let fs = fs_type.new_fs(Some(block_device));

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

static ANONYMOUS_BLOCK_DEVICE_ID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(65536);

fn get_new_anon_block_device_id() -> devfs::drivers::DeviceId {
    let minor = ANONYMOUS_BLOCK_DEVICE_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    devfs::drivers::DeviceId(0, minor)
}
