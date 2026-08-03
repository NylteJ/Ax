pub mod listen;
pub mod namespace;
pub mod sock;
pub mod socket;

use crate::basic::sync::SpinMutex;

/// 网络操作全局锁，将网络栈内所有自旋锁（各 socket 的 `smol_socket`、
/// `network_lo_iface`、`network_lo`）纳入单一全局顺序，消除 ABBA 死锁。
///
/// 此锁仅由 `do_egress`、`Tcp::drop`、`IPv4::drop`、`IPv6::drop` 等底层路径
/// 获取；系统调用层不持有此锁，故同一任务不会在同一调用栈上重复获取（无重入）。
/// 内核态不抢占（`trap_handler` 在来自内核态的时钟中断时不执行调度），因此
/// 持锁的任务不会被切走，其它核上等待此锁的任务最多自旋若干微秒。
pub static NET_LOCK: SpinMutex<()> = SpinMutex::new(());

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
