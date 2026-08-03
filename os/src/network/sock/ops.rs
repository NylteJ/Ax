use core::{
    cmp::min,
    hint::unreachable_unchecked,
    net::{Ipv4Addr, Ipv6Addr, SocketAddrV4},
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
    wire::{IpAddress, IpEndpoint},
};

use crate::{
    memory::uaccess::{copy_from_user, copy_to_user},
    network::{
        is_addr_unspecified,
        namespace::GLOBAL_NET_NAMESPACE,
        sock::{
            Sock, SockDomain,
            domain::{IPv4, copy_sockaddr_to_user},
            protocol::Tcp,
        },
        socket::SocketAddress,
        to_v6_addr,
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

pub(crate) fn do_egress(socket: SocketMRef<'_, '_>, established_tcp: bool) -> Option<&'static Sock> {
    use crate::{
        basic::timer::{cpu_tick_to_us, get_cpu_tick},
        devices::DEVICES,
    };
    use smoltcp::{
        iface::{EgressError, IngressPacket, Meta, PollIngressSingleResult},
        time::Instant,
    };

    debug_assert!(!established_tcp || matches!(&socket, SocketMRef::Tcp(_)));

    // NET_LOCK 由调用方持有（系统调用处理器或 Tcp::drop），此处不再获取

    let mut iface = DEVICES.network_lo_iface.lock();
    let mut lo = DEVICES.network_lo.lock();
    let lo = &mut *lo;

    let mut meta = Meta::new();

    // FIXME: 未来添加非回环设备后这里应当更改

    let egress_result = iface.poll_egress_single(lo, socket, &mut meta);
    match egress_result {
        Ok(()) => (),
        Err(EgressError::Exhausted) => return None,
        Err(EgressError::NeighborPending) => todo!(),
    }

    let _ = meta; // FIXME?

    let current_us = cpu_tick_to_us(get_cpu_tick());
    let timestamp = Instant::from_micros(current_us as i64 / 1000);
    let mut packet = IngressPacket::new();
    let ingress1_result = iface.poll_ingress_read(timestamp, lo, &mut packet);
    match ingress1_result {
        PollIngressSingleResult::None => {
            log::warn!("poll_ingress_read 没有返回数据包，原因可能是 egress 选择不实际执行发送");
            return None;
        },
        PollIngressSingleResult::PacketProcessed => todo!(),
        PollIngressSingleResult::SocketStateChanged => (),
    }

    let dst_addr = to_v6_addr(packet.ip_repr.dst_addr());
    let dst_port = packet.dst_port();

    let dst_sock = if established_tcp {
        let src_addr = to_v6_addr(packet.ip_repr.src_addr());
        let src_port = packet.src_port();

        GLOBAL_NET_NAMESPACE.get_established_sock(dst_addr, dst_port, src_addr, src_port)
    } else {
        GLOBAL_NET_NAMESPACE.get_bound_sock(dst_addr, dst_port)
    };

    let Some(dst_sock) = dst_sock else {
        log::warn!("没有为数据包找到目标地址（{dst_addr}:{dst_port}）");
        log::warn!("对端可能已被销毁；丢弃数据包");
        return None;
    };

    dst_sock.ops.ingress_deliver(dst_sock, &mut |socket| {
        iface.poll_ingress_deliver(lo, &packet, socket)
    });

    GLOBAL_NET_NAMESPACE.wait_queue.wake_all();

    use smoltcp::phy::Device;
    debug_assert!(lo.receive(Instant::ZERO).is_none());
    Some(dst_sock)
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
    private: {
        /// 立即从 smoltcp 的 backlog 中取出所有完成握手的子 socket，
        /// 包装为内核 Sock，注册到 established 表并放入 accept_queue。
        fn eager_accept_children(this: &This) {
            let tcp = &this.data.proto;
            let mut smol_socket = tcp.smol_socket.write();
            let mut extracted = alloc::vec::Vec::new();
            while let Some(child) = smol_socket.accept() {
                extracted.push(child);
            }
            drop(smol_socket);

            for mut child in extracted {
                // 扩展缓冲区，与 Tcp::new() 中的设置保持一致
                child.set_rx_max_capacity(super::protocol::tcp::TCP_MAX_RX_BUFFER_BYTES);
                child.set_tx_max_capacity(super::protocol::tcp::TCP_MAX_TX_BUFFER_BYTES);

                let local = child.local_endpoint().unwrap();
                let remote = child.remote_endpoint().unwrap();

                let smoltcp::wire::IpAddress::Ipv4(local_addr) = local.addr else {
                    todo!("IPv6")
                };
                let smoltcp::wire::IpAddress::Ipv4(remote_addr) = remote.addr else {
                    todo!("IPv6")
                };

                let local_sockaddr = IPv4SockAddr {
                    port: local.port.into(),
                    addr: u32::from_be_bytes(local_addr.octets()).into(),
                    _zero: Default::default(),
                };
                let remote_sockaddr = IPv4SockAddr {
                    port: remote.port.into(),
                    addr: u32::from_be_bytes(remote_addr.octets()).into(),
                    _zero: Default::default(),
                };

                let mut data = IPv4::new(Tcp::with_smol_socket(child));
                data.is_accepted = true; // 与监听 socket 共享端口绑定，Drop 时不调用 unbind_socket
                *data.address.write() = Some(local_sockaddr);
                *data.peer_address.write() = Some(remote_sockaddr);

                let new_sock = Sock::new(my_ops!(), data);

                unsafe {
                    GLOBAL_NET_NAMESPACE.register_established(
                        &new_sock,
                        local_addr.to_ipv6_mapped(),
                        local.port,
                        remote_addr.to_ipv6_mapped(),
                        remote.port,
                    );
                }

                tcp.push_accept(new_sock);
            }
        }
    }
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
            // 先尝试从 smoltcp backlog 中取出可能已完成的连接
            //（处理 accept 在 ingress_deliver 之前被调用的情况）
            eager_accept_children(this);

            // 从内核的 accept 队列中取出
            this.data.proto.pop_accept().ok_or(E_SPECIAL_NET_RETRY)
        }

        fn connect(&this) -> SyscallResult {
            let local_saddr = if let Some(local_addr) = { *this.data.address.read() } {
                local_addr
            } else {
                let v4addr = get_emphemeral_v4addr()?;
                let addr = SocketAddress::IPv4(v4addr);
                this.data.bind_part_1(addr, this).expect("TODO");
                my_ops!().bind_part_2(this).unwrap();

                v4addr
            };

            let mut local_v4addr = local_saddr.ipv4_addr();
            if local_v4addr.is_unspecified() {
                local_v4addr = Ipv4Addr::LOCALHOST;
            }
            let local_port = local_saddr.port.get();
            let local_endpoint = SocketAddrV4::new(local_v4addr, local_port);

            let remote_addr = this.data.peer_address.read().expect("part 1 会完成这部分");

            // FIXME: 支持非回环设备后这里要改
            let mut smol_socket = this.data.proto.smol_socket.write();

            let connect_result = smol_socket.connect(
                crate::devices::DEVICES.network_lo_iface.lock().context(),
                remote_addr.to_endpoint(),
                local_endpoint,
            );

            match connect_result {
                Ok(()) => (),
                Err(ConnectError::InvalidState) => return EISCONN,
                Err(ConnectError::Unaddressable) => return EINVAL, // 对端端口为 0
            }

            debug_assert_eq!(smol_socket.state(), smoltcp::socket::tcp::State::SynSent);

            // 1. 我方首先发送 SYN，同时对端接收到 SYN
            let target_sock =
                match do_egress(SocketMRef::Tcp(&mut smol_socket), false) {
                    Some(sock) => sock,
                    None => {
                        // SYN failed to send; abort the socket to reset SynSent state
                        // so a retry can attempt connect() from a clean state
                        smol_socket.abort();
                        return EAGAIN;
                    }
                };
            let target_sock = try_cast_to_this(target_sock).expect("do_egress 应当验证这一步");
            let mut target_smol_socket = target_sock.data.proto.smol_socket.write();

            debug_assert_eq!(target_smol_socket.state(), smoltcp::socket::tcp::State::Listen);

            // 2. 对端发送 SYN+ACK，我方接收到 SYN+ACK 并变为 Established
            drop(smol_socket);
            let this_sock = do_egress(SocketMRef::Tcp(&mut target_smol_socket), false);
            debug_assert!(this_sock.is_some_and(|s| core::ptr::addr_eq(s, this)));
            let mut smol_socket = this.data.proto.smol_socket.write();
            debug_assert_eq!(smol_socket.state(), smoltcp::socket::tcp::State::Established);

            // 3. 我方发送 ACK，对端接收到 ACK 并变为 Established
            //    对端（子 socket）通过 ingress_deliver 中的 eager accept 自动提取、
            //    注册到 established 表并放入 accept_queue；无需再手动注册。
            drop(target_smol_socket);
            let target_sock_ = do_egress(SocketMRef::Tcp(&mut smol_socket), false);
            debug_assert!(target_sock_.is_some_and(|s| core::ptr::addr_eq(s, target_sock)));

            // 4. 将变为 Established 的我方注册到哈希表中
            let local_addr = local_v4addr.to_ipv6_mapped();
            let remote_port = remote_addr.port.get();
            let remote_addr = remote_addr.ipv4_addr().to_ipv6_mapped();
            unsafe {
                GLOBAL_NET_NAMESPACE.register_established(
                    this,
                    local_addr,
                    local_port,
                    remote_addr,
                    remote_port,
                )
            };

            // debug_assert_eq!(smol_socket.local_endpoint(), this.data.address.read().map(|a| a.to_endpoint()));
            // debug_assert_eq!(smol_socket.remote_endpoint(), this.data.peer_address.read().map(|a| a.to_endpoint()));
            // debug_assert_eq!(target_sock.data.proto.smol_socket.read().local_endpoint(), target_sock.data.address.read().map(|a| a.to_endpoint()));
            // debug_assert_eq!(target_sock.data.proto.smol_socket.read().remote_endpoint(), target_sock.data.peer_address.read().map(|a| a.to_endpoint()));

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
                // if copy_result.is_ok() {
                //     log::trace!("recved: {:?}", crate::basic::bstr::ByteStr::from_bytes(&data[..size]));
                // }
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
                    // 交替在己方和对端之间调用 do_egress：
                    // 己方发送数据段 → 回环 → 对端接收并生成 ACK →
                    // 对端发送 ACK → 回环 → 己方接收 ACK、释放缓冲区。
                    // 若不发送对端的 ACK，己方缓冲区的数据永远无法被确认，
                    // 后续 send_all 将返回 BufferFull 并触发 todo!()。
                    //
                    // 注意：do_egress 内部会调用目标 socket 的 ingress_deliver，
                    // 而 ingress_deliver 会获取目标 socket 的写锁。
                    // 因此在调用 do_egress 发送对端数据前，必须释放己方的写锁，
                    // 否则对端 egress 的 ACK 回环交付给己方时会发生死锁。
                    for _ in 0..64 {
                        match do_egress(SocketMRef::Tcp(&mut smol_socket), true) {
                            Some(peer_sock) => {
                                let peer = try_cast_to_this(peer_sock).expect("对端应为 TCPv4 socket");
                                // 释放己方锁，防止对端 ingress_deliver 回写时死锁
                                drop(smol_socket);
                                let mut peer_smol = peer.data.proto.smol_socket.write();
                                let peer_has_more = do_egress(SocketMRef::Tcp(&mut peer_smol), true).is_some();
                                drop(peer_smol);
                                // 重新获取己方锁，继续下一轮
                                smol_socket = this.data.proto.smol_socket.write();
                                if !peer_has_more {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }

                    SyscallResult::success(written)
                },
                Err(SendAllError::BufferFull) => todo!(),
                Err(SendAllError::InvalidState) => ENOTCONN,
                Err(SendAllError::Custom(err)) => Err(err)?,
            }
        }

        fn ingress_deliver(&this, func: &mut dyn FnMut(SocketMRef<'_, '_>)) {
            func(SocketMRef::Tcp(&mut this.data.proto.smol_socket.write()));
            // 立即取出所有完成握手的子 socket
            eager_accept_children(this);
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
                    if do_egress(SocketMRef::Udp(&mut smol_sock_guard), false).is_none() {
                        log::warn!("UDP send: do_egress 返回 None，数据包可能未被发送");
                    }
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
