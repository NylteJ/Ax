pub mod domain;
pub mod protocol;

mod ops;

use core::sync::atomic::AtomicBool;

use crate::{
    network::{namespace::BindSocketError, socket::SocketAddress},
    syscall::SyscallResult,
};
use derive_more::Debug;

pub use ops::get_sock_ops;

#[derive(Debug)]
pub struct Sock<Data: ?Sized + SockDomain = dyn SockDomain> {
    pub nonblocking: AtomicBool,

    #[debug(skip)]
    pub ops: &'static ops::SockOps,
    #[debug(skip)]
    pub data: Data,
}

impl Sock {
    pub fn new<Data: SockDomain>(
        ops: &'static ops::SockOps,
        data: Data,
    ) -> alloc::boxed::Box<Self> {
        alloc::boxed::Box::new(Sock {
            nonblocking: AtomicBool::new(false),
            ops,
            data,
        })
    }
}

pub trait SockDomain: Send + Sync + 'static {
    /// 获取 SockAddress。和系统调用不同，若 addr 为空指针，则其直接返回 0。
    ///
    /// getsockname 只由 domain 决定，所以放在这里更好
    fn getsockname(&self, addr: *mut u8, addrlen: *mut u32) -> SyscallResult;
    /// 获取对端的 SockAddress。和系统调用不同，若 addr 为空指针，则其直接返回 0。
    ///
    /// getpeername 只由 domain 决定，所以放在这里更好
    fn getpeername(&self, addr: *mut u8, addrlen: *mut u32) -> SyscallResult;

    fn bind_part_1(&self, addr: SocketAddress, sock: &Sock) -> Result<(), BindPart1Error>;
    fn connect_part_1(&self, addr: SocketAddress) -> Result<(), ConnectPart1Error>;
}

#[derive(Debug)]
pub enum BindPart1Error {
    AddressTypeMismatch,
    AlreadyBound,
    AddressAlreadyInUse,
}

impl From<BindSocketError> for BindPart1Error {
    fn from(value: BindSocketError) -> Self {
        match value {
            BindSocketError::AddressAlreadyInUse => BindPart1Error::AddressAlreadyInUse,
        }
    }
}

#[derive(Debug)]
pub enum ConnectPart1Error {
    AddressTypeMismatch,
    AlreadyBound,
}

pub trait SockProtocol: Send + Sync + 'static {}

pub use domain::{IPv4Sock, IPv6Sock};

pub type Tcp4Sock = IPv4Sock<protocol::Tcp>;
pub type Tcp6Sock = IPv6Sock<protocol::Tcp>;
pub type Udp4Sock = IPv4Sock<protocol::Udp>;
pub type Udp6Sock = IPv6Sock<protocol::Udp>;
