use core::{cmp::min, slice};

use crate::{
    memory::uaccess::{copy_one_from_user, copy_one_to_user, copy_to_user},
    network::sock::{Sock, SockProtocol},
    syscall::{SyscallResult, net::Domain},
};

macro_rules! get_and_set_address {
    ($target:expr, set to $addr:ident, expected variant $type:ident, $err_type:ty) => {{
        let SocketAddress::$type(addr) = $addr else {
            return Err(<$err_type>::AddressTypeMismatch);
        };

        {
            let mut address = $target.write();
            if address.is_some() {
                return Err(<$err_type>::AlreadyBound);
            }
            address.replace(addr);
        }

        addr
    }};
}

pub mod ipv4;
pub mod ipv6;
pub mod unix;

pub type IPv4Sock<T = dyn SockProtocol> = Sock<ipv4::IPv4<T>>;
pub type IPv6Sock<T = dyn SockProtocol> = Sock<ipv6::IPv6<T>>;
pub type UnixSock<T = dyn SockProtocol> = Sock<unix::Unix<T>>;

pub use ipv4::IPv4;

/// 将 sockaddr 复制到用户侧，如果 `addr` 为空则直接返回 0。
pub(super) fn copy_sockaddr_to_user(
    addr: *mut u8,
    addrlen: *mut u32,
    domain: Domain,
    sockaddr: &(impl ?Sized + bytemuck::NoUninit),
) -> SyscallResult {
    if addr.is_null() {
        return SyscallResult::success(0);
    }

    let domain_size = size_of_val(&domain);
    let buffer_size = copy_one_from_user(addrlen)? as usize;
    if buffer_size < domain_size {
        todo!();
    }

    copy_one_to_user(addr.cast(), &(domain as u16))?;

    let addr = unsafe { addr.byte_add(domain_size) };
    let buffer_rest = buffer_size - domain_size;

    let value_size = size_of_val(sockaddr);
    let actual_copied = min(buffer_rest, value_size);

    copy_to_user(
        addr as *mut u8,
        &bytemuck::cast_slice(slice::from_ref(sockaddr))[..actual_copied],
    )?;

    copy_one_to_user(addrlen, &(value_size as _))?;
    SyscallResult::success(0)
}
