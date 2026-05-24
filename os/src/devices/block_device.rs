use core::mem::MaybeUninit;

use alloc::string::String;
use uninit::extension_traits::MaybeUninitExt;
use virtio_drivers::{
    device::blk::{SECTOR_SIZE, VirtIOBlk},
    transport::Transport,
};

use crate::{
    basic::sync::Mutex,
    devices::{DeviceResult, hal::VirtIOHal},
};

/// 对块设备的抽象。所有公开接口都接收不可变引用，块设备应自行维护内部可变性。
pub trait BlockDevice: Send + Sync {
    /// 从从 `block_id` 开始的块中读取值到 `buf` 对应的缓冲区，缓冲区大小必须为块大小的非零整数倍。成功时返回读取到的部分的切片（保证从零下标开始写，因此丢弃返回值也不会真正产生问题）。
    ///
    /// 在操作完成前不会返回。
    fn read<'a>(
        &self,
        block_id: usize,
        buf: &'a mut [MaybeUninit<u8>],
    ) -> DeviceResult<&'a mut [u8]>;
    /// 将 `buf` 中的数据写入从 `block_id` 开始的块中，缓冲区大小必须为块大小的非零整数倍。
    ///
    /// 在操作完成前不会返回。
    fn write(&self, block_id: usize, data: &[u8]) -> DeviceResult<()>;

    /// 强制刷新设备缓冲区（如果有），让所有写入操作真正生效。
    ///
    /// 在操作完成前不会返回。
    fn flush(&self) -> DeviceResult<()>;

    /// 返回块的大小（单位：字节）
    fn block_size(&self) -> usize;
    /// 返回块的数量
    fn block_count(&self) -> u64;

    fn device_id(&self) -> DeviceResult<String>;
}

impl<T: Transport + Send + Sync> BlockDevice for Mutex<VirtIOBlk<VirtIOHal, T>> {
    fn read<'a>(
        &self,
        block_id: usize,
        buf: &'a mut [MaybeUninit<u8>],
    ) -> DeviceResult<&'a mut [u8]> {
        // SAFETY: 很遗憾这并不安全，但接口是这么写的（而且实际上这通常也不会产生什么问题）
        let buf = unsafe { buf.assume_init_by_mut() };

        // read_blocks 内部会检测大小是否为块大小的非零整数倍
        // TODO: 避免阻塞
        self.lock().read_blocks(block_id, buf)?;

        Ok(buf)
    }

    fn write(&self, block_id: usize, data: &[u8]) -> DeviceResult<()> {
        // read_blocks 内部会检测大小是否为块大小的非零整数倍
        // TODO: 避免阻塞
        self.lock().write_blocks(block_id, data)?;

        Ok(())
    }

    fn flush(&self) -> DeviceResult<()> {
        Ok(self.lock().flush()?)
    }

    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn block_count(&self) -> u64 {
        self.lock().capacity()
    }

    fn device_id(&self) -> DeviceResult<String> {
        let mut buffer = [0u8; 20];
        let length = self.lock().device_id(&mut buffer)?;
        Ok(String::from_utf8_lossy(&buffer[..length]).into_owned())
    }
}
