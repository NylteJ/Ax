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
        }
    }
}

impl<Proto: ?Sized + SockProtocol> Drop for IPv4<Proto> {
    fn drop(&mut self) {
        if let Some(address) = self.address.get_mut() {
            GLOBAL_NET_NAMESPACE
                .unbind_socket(address.ipv4_addr().to_ipv6_mapped(), address.port.get());
        }
    }
}
