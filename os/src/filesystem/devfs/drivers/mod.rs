mod tty;

use core::mem::MaybeUninit;

use alloc::boxed::Box;
use hashbrown::HashMap;

use crate::{
    basic::UnsafeInited,
    filesystem::vfs::{self, VfsError, VfsResult},
    syscall::filesystem::IoctlCommand,
};

pub trait Driver: Send + Sync {
    fn kread(&self, visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>, len: u64) -> VfsResult<u64>;

    fn kwrite(
        &self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64>;

    fn ioctl(&self, cmd: IoctlCommand, arg: *mut ()) -> VfsResult<usize>;

    fn fsync(&self) -> VfsResult<()> {
        Ok(())
    }
}

impl<T: ?Sized + Driver> vfs::FileOps for &'static T {
    fn kread(
        &mut self,
        visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        (*self).kread(visitor, len)
    }

    fn kwrite(
        &mut self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        (*self).kwrite(visitor, len)
    }

    fn fseek(&mut self, seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        todo!()
    }

    fn fsync(&mut self) -> VfsResult<()> {
        (*self).fsync()
    }

    fn get_dir(&self, _pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        Err(VfsError::NotADirectory)
    }

    fn poll(&self) -> VfsResult<vfs::PollStatus> {
        todo!()
    }

    fn ioctl(&mut self, cmd: IoctlCommand, arg: *mut ()) -> VfsResult<usize> {
        (*self).ioctl(cmd, arg)
    }
}

/// (主设备号, 次设备号)
///
/// 注意块设备和字符设备使用两套 ID 空间
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub u16, pub u32);

impl DeviceId {
    pub fn to_u64(&self) -> u64 {
        ((self.0 as u64) << 20) | (self.1 as u64)
    }
    pub fn from_u64(raw: u64) -> Self {
        Self((raw >> 20) as u16, (raw & 0xFFFFF) as u32)
    }
}

pub enum DeviceType {
    Char,
    Block,
}

pub static CHAR_DEVICE_TYPES: UnsafeInited<HashMap<DeviceId, Box<dyn Driver>>> =
    unsafe { UnsafeInited::uninit() };
pub static BLOCK_DEVICE_TYPES: UnsafeInited<HashMap<DeviceId, Box<dyn Driver>>> =
    unsafe { UnsafeInited::uninit() };

pub fn find_driver(device_type: &DeviceType, device_id: &DeviceId) -> Option<&'static dyn Driver> {
    Some(match device_type {
        DeviceType::Char => &**CHAR_DEVICE_TYPES.get(device_id)?,
        DeviceType::Block => &**BLOCK_DEVICE_TYPES.get(device_id)?,
    })
}

pub fn init() {
    init_char_drivers();
    init_block_drivers();
}

macro_rules! define_driver {
    ($types:ident, $k:expr, $v:expr) => {{
        let k: DeviceId = $k;
        let v: Box<dyn Driver> = $v;
        debug_assert!(!$types.contains_key(&k));
        unsafe { $types.insert_unique_unchecked(k, v) }
    }};
}

pub const DEV_TTY: DeviceId = DeviceId(5, 0);

fn init_char_drivers() {
    let mut types = HashMap::new();

    macro_rules! define {
        ($($tt:tt)*) => {
            define_driver!(types, $($tt)*);
        };
    }

    define!(DEV_TTY, Box::new(tty::TtyDriver::new()));

    unsafe { UnsafeInited::init_unsafe(&CHAR_DEVICE_TYPES, types) };
}

fn init_block_drivers() {
    let types = HashMap::new();

    // TODO

    unsafe { UnsafeInited::init_unsafe(&BLOCK_DEVICE_TYPES, types) };
}
