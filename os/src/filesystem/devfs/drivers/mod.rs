use core::mem::MaybeUninit;

use hashbrown::HashMap;

use crate::{
    basic::{
        UnsafeInited,
        sbi::{getchar_u8, putchar_u8},
    },
    filesystem::vfs::{VfsError, VfsResult},
    memory::uaccess::{copy_from_user_uninit, copy_to_user},
};

/// 驱动，支持多态，不支持自定义数据
pub struct Driver {
    pub read: fn(user_buf: *mut u8, len: u64) -> VfsResult<u64>,
    pub write: fn(user_data: *const u8, len: u64) -> VfsResult<u64>,
    pub fsync: fn() -> VfsResult<()>,
}

/// (主设备号, 次设备号)
///
/// 注意块设备和字符设备使用两套 ID 空间
pub type DeviceId = (u32, u32);

pub enum DeviceType {
    Char,
    Block,
}

pub static CHAR_DEVICE_TYPES: UnsafeInited<HashMap<DeviceId, Driver>> =
    unsafe { UnsafeInited::uninit() };
pub static BLOCK_DEVICE_TYPES: UnsafeInited<HashMap<DeviceId, Driver>> =
    unsafe { UnsafeInited::uninit() };

pub fn find_driver(device_type: &DeviceType, device_id: &DeviceId) -> Option<&'static Driver> {
    match device_type {
        DeviceType::Char => CHAR_DEVICE_TYPES.get(device_id),
        DeviceType::Block => BLOCK_DEVICE_TYPES.get(device_id),
    }
}

pub fn init() {
    init_char_drivers();
    init_block_drivers();
}

macro_rules! define_driver {
    ($types:expr, $device_id:expr, read: $read:expr, write: $write:expr, fsync: $fsync:expr $(,)?) => {{
        let k = $device_id;
        let v = Driver {
            read: $read,
            write: $write,
            fsync: $fsync,
        };
        debug_assert!(!$types.contains_key(&k));
        unsafe { $types.insert_unique_unchecked(k, v) }
    }};
    ($types:expr, $device_id:expr, read: $read:expr, write: $write:expr $(,)?) => {{
        define_driver!($types, $device_id, read: $read, write: $write, fsync: || Ok(()))
    }};
}

pub const DEV_TTY: DeviceId = (5, 0);

fn init_char_drivers() {
    let mut types = HashMap::new();

    macro_rules! define {
        ($($tt:tt)*) => {
            define_driver!(types, $($tt)*);
        };
    }

    define!(
        DEV_TTY,
        read: |mut user_buf: *mut u8, len: u64| {
            let len = len as usize;

            const BUFFER_BYTES: usize = 128;

            let mut buffer = [MaybeUninit::uninit(); BUFFER_BYTES];

            let mut i = 0;
            while i < len {
                let index = i % BUFFER_BYTES;
                let chr = *buffer[index].write(getchar_u8().map_err(|_| VfsError::IoError)?);

                if index == BUFFER_BYTES - 1 {
                    copy_to_user(user_buf, unsafe { buffer.assume_init_ref() })?;
                    user_buf = unsafe { user_buf.add(BUFFER_BYTES) };
                }

                i += 1;
                if chr == b'\n' {
                    break; // 行缓冲
                }
            }

            let tail = i % BUFFER_BYTES;
            copy_to_user(user_buf, &unsafe { buffer.assume_init_ref() }[..tail])?;

            Ok(i as u64)
        },
        write: |mut user_data: *const u8, len: u64| {
            let init_len = len;
            let mut len = len as usize;

            const BUFFER_BYTES: usize = 1024;

            let mut buffer = [MaybeUninit::uninit(); BUFFER_BYTES];

            while len > 0 {
                let count = len.min(BUFFER_BYTES);

                let buf = copy_from_user_uninit(&mut buffer[..count], user_data)?;
                for byte in buf {
                    putchar_u8(*byte).map_err(|_| VfsError::IoError)?;
                }

                len -= count;
                user_data = unsafe { user_data.add(count) };
            }

            Ok(init_len)
        },
    );

    unsafe { UnsafeInited::init_unsafe(&CHAR_DEVICE_TYPES, types) };
}

fn init_block_drivers() {
    let types = HashMap::new();

    // TODO

    unsafe { UnsafeInited::init_unsafe(&BLOCK_DEVICE_TYPES, types) };
}
