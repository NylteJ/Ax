use alloc::vec;
use smoltcp::{
    socket::{udp::PacketMetadata, udp::Socket},
    storage::PacketBuffer,
};

use crate::{basic::sync::SpinRwLock, network::sock::SockProtocol};

pub struct Udp {
    pub smol_socket: SpinRwLock<Socket<'static>>,
}

impl SockProtocol for Udp {}

impl Udp {
    pub fn new() -> Self {
        macro_rules! buffer {
            ($count:literal packets, $bytes:literal bytes) => {
                PacketBuffer::new(vec![PacketMetadata::EMPTY; $count], vec![
                    Default::default();
                    $bytes
                ])
            };
        }
        let rx_buffer = buffer!(64 packets, 4096 bytes);
        let tx_buffer = buffer!(64 packets, 4096 bytes);

        Self {
            smol_socket: SpinRwLock::new(Socket::new(rx_buffer, tx_buffer)),
        }
    }
}
