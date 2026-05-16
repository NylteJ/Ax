use crate::{basic::sync::OnceInited, devices::block_device::BlockDevice};

pub mod block_device;
pub mod pci;
pub mod virtio;

mod hal;

// FIXME: 如果我们未来要支持多块设备，则这里需修改
pub static BLOCK_DEVICES: OnceInited<[&'static dyn BlockDevice; 1]> = OnceInited::new();

#[derive(Debug)]
pub enum DeviceError {
    BufferTooSmall,
    InvalidBlockId,
    NotYetImplemented,

    VirtIOError(virtio_drivers::Error),
}

pub type DeviceResult<T> = Result<T, DeviceError>;

impl From<virtio_drivers::Error> for DeviceError {
    fn from(value: virtio_drivers::Error) -> Self {
        Self::VirtIOError(value)
    }
}
