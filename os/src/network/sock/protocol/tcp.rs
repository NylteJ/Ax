use alloc::vec::Vec;
use smoltcp::socket::tcp::{self, Socket};

use crate::{basic::sync::RwLock, network::sock::SockProtocol};

pub struct Tcp {
    pub smol_socket: RwLock<Socket<'static>>,
}

impl SockProtocol for Tcp {}

impl Tcp {
    pub fn new() -> Self {
        // 需要时这两个缓冲区会自行增长
        // TODO: 最好也让它们能自行缩减
        let mut rx_buffer = tcp::SocketBuffer::new(Vec::new());
        let mut tx_buffer = tcp::SocketBuffer::new(Vec::new());

        const MAX_RX_BUFFER_BYTES: usize = 128 * 1024;
        const MAX_TX_BUFFER_BYTES: usize = 64 * 1024;

        rx_buffer.set_max_capacity(MAX_RX_BUFFER_BYTES);
        tx_buffer.set_max_capacity(MAX_TX_BUFFER_BYTES);

        Self::with_smol_socket(Socket::new(rx_buffer, tx_buffer))
    }

    pub(in super::super) fn with_smol_socket(socket: Socket<'static>) -> Self {
        Self {
            smol_socket: RwLock::new(socket),
        }
    }
}
