use core::{
    ffi::c_void,
    net::{Ipv4Addr, Ipv6Addr},
    sync::atomic::Ordering,
};

use alloc::boxed::Box;
use bitflags::bitflags;
use num_enum::TryFromPrimitive;
use zerocopy::network_endian;

use crate::multitask::FDTableEntry;
use crate::network::namespace::GLOBAL_NET_NAMESPACE;
use crate::network::socket::{Socket, SocketAddress};
use crate::syscall::filesystem::get_file_from_fd;
use crate::syscall::{SyscallResult, filesystem::cur_fd_rlimit};
use crate::{
    filesystem::{SocketFile, vfs::File},
    multitask::cur_task,
};
use crate::{memory::uaccess::copy_from_user_uninit, multitask::wait::Waiting};

use crate::syscall::err_codes::*;

#[derive(TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u16)]
pub enum Domain {
    UNIX = 1,
    INET = 2,
    INET6 = 10,
    // NETLINK = 16,
    // PACKET = 17,
    // RDS = 21,
    // ALG = 38,
    // UNSPEC = 512,
}

#[derive(TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum SocketType {
    /// TCP
    STREAM = 1,
    /// UDP
    DGRAM = 2,
    RAW = 3,
    // RDM = 4,
    SEQPACKET = 5,
    // PACKET = 10,
}

bitflags! {
    struct SocketFlags: isize {
        const CLOEXEC = 0x80000;
        const NONBLOCK = 0x800;
    }
}

#[derive(TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum Protocol {
    IP = 0,
    ICMP = 1,
    IGMP = 2,
    ETH_P_ALL = 3,
    TCP = 6,
    EGP = 8,
    PUP = 12,
    UDP = 17,
    IDP = 22,
    IPv6 = 41,
    ICMPv6 = 58,
    SCTP = 132,
    UDPLITE = 136,
}

pub extern "C" fn socket(domain: isize, type_: isize, protocol: isize) -> SyscallResult {
    let domain: u16 = domain.try_into().map_err(|_| EAFNOSUPPORT)?;
    let domain = Domain::try_from(domain).map_err(|_| EAFNOSUPPORT)?;

    let socket_flags = SocketFlags::from_bits_retain(type_);
    let socket_type = SocketType::try_from(type_ as u8).map_err(|_| EINVAL)?;

    let protocol: u8 = protocol.try_into().map_err(|_| EINVAL)?;
    let protocol = Protocol::try_from(protocol).map_err(|_| EINVAL)?;

    let socket = Socket::new(domain, socket_type, protocol)?;
    if socket_flags.contains(SocketFlags::NONBLOCK) {
        socket.sock.nonblocking.store(true, Ordering::Relaxed);
    }

    let file = SocketFile::new(socket);

    let fd_entry = if socket_flags.contains(SocketFlags::CLOEXEC) {
        FDTableEntry::cloexec(file)
    } else {
        FDTableEntry::no_flags(file)
    };

    let fd = cur_task().files.write().insert(fd_entry, cur_fd_rlimit())?;

    SyscallResult::success(fd)
}

#[repr(C)]
#[derive(derive_more::Debug, Clone, Copy, Default)]
pub struct IPv4SockAddr {
    pub port: network_endian::U16,
    pub addr: network_endian::U32,
    #[debug(skip)]
    pub _zero: [u8; 8],
}
unsafe impl bytemuck::Zeroable for IPv4SockAddr {}
unsafe impl bytemuck::AnyBitPattern for IPv4SockAddr {}
unsafe impl bytemuck::NoUninit for IPv4SockAddr {}

impl IPv4SockAddr {
    pub fn to_endpoint(&self) -> smoltcp::wire::IpEndpoint {
        use smoltcp::wire::{IpAddress, IpEndpoint};
        let addr = IpAddress::Ipv4(self.ipv4_addr());
        let port = self.port.get();
        IpEndpoint::new(addr, port)
    }

    #[inline]
    pub fn ipv4_addr(&self) -> Ipv4Addr {
        Ipv4Addr::from_octets(self.addr.to_bytes())
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IPv6SockAddr {
    pub port: network_endian::U16,
    pub flowinfo: network_endian::U32,
    pub addr: network_endian::U128,
    pub scope: network_endian::U32,
}
unsafe impl bytemuck::Zeroable for IPv6SockAddr {}
unsafe impl bytemuck::AnyBitPattern for IPv6SockAddr {}
unsafe impl bytemuck::NoUninit for IPv6SockAddr {}

impl IPv6SockAddr {
    #[inline]
    pub fn ipv6_addr(&self) -> Ipv6Addr {
        Ipv6Addr::from_octets(self.addr.to_bytes())
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UnixSockAddr {
    pub path: [u8; 108],
}
unsafe impl bytemuck::Zeroable for UnixSockAddr {}
unsafe impl bytemuck::AnyBitPattern for UnixSockAddr {}
unsafe impl bytemuck::NoUninit for UnixSockAddr {}

impl Default for UnixSockAddr {
    fn default() -> Self {
        Self { path: [0; 108] }
    }
}

fn get_socket_from_file(file: &File) -> Result<&Socket, SyscallResult> {
    let socket_file = SocketFile::from_dyn_file(file).ok_or(ENOTSOCK)?;
    Ok(SocketFile::get_socket(socket_file))
}

macro_rules! get_socket_addr {
    (addr: $addre:expr, addrlen: $addrlen:expr, $($domain:pat => $addr:ident = $type:ty { $($tokens:tt)* })+) => {{
        let mut bytes;
        let (family, rest_bytes) = {
            bytes = Box::try_new_uninit_slice($addrlen)?;
            let bytes = copy_from_user_uninit(&mut bytes, $addre)?;

            if bytes.len() < 2 {
                return EINVAL;
            }

            let sa_family = u16::from_ne_bytes([bytes[0], bytes[1]]);
            let family = Domain::try_from(sa_family).map_err(|_| EAFNOSUPPORT)?;

            (family, &mut bytes[2..])
        };

        match family {
            $($domain => {
                static_assertions::assert_impl_all!($type: bytemuck::AnyBitPattern);
                let $addr = {
                    const BYTES: usize = size_of::<$type>();
                    let array = rest_bytes.as_array::<BYTES>().ok_or(EINVAL)?;
                    unsafe { core::mem::transmute::<_, &$type>(array) }
                };

                $($tokens)*
            })+
        }
    }};
}

pub extern "C" fn bind(sockfd: isize, addr: *const u8, addrlen: usize) -> SyscallResult {
    let file = get_file_from_fd(sockfd)?;
    let socket = get_socket_from_file(&file)?;
    let sock = socket.sock.as_ref();

    fn ipv4_ipv6_common(port: u16) -> SyscallResult {
        let new_port = if port == 0 {
            // 自动分配临时端口
            GLOBAL_NET_NAMESPACE
                .new_ephemeral_port()
                .ok_or(EADDRINUSE)?
        } else {
            if port < 1024 {
                // 保留端口，要求特权
                let cur_uids = *cur_task().user_ids.read();

                if cur_uids.egid != 0 || cur_uids.euid != 0 {
                    return EACCES;
                }
            }
            port
        };

        // TODO: EADDRNOTAVAIL（“requested address was not local”）
        //       （其实大概率不会 DO 了，因为这个貌似要路由表，看着就复杂到爆炸）

        SyscallResult::success(new_port as usize)
    }

    let socket_addr = get_socket_addr! {
        addr: addr,
        addrlen: addrlen,

        Domain::UNIX => unix_addr = UnixSockAddr {
            todo!()
        }
        Domain::INET => ipv4_addr = IPv4SockAddr {
            let new_port = ipv4_ipv6_common(ipv4_addr.port.get())? as u16;

            SocketAddress::IPv4(IPv4SockAddr {
                port: new_port.into(),
                ..*ipv4_addr
            })
        }
        Domain::INET6 => ipv6_addr = IPv6SockAddr {
            let new_port = ipv4_ipv6_common(ipv6_addr.port.get())? as u16;

            SocketAddress::IPv6(IPv6SockAddr {
                port: new_port.into(),
                ..*ipv6_addr
            })
        }
    };

    sock.data.bind_part_1(socket_addr, sock)?;
    sock.ops.bind_part_2(sock)
}

pub extern "C" fn getsockname(sockfd: isize, addr: *mut u8, addrlen: *mut u32) -> SyscallResult {
    if addr.is_null() {
        // 注意下面的 getsockname 允许 addr 为 NULL，但系统调用不允许
        // FIXME：如果有哪个神经病映射了 0 地址就会出问题
        return EFAULT;
    }

    let file = get_file_from_fd(sockfd)?;
    let socket = get_socket_from_file(&file)?;
    socket.sock.data.getsockname(addr, addrlen)
}

const MAX_BACKLOG: u32 = 128;

pub extern "C" fn listen(sockfd: isize, backlog: isize) -> SyscallResult {
    let file = get_file_from_fd(sockfd)?;
    let socket = get_socket_from_file(&file)?;

    let backlog = (backlog as u32).clamp(1, MAX_BACKLOG);

    match socket.socket_type {
        SocketType::STREAM | SocketType::SEQPACKET => socket.sock.ops.listen(&socket.sock, backlog),
        _ => EOPNOTSUPP,
    }
}

fn autoretry_advanced<R>(
    nonblocking: bool,
    mut func: impl FnMut() -> R,
    mut need_retry: impl FnMut(&R) -> bool,
) -> Result<R, SyscallResult> {
    loop {
        let result = func();

        if need_retry(&result) {
            if nonblocking {
                return Err(EAGAIN);
            } else {
                let mut waiting = Waiting::new();
                waiting.wait(&GLOBAL_NET_NAMESPACE.wait_queue);
                waiting.sleep()?;
            }
        } else {
            break Ok(result);
        }
    }
}

fn autoretry(nonblocking: bool, func: impl FnMut() -> SyscallResult) -> SyscallResult {
    match autoretry_advanced(nonblocking, func, |result| *result == E_SPECIAL_NET_RETRY) {
        Ok(x) => x,
        Err(x) => x,
    }
}

pub extern "C" fn accept(sockfd: isize, addr: *mut u8, addrlen: *mut u32) -> SyscallResult {
    let file = get_file_from_fd(sockfd)?;
    let socket = get_socket_from_file(&file)?;
    let sock = &socket.sock;

    let new_sock = match socket.socket_type {
        SocketType::STREAM | SocketType::SEQPACKET => autoretry_advanced(
            sock.nonblocking.load(Ordering::Relaxed),
            || sock.ops.accept(&socket.sock),
            |result| matches!(result, Err(E_SPECIAL_NET_RETRY)),
        )??,
        _ => return EOPNOTSUPP,
    };

    new_sock.data.getpeername(addr, addrlen)?; // 它允许 addr 为空

    let new_socket = Socket {
        sock: new_sock,
        ..*socket
    };

    let fd_entry = FDTableEntry::no_flags(SocketFile::new(new_socket));

    let fd = cur_task().files.write().insert(fd_entry, cur_fd_rlimit())?;

    SyscallResult::success(fd)
}

pub extern "C" fn getpeername(sockfd: isize, addr: *mut u8, addrlen: *mut u32) -> SyscallResult {
    if addr.is_null() {
        // 注意下面的 getpeername 允许 addr 为 NULL，但系统调用不允许
        // FIXME：如果有哪个神经病映射了 0 地址就会出问题
        return EFAULT;
    }

    let file = get_file_from_fd(sockfd)?;
    let socket = get_socket_from_file(&file)?;
    socket.sock.data.getpeername(addr, addrlen)
}

fn copy_addr_from_user(addr: *const u8, addrlen: usize) -> Result<SocketAddress, SyscallResult> {
    let mut ret = None;
    let sys_result = || -> SyscallResult {
        ret.replace(get_socket_addr! {
            addr: addr,
            addrlen: addrlen,

            Domain::UNIX => unix_addr = UnixSockAddr {
                todo!()
            }
            Domain::INET => ipv4_addr = IPv4SockAddr {
                SocketAddress::IPv4(*ipv4_addr)
            }
            Domain::INET6 => ipv6_addr = IPv6SockAddr {
                SocketAddress::IPv6(*ipv6_addr)
            }
        });
        SyscallResult::success(0)
    }();

    if sys_result.value != 0 {
        return Err(sys_result);
    }
    return Ok(ret.unwrap());
}

pub extern "C" fn sendto(
    sockfd: isize,
    buf: *const c_void,
    size: usize,
    flags: usize,
    dest_addr: *const u8,
    addrlen: usize,
) -> SyscallResult {
    if dest_addr.is_null() && addrlen == 0 {
        return send(sockfd, buf, size, flags);
    }

    let file = get_file_from_fd(sockfd)?;
    let socket = get_socket_from_file(&file)?;
    let sock = &socket.sock;

    match socket.socket_type {
        SocketType::STREAM | SocketType::SEQPACKET => EISCONN,
        _ => {
            let addr = copy_addr_from_user(dest_addr, addrlen)?;

            autoretry(sock.nonblocking.load(Ordering::Relaxed), || {
                sock.ops.sendto(&socket.sock, buf.cast(), size, addr)
            })
        }
    }
}

pub extern "C" fn recvfrom(
    sockfd: isize,
    buf: *mut c_void,
    size: usize,
    flags: usize,
    src_addr: *mut u8,
    addrlen: *mut u32,
) -> SyscallResult {
    let file = get_file_from_fd(sockfd)?;
    let socket = get_socket_from_file(&file)?;
    let sock = &socket.sock;

    autoretry(sock.nonblocking.load(Ordering::Relaxed), || {
        sock.ops.recvfrom(sock, buf.cast(), size, src_addr, addrlen)
    })
}

pub extern "C" fn connect(sockfd: isize, addr: *const u8, addrlen: usize) -> SyscallResult {
    let file = get_file_from_fd(sockfd)?;
    let socket = get_socket_from_file(&file)?;
    let sock = &socket.sock;

    let addr = copy_addr_from_user(addr, addrlen)?;
    sock.data.connect_part_1(addr)?;
    sock.ops.connect(sock)
}

pub extern "C" fn setsockopt() -> SyscallResult {
    log::error!("TODO: setsockopt");
    SyscallResult::success(0)
}

pub extern "C" fn send(
    sockfd: isize,
    buf: *const c_void,
    size: usize,
    flags: usize,
) -> SyscallResult {
    let file = get_file_from_fd(sockfd)?;
    let socket = get_socket_from_file(&file)?;
    let sock = &socket.sock;

    sock.ops.send(sock, buf.cast(), size)
}
