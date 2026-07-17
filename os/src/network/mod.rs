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
