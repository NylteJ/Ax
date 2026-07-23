pub mod listen;
pub mod namespace;
pub mod sock;
pub mod socket;

fn is_addr_unspecified(addr: core::net::Ipv6Addr) -> bool {
    addr.is_unspecified()
        || addr
            .to_ipv4_mapped()
            .is_some_and(|addr| addr.is_unspecified())
}

fn to_v6_addr(addr: smoltcp::wire::IpAddress) -> core::net::Ipv6Addr {
    match addr {
        smoltcp::wire::IpAddress::Ipv4(ipv4_addr) => ipv4_addr.to_ipv6_mapped(),
        smoltcp::wire::IpAddress::Ipv6(ipv6_addr) => ipv6_addr,
    }
}
