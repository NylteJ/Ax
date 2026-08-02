pub mod listen;
pub mod namespace;
pub mod sock;
pub mod socket;

use core::sync::atomic::{AtomicUsize, Ordering};

/// 网络栈全局互斥锁（同一任务可重入）。
///
/// 网络栈内部的自旋锁（各 socket 的 `smol_socket`、`network_lo_iface`、`network_lo`）之间
/// 存在跨锁序依赖：例如 `connect` 会持有己方 socket 写锁调用 `do_egress`（其内部获取 iface 锁，
/// 并经由 `ingress_deliver` 获取对端 socket 写锁），而另一核上的 `do_egress` 可能正持有 iface
/// 锁等待该 socket 写锁——形成 ABBA 死锁。多核抢占（RISC-V 次要核定时器）启用后并发概率大增，
/// 实测必现。
///
/// 因此所有网络操作（socket 相关系统调用、`do_egress`、socket 销毁时的 FIN 发送与解绑）都必须
/// 持有本锁，将全部网络锁纳入单一全局顺序，消除任何环。允许同一任务嵌套获取（例如 `close`
/// 触发 `Tcp::drop` → `do_egress`，而 `do_egress` 也可能在已持锁的网络系统调用内部被调用）。
/// 内核态无抢占，任务持锁期间不可能被切换走，用任务指针（tp）作为持有者标识是可靠的。
pub static NET_LOCK: ReentrantSpinLock = ReentrantSpinLock::new();

pub struct ReentrantSpinLock {
    /// 持有者的任务指针（tp 值）；`usize::MAX` 表示空闲。
    owner: AtomicUsize,
    depth: AtomicUsize,
}

impl ReentrantSpinLock {
    pub const fn new() -> Self {
        Self {
            owner: AtomicUsize::new(usize::MAX),
            depth: AtomicUsize::new(0),
        }
    }

    #[inline]
    pub fn lock(&self) {
        use crate::arch::interfaces::Regs;
        let my_task = crate::arch::NativeRegs::get_tp() as usize;
        loop {
            if self.owner.load(Ordering::Relaxed) == my_task {
                self.depth.fetch_add(1, Ordering::Relaxed);
                return;
            }
            if self
                .owner
                .compare_exchange(usize::MAX, my_task, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.depth.store(1, Ordering::Relaxed);
                return;
            }
            core::hint::spin_loop();
        }
    }

    #[inline]
    pub fn unlock(&self) {
        if self.depth.fetch_sub(1, Ordering::Relaxed) == 1 {
            self.owner.store(usize::MAX, Ordering::Release);
        }
    }

    pub fn lock_guard(&self) -> NetLockGuard<'_> {
        self.lock();
        NetLockGuard { lock: self }
    }
}

pub struct NetLockGuard<'a> {
    lock: &'a ReentrantSpinLock,
}

impl Drop for NetLockGuard<'_> {
    fn drop(&mut self) {
        self.lock.unlock();
    }
}

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
