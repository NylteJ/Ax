use crate::{
    basic::sync::RwLock,
    network::{
        namespace::GLOBAL_NET_NAMESPACE,
        sock::{BindPart1Error, Sock, SockDomain, SockProtocol, domain::copy_sockaddr_to_user},
        socket::SocketAddress,
    },
    syscall::{
        SyscallResult,
        err_codes::ENOTCONN,
        net::{Domain, IPv6SockAddr},
    },
};

pub struct IPv6<Proto: ?Sized + SockProtocol> {
    pub address: RwLock<Option<IPv6SockAddr>>,
    pub peer_address: RwLock<Option<IPv6SockAddr>>,

    pub proto: Proto,
}

impl<Proto: ?Sized + SockProtocol> SockDomain for IPv6<Proto> {
    fn getsockname(&self, addr: *mut u8, addrlen: *mut u32) -> SyscallResult {
        copy_sockaddr_to_user(
            addr,
            addrlen,
            Domain::INET6,
            &self.address.read().unwrap_or_default(),
        )
    }

    fn getpeername(&self, addr: *mut u8, addrlen: *mut u32) -> SyscallResult {
        copy_sockaddr_to_user(
            addr,
            addrlen,
            Domain::INET6,
            &self.peer_address.read().ok_or(ENOTCONN)?,
        )
    }

    fn bind_part_1(&self, addr: SocketAddress, sock: &Sock) -> Result<(), BindPart1Error> {
        let SocketAddress::IPv6(addr) = addr else {
            return Err(BindPart1Error::AddressTypeMismatch);
        };

        {
            let mut address = self.address.write();
            if address.is_some() {
                return Err(BindPart1Error::AlreadyBound);
            }
            address.replace(addr);
        }

        unsafe { GLOBAL_NET_NAMESPACE.bind_socket(addr.ipv6_addr(), addr.port.get(), sock) }?;

        Ok(())
    }
}

impl<Proto: ?Sized + SockProtocol> Drop for IPv6<Proto> {
    fn drop(&mut self) {
        if let Some(address) = self.address.get_mut() {
            GLOBAL_NET_NAMESPACE.unbind_socket(address.ipv6_addr(), address.port.get());
        }
    }
}
