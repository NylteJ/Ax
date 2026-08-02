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

/// TCP 接收缓冲区的最大容量（字节）。
pub const TCP_MAX_RX_BUFFER_BYTES: usize = 128 * 1024;
/// TCP 发送缓冲区的最大容量（字节）。
pub const TCP_MAX_TX_BUFFER_BYTES: usize = 64 * 1024;
/// TCP 接收/发送缓冲区的初始容量（字节），不得大于对应的最大容量。
const TCP_INITIAL_BUFFER_BYTES: usize = 65536;

impl Tcp {
    pub fn new() -> Self {
        debug_assert!(TCP_INITIAL_BUFFER_BYTES <= TCP_MAX_RX_BUFFER_BYTES);
        debug_assert!(TCP_INITIAL_BUFFER_BYTES <= TCP_MAX_TX_BUFFER_BYTES);

        // 初始化时即分配足够的缓冲区容量，确保 TCP 窗口不为 0。
        // 若窗口为 0，对端无法发送数据，导致 TCP 连接无法正常工作。
        let mut rx_buffer = tcp::SocketBuffer::new(alloc::vec![0; TCP_INITIAL_BUFFER_BYTES]);
        let mut tx_buffer = tcp::SocketBuffer::new(alloc::vec![0; TCP_INITIAL_BUFFER_BYTES]);

        // 设置最大容量，允许缓冲区在需要时自动增长。
        // RingBuffer 默认 max_capacity=0（禁止增长），必须显式设置。
        rx_buffer.set_max_capacity(TCP_MAX_RX_BUFFER_BYTES);
        tx_buffer.set_max_capacity(TCP_MAX_TX_BUFFER_BYTES);

        let mut socket = Socket::new(rx_buffer, tx_buffer);
        // 禁用延迟 ACK，确保数据能及时确认，窗口能及时更新
        socket.set_ack_delay(None);
        Self::with_smol_socket(socket)
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
        // 网络操作必须在 NET_LOCK 下进行（可重入）。见 NET_LOCK 的文档。
        let _net_lock = crate::network::NET_LOCK.lock_guard();

        let smol_socket = self.smol_socket.get_mut();

        // 曾处于 Established 及其后续状态（FinWait1/2, CloseWait, Closing, LastAck）
        // 的 socket 都已注册到 established 表，Drop 时必须移除，否则留下悬空指针。
        // Closed / TimeWait 表示连接已完全清理，无需额外操作。
        // Listen / SynSent / SynReceived 从未注册到 established 表，也无需操作。
        let needs_unregister = matches!(
            smol_socket.state(),
            State::Established
                | State::FinWait1
                | State::FinWait2
                | State::CloseWait
                | State::Closing
                | State::LastAck
        );

        if needs_unregister {
            if let (Some(local), Some(remote)) =
                (smol_socket.local_endpoint(), smol_socket.remote_endpoint())
            {
                GLOBAL_NET_NAMESPACE.unregister_established(
                    to_v6_addr(local.addr),
                    local.port,
                    to_v6_addr(remote.addr),
                    remote.port,
                );
            }
        }

        match smol_socket.state() {
            State::Established | State::CloseWait => {
                // 主动关闭连接，发送 FIN。
                // Established → close() transitions to FinWait1, sends FIN.
                // CloseWait means peer already sent FIN but local app hasn't
                // called close(); if the socket is dropped we must send FIN
                // to prevent the peer from hanging in FinWait2.
                smol_socket.close();
                // 发送 FIN 包；若对端已不存在则静默忽略
                let _ = crate::network::sock::ops::do_egress(
                    smoltcp::socket::SocketMRef::Tcp(smol_socket),
                    true,
                );
            }
            State::Closed | State::TimeWait => {
                // 已经关闭，无需额外操作
            }
            _ => {
                // 其它状态：清理但不需要发送 FIN
            }
        }
    }
}
