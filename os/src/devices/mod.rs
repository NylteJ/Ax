use alloc::{boxed::Box, vec::Vec};

use crate::basic::UnsafeInited;

pub mod block_device;
pub mod pci;
pub mod virtio;

mod hal;

use block_device::BlockDevice;

pub static DEVICES: UnsafeInited<Devices> = unsafe { UnsafeInited::uninit() };

#[derive(Debug)]
pub enum DeviceError {
    VirtIOError(virtio_drivers::Error),
}

pub type DeviceResult<T> = Result<T, DeviceError>;

impl From<virtio_drivers::Error> for DeviceError {
    fn from(value: virtio_drivers::Error) -> Self {
        Self::VirtIOError(value)
    }
}

pub struct Devices {
    pub block: Vec<Box<dyn BlockDevice>>,
}

impl Devices {
    pub fn new() -> Self {
        Self { block: Vec::new() }
    }

    /// 添加一个块设备，内部会执行去重
    pub fn add_block_device(&mut self, device: Box<dyn BlockDevice>) {
        use core::ffi::CStr;

        use crate::basic::bstr::ByteStr;

        let device_id = device.device_id();
        let device_id = device_id
            .as_ref()
            .ok()
            .and_then(|str| CStr::from_bytes_until_nul(str.as_bytes()).ok())
            .unwrap_or(c"");

        if device_id.is_empty() {
            log::warn!("待添加块设备没有设备 ID，只能依据其它参数尽力去重，可能误杀");

            let has_dup = self.block.iter().any(|dev| {
                dev.block_size() == device.block_size() && dev.block_count() == device.block_count()
            });

            if has_dup {
                log::warn!(
                    "待添加块设备与已有设备在块大小和块数量上完全相同，可能是重复的，跳过添加"
                );
                return;
            }
        } else {
            let has_dup = self.block.iter().any(|dev| {
                dev.device_id().map_or(false, |str| {
                    CStr::from_bytes_until_nul(str.as_bytes())
                        .map_or(false, |cstr| cstr == device_id)
                })
            });

            if has_dup {
                log::info!(
                    "待添加块设备的设备 ID {} 与已有设备重复，跳过添加",
                    ByteStr::new(device_id.to_bytes())
                );
                return;
            }
        }

        log::info!(
            "添加块设备，设备 ID: {}",
            ByteStr::new(device_id.to_bytes())
        );
        self.block.push(device);
    }
}
