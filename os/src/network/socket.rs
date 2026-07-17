use alloc::boxed::Box;

use crate::{
    network::sock::{Sock, get_sock_ops},
    syscall::net::{Domain, IPv4SockAddr, IPv6SockAddr, Protocol, SocketType},
};

#[derive(Debug, Clone, Copy)]
pub enum SocketAddress {
    IPv4(IPv4SockAddr),
    IPv6(IPv6SockAddr),
    Unix(!),
}

#[derive(Debug)]
pub struct Socket {
    pub domain: Domain,
    pub socket_type: SocketType,

    pub sock: Box<Sock>,
}

impl Socket {
    pub fn new(
        domain: Domain,
        socket_type: SocketType,
        protocol: Protocol,
    ) -> Result<Self, NewSocketError> {
        verify_protocol(socket_type, protocol)?;

        Ok(Self {
            domain,
            socket_type,

            sock: {
                use Domain::*;
                use Protocol::*;
                use SocketType::*;

                use super::sock::domain;
                use super::sock::protocol as proto;

                let ops = get_sock_ops(domain, socket_type, protocol);

                match (domain, socket_type, protocol) {
                    (INET, STREAM, TCP | IP) => {
                        Sock::new(ops, domain::IPv4::new(proto::Tcp::new()))
                    }
                    (INET, DGRAM, UDP | IP) => Sock::new(ops, domain::IPv4::new(proto::Udp::new())),

                    (d, s, p) => {
                        log::error!("不支持的组合：{d:?}, {s:?}, {p:?}");
                        return Err(NewSocketError::InvalidProtocol); // FIXME
                    }
                }
            },
        })
    }
}

fn verify_protocol(socket_type: SocketType, protocol: Protocol) -> Result<(), NewSocketError> {
    macro_rules! rules {
        ($($type:pat => $protocols:pat),+ $(,)?) => {
            match socket_type {
                $($type => if matches!(protocol, $protocols) { Ok(()) } else { Err(NewSocketError::InvalidProtocol) })+
            }
        };
    }

    use Protocol::*;
    use SocketType::*;

    rules! {
        STREAM | SEQPACKET | RAW => TCP | IP | SCTP | ETH_P_ALL,
        DGRAM                    => UDP | IP | UDPLITE,
    }
}

pub enum NewSocketError {
    InvalidProtocol,
}
