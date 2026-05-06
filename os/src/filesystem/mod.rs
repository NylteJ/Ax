pub mod ext4;
pub mod tmpfs;
pub mod vfs;

pub mod filesystems;

mod utils;

pub fn init_all() {
    tmpfs::register();
}
