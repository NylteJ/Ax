use crate::{
    basic::sync::SpinRwLock,
    network::{
        namespace::GLOBAL_NET_NAMESPACE,
        sock::{BindPart1Error, Sock, SockDomain, SockProtocol, domain::copy_sockaddr_to_user},
        socket::SocketAddress,
    },
    syscall::{
        SyscallResult,
        err_codes::ENOTCONN,
        net::{Domain, IPv4SockAddr},
    },
};

pub struct IPv4<Proto: ?Sized + SockProtocol> {
    pub address: SpinRwLock<Option<IPv4SockAddr>>,
    pub peer_address: SpinRwLock<Option<IPv4SockAddr>>,

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
        let SocketAddress::IPv4(addr) = addr else {
            return Err(BindPart1Error::AddressTypeMismatch);
        };

        {
            let mut address = self.address.write();
            if address.is_some() {
                return Err(BindPart1Error::AlreadyBound);
            }
            address.replace(addr);
        }

        unsafe {
            GLOBAL_NET_NAMESPACE.bind_socket(
                addr.ipv4_addr().to_ipv6_mapped(),
                addr.port.get(),
                sock,
            )
        }?;

        Ok(())
    }
}

impl<Proto: SockProtocol> IPv4<Proto> {
    pub fn new(proto: Proto) -> Self {
        Self {
            address: SpinRwLock::new(None),
            peer_address: SpinRwLock::new(None),
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
