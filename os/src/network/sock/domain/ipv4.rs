use crate::{
    basic::sync::RwLock,
    network::{
        namespace::GLOBAL_NET_NAMESPACE,
        sock::{
            BindPart1Error, ConnectPart1Error, Sock, SockDomain, SockProtocol,
            domain::copy_sockaddr_to_user,
        },
        socket::SocketAddress,
    },
    syscall::{
        SyscallResult,
        err_codes::ENOTCONN,
        net::{Domain, IPv4SockAddr},
    },
};

pub struct IPv4<Proto: ?Sized + SockProtocol> {
    pub address: RwLock<Option<IPv4SockAddr>>,
    pub peer_address: RwLock<Option<IPv4SockAddr>>,

    /// 标记此 socket 是通过 accept() 产生的子 socket，它与监听 socket 共享端口绑定。
    /// Drop 时不应调用 unbind_socket（绑定由监听 socket 管理）。
    pub is_accepted: bool,

    pub proto: Proto,
}

impl<Proto: ?Sized + SockProtocol> SockDomain for IPv4<Proto> {
    fn getsockname(&self, addr: *mut u8, addrlen: *mut u32) -> SyscallResult {
        copy_sockaddr_to_user(
            addr,
            addrlen,
            Domain::INET,
            &self.address.read().unwrap_or_default(),
        )
    }

    fn getpeername(&self, addr: *mut u8, addrlen: *mut u32) -> SyscallResult {
        copy_sockaddr_to_user(
            addr,
            addrlen,
            Domain::INET,
            &self.peer_address.read().ok_or(ENOTCONN)?,
        )
    }

    fn bind_part_1(&self, addr: SocketAddress, sock: &Sock) -> Result<(), BindPart1Error> {
        let addr =
            get_and_set_address!(self.address, set to addr, expected variant IPv4, BindPart1Error);

        unsafe {
            GLOBAL_NET_NAMESPACE.bind_socket(
                addr.ipv4_addr().to_ipv6_mapped(),
                addr.port.get(),
                sock,
            )
        }?;

        Ok(())
    }

    fn connect_part_1(&self, addr: SocketAddress) -> Result<(), ConnectPart1Error> {
        get_and_set_address!(self.peer_address, set to addr, expected variant IPv4, ConnectPart1Error);
        Ok(())
    }
}

impl<Proto: SockProtocol> IPv4<Proto> {
    pub fn new(proto: Proto) -> Self {
        Self {
            address: RwLock::new(None),
            peer_address: RwLock::new(None),
            proto,
            is_accepted: false,
        }
    }
}

impl<Proto: ?Sized + SockProtocol> Drop for IPv4<Proto> {
    fn drop(&mut self) {
        // 网络操作必须在 NET_LOCK 下进行（可重入）。见 NET_LOCK 的文档。
        let _net_lock = crate::network::NET_LOCK.lock_guard();

        if self.is_accepted {
            // 子 socket 与监听 socket 共享端口绑定，不负责解绑
            return;
        }
        if let Some(address) = self.address.get_mut() {
            GLOBAL_NET_NAMESPACE
                .unbind_socket(address.ipv4_addr().to_ipv6_mapped(), address.port.get());
        }
    }
}
