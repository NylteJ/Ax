use core::{
    cmp::min,
    net::{Ipv4Addr, SocketAddrV4},
    ops::FromResidual,
    sync::atomic::Ordering,
};

use alloc::boxed::Box;
use smoltcp::{
    socket::{
        SocketMRef,
        tcp::{ConnectError, ListenError},
        udp::{SendWithError, UdpMetadata},
    },
    wire::IpAddress,
};

use crate::{
    memory::uaccess::{copy_from_user, copy_to_user},
    network::{
        namespace::GLOBAL_NET_NAMESPACE,
        sock::{
            Sock, SockDomain,
            domain::{IPv4, copy_sockaddr_to_user},
            protocol::Tcp,
        },
        socket::SocketAddress,
    },
    syscall::{
        SyscallResult,
        net::{Domain, IPv4SockAddr, Protocol, SocketType},
    },
};

// 有一说一我已经开始后悔用 Rust 了（甚至就算我们手写了虚表，DST 嵌套还是会带来额外的 drop 虚调用，神了）
macro_rules! define {
    ($($(#[doc = $doc:expr])? fn $name:ident(sock $(, $pname:ident: $ptype:ty)* $(,)?) $(-> $ret:ty)?;)*) => {
        pub struct SockOps {
            $(
                $(#[doc = $doc])?
                $name: fn (*const () $(, $ptype)*) $(-> $ret)?,
            )*
        }

        impl SockOps {
            $(
                $(#[doc = $doc])?
                pub fn $name(&self, sock: &Sock $(, $pname: $ptype)*) $(-> $ret)? {
                    let sock: *const Sock = sock;
                    let ret = (self.$name)(sock.cast() $(, $pname)*);
                    ret
                }
            )*

            #[allow(unused)]
            const ALL_UNIMPL: Self = Self {
                $($name: |this: *const () $(, $pname: $ptype)*| unimplemented!(), )*
            };
        }
    };
}

macro_rules! sock_ops_impl {
    (target $target:ty, fn $name:ident (&$this: ident $(, $pname2:ident: $ptype2:ty)* $(,)?) $(-> $ret2:ty)? { $($content:tt)* }) => {
        |this: *const () $(, $pname2: $ptype2)*| $(-> $ret2)? {
            let this = this.cast::<$target>();
            let $this = unsafe { this.as_ref_unchecked() };
            $($content)*
        }
    };
    (target $target:ty, fn $name:ident = unimplemented;) => {
        SockOps::ALL_UNIMPL.$name
    };
}

macro_rules! sock_ops {
    ($(private: { $($private:tt)* })? static $ops_name:ident = unsafe impl for $target:ty /*$(: $derived:expr)?*/ {
        $(fn $name:ident $((&$this:ident $(, $pname2:ident: $ptype2:ty)* $(,)?) $(-> $ret2:ty)? { $($content:tt)* })? $(= $unimpl:ident;)?)*
    }) => {
        static $ops_name: SockOps = const {
            #[allow(unused)]
            type This = $target;
            #[allow(unused)]
            fn try_cast_to_this(sock: &Sock) -> Option<&This> {
                if core::ptr::addr_eq(sock.ops, &$ops_name) {
                    let data_ptr = sock as *const Sock as *const This;
                    Some(unsafe { data_ptr.as_ref_unchecked() })
                } else {
                    None
                }
            }
            #[allow(unused)]
            macro_rules! my_ops {
                () => { &$ops_name };
            }

            $($($private)*)?

            SockOps {
                $(
                    $name: sock_ops_impl!(target $target, fn $name $((&$this $(, $pname2: $ptype2)*) $(-> $ret2)? { $($content)* })? $(= $unimpl;)?),
                )*

                // $(..$derived)?
            }
        };
    }
}

define! {
    fn bind_part_2(sock) -> SyscallResult;
    fn listen(sock, backlog: u32) -> SyscallResult;

    fn accept(sock) -> Result<Box<Sock>, SyscallResult>;
    fn connect(sock) -> SyscallResult;

    fn sendto(sock, buf: *const u8, size: usize, addr: SocketAddress) -> SyscallResult;
    fn recvfrom(sock, buf: *mut u8, size: usize, src_addr: *mut u8, addrlen: *mut u32) -> SyscallResult;

    fn send(sock, buf: *const u8, size: usize) -> SyscallResult;

    fn ingress_deliver(sock, func: &mut dyn FnMut(SocketMRef<'_, '_>));
}

use crate::syscall::err_codes::*;

fn do_egress(socket: SocketMRef<'_, '_>) -> Option<&'static Sock> {
    use crate::{
        basic::timer::{cpu_tick_to_us, get_cpu_tick},
        devices::DEVICES,
    };
    use smoltcp::{
        iface::{EgressError, IngressPacket, Meta, PollIngressSingleResult},
        time::Instant,
    };

    let mut iface = DEVICES.network_lo_iface.lock();
    let mut lo = DEVICES.network_lo.lock();
    let lo = &mut *lo;

    let mut meta = Meta::new();

    // FIXME: 未来添加非回环设备后这里应当更改

    let egress_result = iface.poll_egress_single(lo, socket, &mut meta);
    match egress_result {
        Ok(()) => (),
        Err(EgressError::Exhausted) => panic!("调用方应当保证 socket 有数据可发"),
        Err(EgressError::NeighborPending) => todo!(),
    }

    let _ = meta; // FIXME?

    let current_us = cpu_tick_to_us(get_cpu_tick());
    let timestamp = Instant::from_micros(current_us as i64 / 1000);
    let mut packet = IngressPacket::new();
    let ingress1_result = iface.poll_ingress_read(timestamp, lo, &mut packet);
    match ingress1_result {
        PollIngressSingleResult::None => unreachable!("我们刚刚发送了数据"),
        PollIngressSingleResult::PacketProcessed => todo!(),
        PollIngressSingleResult::SocketStateChanged => (),
    }

    let dst_addr = match packet.ip_repr.dst_addr() {
        IpAddress::Ipv4(ipv4_addr) => ipv4_addr.to_ipv6_mapped(),
        IpAddress::Ipv6(ipv6_addr) => ipv6_addr,
    };

    let dst_port = packet.dst_port();

    let Some(target_sock) = GLOBAL_NET_NAMESPACE.get_sock(dst_addr, dst_port) else {
        log::warn!("没有为数据包找到目标地址（{dst_addr}:{dst_port}）");
        log::error!("TODO: 回馈端口不可达");
        return None;
    };

    target_sock.ops.ingress_deliver(target_sock, &mut |socket| {
        iface.poll_ingress_deliver(lo, &packet, socket)
    });

    use smoltcp::phy::Device;
    debug_assert!(lo.receive(Instant::ZERO).is_none());
    Some(target_sock)
}

fn get_emphemeral_v4addr() -> Result<IPv4SockAddr, SyscallResult> {
    Ok(IPv4SockAddr {
        port: GLOBAL_NET_NAMESPACE
            .new_ephemeral_port()
            .ok_or(EAGAIN)?
            .into(),
        addr: 0.into(),
        _zero: Default::default(),
    })
}

sock_ops! {
    static TCP4_OPS = unsafe impl for super::Tcp4Sock {
        fn bind_part_2(&_this) -> SyscallResult {
            // smoltcp 的绑定操作是在 connect / listen 中进行的，因此这里什么都不用做
            SyscallResult::success(0)
        }

        fn listen(&this, backlog: u32) -> SyscallResult {
            let tcp = &this.data.proto;

            let addr = this.data.address.read().expect("TODO: autobind");

            let port = addr.port.get();
            let addr = addr.ipv4_addr();

            let listen_result = if addr.is_unspecified() {
                tcp.smol_socket.write().listen(port, backlog)
            } else {
                tcp.smol_socket.write().listen(SocketAddrV4::new(addr, port), backlog)
            };

            match listen_result {
                Ok(()) => SyscallResult::success(0),
                Err(ListenError::InvalidState) => EINVAL,
                Err(ListenError::Unaddressable) => unreachable!("外层应当处理临时端口"),
            }
        }

        fn accept(&this) -> Result<Box<Sock>, SyscallResult> {
            let new_sock = this.data.proto.smol_socket.write().accept().ok_or(E_SPECIAL_NET_RETRY)?;

            Ok(Sock::new(my_ops!(), IPv4::new(Tcp::with_smol_socket(new_sock))))
        }

        fn connect(&this) -> SyscallResult {
            let local_addr = if let Some(local_addr) = { *this.data.address.read() } {
                local_addr
            } else {
                let v4addr = get_emphemeral_v4addr()?;
                let addr = SocketAddress::IPv4(v4addr);
                this.data.bind_part_1(addr, this).expect("TODO");
                my_ops!().bind_part_2(this).unwrap();

                v4addr
            };

            let mut local_v4addr = local_addr.ipv4_addr();
            if local_v4addr.is_unspecified() {
                local_v4addr = Ipv4Addr::LOCALHOST;
            }
            let local_endpoint = SocketAddrV4::new(local_v4addr, local_addr.port.get());

            // FIXME: 支持非回环设备后这里要改
            let mut smol_socket = this.data.proto.smol_socket.write();

            let connect_result = smol_socket.connect(
                crate::devices::DEVICES.network_lo_iface.lock().context(),
                this.data.peer_address.read().expect("part 1 会完成这部分").to_endpoint(),
                local_endpoint,
            );

            match connect_result {
                Ok(()) => (),
                Err(ConnectError::InvalidState) => return EISCONN,
                Err(ConnectError::Unaddressable) => return EINVAL, // 对端端口为 0
            }

            debug_assert_eq!(smol_socket.state(), smoltcp::socket::tcp::State::SynSent);

            // 1. 我方首先发送 SYN，同时对端接收到 SYN
            let target_sock = do_egress(SocketMRef::Tcp(&mut smol_socket)).expect("TODO");
            let target_sock = try_cast_to_this(target_sock).expect("do_egress 应当验证这一步");
            let mut target_smol_socket = target_sock.data.proto.smol_socket.write();

            debug_assert_eq!(target_smol_socket.state(), smoltcp::socket::tcp::State::Listen);

            // 2. 对端发送 SYN+ACK，我方接收到 SYN+ACK 并变为 Established
            drop(smol_socket);
            let this_sock = do_egress(SocketMRef::Tcp(&mut target_smol_socket));
            debug_assert!(this_sock.is_some_and(|s| core::ptr::addr_eq(s, this)));
            let mut smol_socket = this.data.proto.smol_socket.write();
            debug_assert_eq!(smol_socket.state(), smoltcp::socket::tcp::State::Established);

            // 3. 我方发送 ACK，对端接收到 ACK 并变为 Established
            drop(target_smol_socket);
            let target_sock_ = do_egress(SocketMRef::Tcp(&mut smol_socket));
            debug_assert!(target_sock_.is_some_and(|s| core::ptr::addr_eq(s, target_sock)));

            SyscallResult::success(0)
        }

        fn sendto = unimplemented;

        fn recvfrom(
            &this,
            buf: *mut u8,
            size: usize,
            src_addr: *mut u8,
            addrlen: *mut u32,
        ) -> SyscallResult {
            let result = this.data.proto.smol_socket.write().recv(|data| {
                let size = min(size, data.len());
                let copy_result = copy_to_user(buf, &data[..size]);
                match copy_result {
                    Ok(()) => (size, Ok(size)),
                    Err(err) => (0, Err(err)),
                }
            });
            
            use smoltcp::socket::tcp::RecvError;
            let written = match result {
                Ok(Ok(0)) => return E_SPECIAL_NET_RETRY,
                Ok(Ok(written)) => written,
                Ok(Err(err)) => Err(err)?,
                Err(RecvError::InvalidState) => return ENOTCONN,
                Err(RecvError::Finished) => 0,
            };

            if !src_addr.is_null() {
                let addr = this.data.peer_address.read().expect("已经 connect 的 TCP socket 不应该没有对端地址");
                copy_sockaddr_to_user(src_addr, addrlen, Domain::INET, &addr)?;
            }
            
            SyscallResult::success(written)
        }

        fn send(&this, buf: *const u8, size: usize) -> SyscallResult {
            let mut smol_socket = this.data.proto.smol_socket.write();
            let send_result = smol_socket.send_all(size, |buffer| {
                copy_from_user(buffer, buf).map(|_| buffer.len())
            });

            use smoltcp::socket::tcp::SendAllError;
            match send_result {
                Ok(written) => {
                    do_egress(SocketMRef::Tcp(&mut smol_socket));

                    SyscallResult::success(written)
                },
                Err(SendAllError::BufferFull) => todo!(),
                Err(SendAllError::InvalidState) => ENOTCONN,
                Err(SendAllError::Custom(err)) => Err(err)?,
            }
        }

        fn ingress_deliver(&this, func: &mut dyn FnMut(SocketMRef<'_, '_>)) {
            func(SocketMRef::Tcp(&mut this.data.proto.smol_socket.write()));
        }
    }
}

sock_ops! {
    static UDP4_OPS = unsafe impl for super::Udp4Sock {
        fn bind_part_2(&this) -> SyscallResult {
            let smol_socket = &this.data.proto.smol_socket;
            let address = this.data.address.read().expect("part 1 做了这一步");
            let endpoint = address.to_endpoint();
            smol_socket.write().bind(endpoint).expect("part 1 排除了多次绑定，同时外层应当处理临时端口");
            SyscallResult::success(0)
        }

        fn listen = unimplemented;
        fn accept = unimplemented;

        fn connect(&_this) -> SyscallResult {
            // 无事可做
            SyscallResult::success(0)
        }

        fn sendto(&this, buf: *const u8, size: usize, addr: SocketAddress) -> SyscallResult {
            let SocketAddress::IPv4(addr) = addr else {
                return EAFNOSUPPORT;
            };

            let smol_sock = &this.data.proto.smol_socket;

            let local_addr = if let Some(local_addr) = { *this.data.address.read() } {
                local_addr
            } else {
                let v4addr = get_emphemeral_v4addr()?;
                let addr = SocketAddress::IPv4(v4addr);
                this.data.bind_part_1(addr, this).expect("TODO");
                my_ops!().bind_part_2(this).unwrap();

                v4addr
            };
            let mut local_v4addr = local_addr.ipv4_addr();
            if local_v4addr.is_unspecified() {
                local_v4addr = Ipv4Addr::LOCALHOST; // 不必担心冲突，因为 0.0.0.0 与同端口上的任意地址互斥
            }
            let local_address = IpAddress::Ipv4(local_v4addr);

            let send_result = {
                let mut smol_sock_guard = smol_sock.write();
                let result = smol_sock_guard.send_with(
                    size,
                    UdpMetadata {
                        endpoint: addr.to_endpoint(),
                        local_address: Some(local_address),
                        meta: Default::default(),
                    },
                    |buffer| {
                        debug_assert_eq!(buffer.len(), size);
                        copy_from_user(buffer, buf).map(|()| buffer.len())
                    },
                );

                if result.is_ok() {
                    do_egress(SocketMRef::Udp(&mut smol_sock_guard));
                }

                result
            };

            match send_result {
                Ok(written) => SyscallResult::success(written),
                Err(SendWithError::Unaddressable) => EINVAL,
                Err(SendWithError::BufferFull) => E_SPECIAL_NET_RETRY,
                Err(SendWithError::Custom(err)) => Err(err)?,
            }
        }

        fn recvfrom(
            &this,
            buf: *mut u8,
            size: usize,
            src_addr: *mut u8,
            addrlen: *mut u32,
        ) -> SyscallResult {
            let mut socket_guard = this.data.proto.smol_socket.write();
            let (data, metadata) = {
                let result = socket_guard.recv();
                result.map_err(|err| {
                    use smoltcp::socket::udp::RecvError;
                    match err {
                        RecvError::Exhausted => E_SPECIAL_NET_RETRY,
                        RecvError::Truncated => unreachable!("smoltcp 不会返回它"),
                    }
                })?
            };

            if !src_addr.is_null() {
                let IpAddress::Ipv4(addr) = metadata.endpoint.addr else {
                    unreachable!();
                };

                let addr = IPv4SockAddr {
                    addr: addr.octets().into(),
                    port: metadata.endpoint.port.into(),
                    _zero: Default::default(),
                };

                copy_sockaddr_to_user(src_addr, addrlen, Domain::INET, &addr)?;
            }

            let actual_copied = min(size, data.len());

            copy_to_user(buf, &data[..actual_copied])?; // 剩余部分直接静默丢弃，神奇吧
            SyscallResult::success(actual_copied)
        }

        fn send(&this, buf: *const u8, size: usize) -> SyscallResult {
            let addr = this.data.peer_address.read().ok_or(ENOTCONN)?;
            my_ops!().sendto(this, buf, size, SocketAddress::IPv4(addr))
        }

        fn ingress_deliver(&this, func: &mut dyn FnMut(SocketMRef<'_, '_>)) {
            func(SocketMRef::Udp(&mut this.data.proto.smol_socket.write()));
        }
    }
}

pub fn get_sock_ops(
    domain: Domain,
    socket_type: SocketType,
    protocol: Protocol,
) -> &'static SockOps {
    use Domain::*;
    use Protocol::*;
    use SocketType::*;

    match (domain, socket_type, protocol) {
        (INET, STREAM, TCP | IP) => &TCP4_OPS,
        (INET, DGRAM, UDP | IP) => &UDP4_OPS,

        (domain, socket_type, protocol) => {
            log::error!("不支持的组合：{domain:?}, {socket_type:?}, {protocol:?}");
            &SockOps::ALL_UNIMPL
        }
    }
}
