use core::{
    net::{Ipv4Addr, Ipv6Addr},
    num::NonZero,
    ptr::NonNull,
};

use hashbrown::{HashMap, hash_map};

use crate::{
    basic::{
        UnsafeInited,
        id_allocator::IdAllocator,
        sync::{SpinMutex, SpinRwLock},
    },
    multitask::wait::WaitQueue,
    network::{is_addr_unspecified, sock::Sock},
};

#[derive(Hash, PartialEq, Eq)]
struct BindingKey {
    addr: Ipv6Addr,
    port: u16,
}

#[derive(Hash, PartialEq, Eq, Debug)]
struct EstablishedTuple {
    local_addr: Ipv6Addr,
    local_port: u16,
    remote_addr: Ipv6Addr,
    remote_port: u16,
}

// [EPHEMERAL_PORT_START, EPHEMERAL_PORT_END]
pub const EPHEMERAL_PORT_START: u16 = 49152;
pub const EPHEMERAL_PORT_END: u16 = 65535;

const EPHEMERAL_PORT_COUNT: usize = (EPHEMERAL_PORT_END - EPHEMERAL_PORT_START + 1) as usize;

pub struct NetworkNamespace {
    bindings: SpinRwLock<HashMap<BindingKey, NonNull<Sock>>>,
    specified_counts: SpinRwLock<HashMap<u16, NonZero<u32>>>,

    /// 已经建立的 TCP sockets
    established: SpinRwLock<HashMap<EstablishedTuple, NonNull<Sock>>>,

    ephemeral_port_allocator_raw: SpinMutex<IdAllocator<EPHEMERAL_PORT_COUNT>>,

    pub wait_queue: WaitQueue, // FIXME: 这是临时的
}

unsafe impl Send for NetworkNamespace {}
unsafe impl Sync for NetworkNamespace {}

impl NetworkNamespace {
    pub fn new() -> Self {
        Self {
            bindings: SpinRwLock::new(HashMap::new()),
            specified_counts: SpinRwLock::new(HashMap::new()),

            established: SpinRwLock::new(HashMap::new()),
            ephemeral_port_allocator_raw: SpinMutex::new(IdAllocator::new()),

            wait_queue: WaitQueue::new(),
        }
    }

    /// 将 IPv4 / IPv6 地址和端口绑定到套接字上。
    ///
    /// 对于 IPv4 地址，应当使用 ::ffff:a.b.c.d 的 IPv6 映射地址。
    ///
    /// # Safety
    /// 必须在 `sock` 被销毁前调用 [`unbind_socket`][`Self::unbind_socket`] 来解除绑定。
    pub unsafe fn bind_socket(
        &self,
        addr: Ipv6Addr,
        port: u16,
        sock: &Sock,
    ) -> Result<(), BindSocketError> {
        let key = BindingKey { addr, port };

        let mut bindings = self.bindings.write();
        let hash_map::Entry::Vacant(vacant) = bindings.entry(key) else {
            return Err(BindSocketError::AddressAlreadyInUse);
        };

        if addr.is_unspecified()
            || addr
                .to_ipv4_mapped()
                .is_some_and(|addr| addr.is_unspecified())
        {
            // 通配符
            if self.specified_counts.read().contains_key(&port) {
                return Err(BindSocketError::AddressAlreadyInUse);
            }
        } else {
            // 通常地址
            match self.specified_counts.write().entry(port) {
                hash_map::Entry::Occupied(mut occupied) => {
                    let cnt = occupied.get_mut();
                    *cnt = cnt.checked_add(1).expect("u32 都不够用那也是神了");
                }
                hash_map::Entry::Vacant(vacant) => {
                    vacant.insert_entry(NonZero::new(1).unwrap());
                }
            }
        }

        vacant.insert_entry(NonNull::from_ref(sock));
        Ok(())
    }

    pub fn unbind_socket(&self, addr: Ipv6Addr, port: u16) {
        let removed = self.bindings.write().remove(&BindingKey { addr, port });
        debug_assert!(removed.is_some(), "尝试解绑未绑定的套接字 {addr}:{port}");
    }

    /// 获取绑定到给定地址和端口的 [`Sock`]，注意返回 [`Sock`] 的地址不一定和传入参数完全一致，因为存在通配符。
    // FIXME: 这步不安全（返回的引用的生命周期过长了）
    pub fn get_bound_sock(&self, addr: Ipv6Addr, port: u16) -> Option<&Sock> {
        debug_assert!(!is_addr_unspecified(addr));

        let bindings = self.bindings.read();
        let ptr = bindings
            .get(&BindingKey { addr, port })
            .or_else(|| {
                bindings.get(&BindingKey {
                    addr: Ipv6Addr::UNSPECIFIED,
                    port,
                })
            })
            .or_else(|| {
                bindings.get(&BindingKey {
                    addr: Ipv4Addr::UNSPECIFIED.to_ipv6_mapped(),
                    port,
                })
            })
            .cloned()?;

        Some(unsafe { ptr.as_ref() })
    }

    // FIXME: 这步不安全（返回的引用的生命周期过长了）
    pub fn get_established_sock(
        &self,
        local_addr: Ipv6Addr,
        local_port: u16,
        remote_addr: Ipv6Addr,
        remote_port: u16,
    ) -> Option<&Sock> {
        debug_assert!(!is_addr_unspecified(local_addr));
        debug_assert!(!is_addr_unspecified(remote_addr));

        let tuple = EstablishedTuple {
            local_addr,
            local_port,
            remote_addr,
            remote_port,
        };
        let ptr = *self.established.read().get(&tuple)?;
        Some(unsafe { ptr.as_ref() })
    }

    /// 将 IPv4 / IPv6 地址和端口四元组绑定到 TCP 套接字上。
    ///
    /// 对于 IPv4 地址，应当使用 ::ffff:a.b.c.d 的 IPv6 映射地址。
    ///
    /// # Safety
    /// 必须在 `sock` 被销毁前调用 [`unregister_established`][`Self::unregister_established`] 来解除绑定。且此四元组必须处于未绑定状态。
    pub unsafe fn register_established(
        &self,
        sock: &Sock,
        local_addr: Ipv6Addr,
        local_port: u16,
        remote_addr: Ipv6Addr,
        remote_port: u16,
    ) {
        debug_assert!(!is_addr_unspecified(local_addr));
        debug_assert!(!is_addr_unspecified(remote_addr));
        debug_assert!(
            self.get_bound_sock(local_addr, local_port).is_some(),
            "TCP 套接字必须已经绑定到本地地址和端口（无论显式还是隐式）"
        );

        let tuple = EstablishedTuple {
            local_addr,
            local_port,
            remote_addr,
            remote_port,
        };
        let mut established = self.established.write();
        debug_assert!(!established.contains_key(&tuple));
        unsafe { established.insert_unique_unchecked(tuple, NonNull::from(sock)) };
    }

    pub fn unregister_established(
        &self,
        local_addr: Ipv6Addr,
        local_port: u16,
        remote_addr: Ipv6Addr,
        remote_port: u16,
    ) {
        debug_assert!(!is_addr_unspecified(local_addr));
        debug_assert!(!is_addr_unspecified(remote_addr));

        let tuple = EstablishedTuple {
            local_addr,
            local_port,
            remote_addr,
            remote_port,
        };
        let removed = self.established.write().remove(&tuple);
        debug_assert!(removed.is_some(), "尝试注销未注册的 TCP 套接字四元组 {tuple:?}");
    }

    pub fn new_ephemeral_port(&self) -> Option<u16> {
        let id = self.ephemeral_port_allocator_raw.lock().allocate()?;
        Some(EPHEMERAL_PORT_START + id as u16)
    }

    // TODO: 释放临时端口
}

pub enum BindSocketError {
    AddressAlreadyInUse,
}

// TODO: 如果未来要支持命名空间，这里要改
pub static GLOBAL_NET_NAMESPACE: UnsafeInited<NetworkNamespace> = unsafe { UnsafeInited::uninit() };
pub fn init() {
    unsafe { UnsafeInited::init_unsafe(&GLOBAL_NET_NAMESPACE, NetworkNamespace::new()) };
}
