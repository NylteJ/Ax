use alloc::{boxed::Box, vec::Vec};

use crate::basic::{
    UnsafeInited,
    sync::Mutex,
    timer::{cpu_tick_to_us, get_cpu_tick},
};

pub mod block_device;
pub mod pci;
pub mod virtio;

mod hal;
mod net_loopback;

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
    pub network_lo: Mutex<net_loopback::NetLoopbackDevice>, // FIXME
    pub network_lo_iface: Mutex<smoltcp::iface::Interface>,
}

impl Devices {
    pub fn new() -> Self {
        let mut network_lo = net_loopback::NetLoopbackDevice::new();
        let config = {
            let mut config = smoltcp::iface::Config::new(smoltcp::wire::HardwareAddress::Ip);
            config.random_seed = get_cpu_tick() as u64; // 按文档的说法，这个值只要在不同启动间不同即可
            config
        };

        let now = smoltcp::time::Instant::from_micros(cpu_tick_to_us(get_cpu_tick()) as i64 / 1000);
        let mut network_lo_iface = smoltcp::iface::Interface::new(config, &mut network_lo, now);

        network_lo_iface.set_any_ip(true);

        Self {
            block: Vec::new(),
            network_lo: Mutex::new(network_lo),
            network_lo_iface: Mutex::new(network_lo_iface),
        }
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
