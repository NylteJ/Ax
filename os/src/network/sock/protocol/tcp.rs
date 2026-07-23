use core::net::Ipv6Addr;

use alloc::{boxed::Box, vec::Vec};
use smoltcp::{
    socket::tcp::{self, Socket, State},
    wire::IpAddress,
};

use crate::{
    basic::sync::{SpinMutex, SpinRwLock},
    network::{namespace::GLOBAL_NET_NAMESPACE, sock::{Sock, SockProtocol}, to_v6_addr},
};

pub struct Tcp {
    pub smol_socket: SpinRwLock<Socket<'static>>,
    accept_queue: SpinMutex<Vec<Box<Sock>>>,
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
            smol_socket: SpinRwLock::new(socket),
            accept_queue: SpinMutex::new(Vec::new()),
        }
    }

    pub(in super::super) fn push_accept(&self, sock: Box<Sock>) {
        self.accept_queue.lock().push(sock);
    }

    pub(in super::super) fn pop_accept(&self) -> Option<Box<Sock>> {
        self.accept_queue.lock().pop()
    }

    /// 获取 accept_queue 的长度
    #[allow(unused)]
    pub(in super::super) fn accept_queue_len(&self) -> usize {
        self.accept_queue.lock().len()
    }
}

impl Drop for Tcp {
    fn drop(&mut self) {
        let smol_socket = self.smol_socket.get_mut();

        if smol_socket.state() == State::Established {
            let local = smol_socket.local_endpoint().unwrap();
            let remote = smol_socket.remote_endpoint().unwrap();

            GLOBAL_NET_NAMESPACE.unregister_established(
                to_v6_addr(local.addr),
                local.port,
                to_v6_addr(remote.addr),
                remote.port,
            );
        } else {
            debug_assert!(
                smol_socket.local_endpoint().zip(smol_socket.remote_endpoint()).is_none_or(|(local, remote)|{
                    GLOBAL_NET_NAMESPACE.get_established_sock(
                        to_v6_addr(local.addr),
                        local.port,
                        to_v6_addr(remote.addr),
                        remote.port,
                    ).is_none()
                })
            )
        }
    }
}
