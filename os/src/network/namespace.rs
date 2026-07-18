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
        sync::{Mutex, RwLock},
    },
    multitask::wait::WaitQueue,
    network::{is_addr_unspecified, sock::Sock},
};

#[derive(Hash, PartialEq, Eq)]
struct BindingKey {
    addr: Ipv6Addr,
    port: u16,
}

// [EPHEMERAL_PORT_START, EPHEMERAL_PORT_END]
pub const EPHEMERAL_PORT_START: u16 = 49152;
pub const EPHEMERAL_PORT_END: u16 = 65535;

const EPHEMERAL_PORT_COUNT: usize = (EPHEMERAL_PORT_END - EPHEMERAL_PORT_START + 1) as usize;

pub struct NetworkNamespace {
    bindings: RwLock<HashMap<BindingKey, NonNull<Sock>>>,
    specified_counts: RwLock<HashMap<u16, NonZero<u32>>>,

    ephemeral_port_allocator_raw: Mutex<IdAllocator<EPHEMERAL_PORT_COUNT>>,

    pub wait_queue: WaitQueue, // FIXME: 这是临时的
}

unsafe impl Send for NetworkNamespace {}
unsafe impl Sync for NetworkNamespace {}

impl NetworkNamespace {
    pub fn new() -> Self {
        Self {
            bindings: RwLock::new(HashMap::new()),
            specified_counts: RwLock::new(HashMap::new()),
            ephemeral_port_allocator_raw: Mutex::new(IdAllocator::new()),

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

    pub fn get_sock(&self, addr: Ipv6Addr, port: u16) -> Option<&Sock> {
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
