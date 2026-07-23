use core::{alloc::Layout, intrinsics::unlikely};

use alloc::{alloc::handle_alloc_error, boxed::Box, collections::vec_deque::VecDeque};

pub struct NetLoopbackDevice {
    queue: VecDeque<Box<[u8]>>,
}

const LOOPBACK_MTU: usize = 65536;

impl NetLoopbackDevice {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }
}
pub struct RxToken {
    packet: Box<[u8]>,
}

impl smoltcp::phy::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.packet)
    }
}

pub struct TxToken<'a> {
    device: &'a mut NetLoopbackDevice,
}

impl smoltcp::phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        debug_assert!(len <= LOOPBACK_MTU);

        // 这里不使用 Box::new_uninit_slice(len).assume_init() 以避免直接使用明确未初始化的内存
        // 由于我们知道我们的平台并不在意内存是否初始化，此处使用未初始化的内存以节省初始化开销
        let layout =
            unsafe { Layout::from_size_align_unchecked(len * size_of::<u8>(), align_of::<u8>()) };
        let buffer = unsafe { alloc::alloc::alloc(layout) };
        if unlikely(buffer.is_null()) {
            handle_alloc_error(layout);
        }
        let buffer = core::ptr::slice_from_raw_parts_mut(buffer, len);
        let mut buffer = unsafe { Box::from_raw(buffer) };

        let ret = f(&mut buffer);
        log::trace!("transmitted {:?} ({} bytes)", crate::basic::bstr::ByteStr::new(&buffer), buffer.len());

        self.device.queue.push_back(buffer);

        ret
    }
}

impl smoltcp::phy::Device for NetLoopbackDevice {
    type RxToken<'a> = RxToken;

    type TxToken<'a> = TxToken<'a>;

    fn receive(
        &mut self,
        _timestamp: smoltcp::time::Instant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.queue.pop_front()?;

        let tx = TxToken { device: self };
        let rx = RxToken { packet };
        Some((rx, tx))
    }

    fn transmit(&mut self, _timestamp: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken { device: self })
    }

    fn capabilities(&self) -> smoltcp::phy::DeviceCapabilities {
        let mut ret = smoltcp::phy::DeviceCapabilities::default();

        ret.medium = smoltcp::phy::Medium::Ip;
        ret.max_transmission_unit = LOOPBACK_MTU;

        ret
    }
}
