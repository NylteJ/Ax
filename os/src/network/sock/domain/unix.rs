use crate::{
    basic::sync::SpinRwLock,
    network::{
        sock::{BindPart1Error, Sock, SockDomain, SockProtocol, domain::copy_sockaddr_to_user},
        socket::SocketAddress,
    },
    syscall::{
        SyscallResult,
        err_codes::ENOTCONN,
        net::{Domain, UnixSockAddr},
    },
};

pub struct Unix<Proto: ?Sized + SockProtocol> {
    pub address: SpinRwLock<Option<UnixSockAddr>>,
    pub peer_address: SpinRwLock<Option<UnixSockAddr>>,

    pub proto: Proto,
}

impl<Proto: ?Sized + SockProtocol> SockDomain for Unix<Proto> {
    fn getsockname(&self, addr: *mut u8, addrlen: *mut u32) -> SyscallResult {
        copy_sockaddr_to_user(
            addr,
            addrlen,
            Domain::UNIX,
            &self.address.read().unwrap_or_default(),
        )
    }

    fn getpeername(&self, addr: *mut u8, addrlen: *mut u32) -> SyscallResult {
        copy_sockaddr_to_user(
            addr,
            addrlen,
            Domain::UNIX,
            &self.peer_address.read().ok_or(ENOTCONN)?,
        )
    }

    fn bind_part_1(&self, addr: SocketAddress, sock: &Sock) -> Result<(), BindPart1Error> {
        todo!()
    }
}
