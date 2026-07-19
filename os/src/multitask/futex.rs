use core::{
    array,
    hash::{Hash, Hasher},
    mem::discriminant,
    ptr::{NonNull, addr_eq},
    sync::atomic::AtomicU32,
};

use alloc::{boxed::Box, sync::Arc};
use intrusive_collections::{LinkedListLink, intrusive_adapter};

use crate::{
    basic::{
        UnsafeInited,
        sync::{SpinMutex, SpinMutexGuard, SpinRwLock},
        timer::{get_cpu_tick, wake_task_when},
    },
    filesystem::vfs::INodeArc,
    memory::{
        address::PhysAddr,
        address_space::AddressSpace,
        uaccess::{CopyError, copy_one_from_user, copy_one_to_user},
    },
    multitask::{cur_task, task::Task},
};

// 我们不会真正用它访问任何东西，它永远只用于比较，因此它只要能唯一确定一个 Futex 就行了
#[derive(Debug, Clone)]
pub enum FutexKey {
    // 此处的 Arc 是为防止 ABA 问题所必须的
    Private {
        addr_space: Arc<SpinRwLock<AddressSpace>>,
        uaddr: NonNull<AtomicU32>,
    },
    SharedFile {
        inode: INodeArc,
        offset: u64,
    },
    SharedAnon {
        addr: PhysAddr,
    }, // 反正我们也不支持页面交换 :P
}
unsafe impl Send for FutexKey {}
unsafe impl Sync for FutexKey {}

impl PartialEq for FutexKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                FutexKey::Private {
                    addr_space: as1,
                    uaddr: ua1,
                },
                FutexKey::Private {
                    addr_space: as2,
                    uaddr: ua2,
                },
            ) => Arc::ptr_eq(as1, as2) && ua1 == ua2,
            (
                FutexKey::SharedFile {
                    inode: i1,
                    offset: o1,
                },
                FutexKey::SharedFile {
                    inode: i2,
                    offset: o2,
                },
            ) => i1.addr_eq(i2) && o1 == o2,
            (FutexKey::SharedAnon { addr: a1 }, FutexKey::SharedAnon { addr: a2 }) => a1 == a2,
            _ => false,
        }
    }
}

impl Eq for FutexKey {}

impl Hash for FutexKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        discriminant(self).hash(state);
        match self {
            FutexKey::Private { addr_space, uaddr } => {
                Arc::as_ptr(addr_space).hash(state);
                uaddr.hash(state);
            }
            FutexKey::SharedFile { inode, offset } => {
                inode.addr().hash(state);
                offset.hash(state);
            }
            FutexKey::SharedAnon { addr } => {
                addr.hash(state);
            }
        }
    }
}

struct FutexHasher(u64);

impl Hasher for FutexHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // 基本上是你能找到的最烂的实现
        // 很遗憾但这就是 Rust
        for byte in bytes {
            self.0 = self.0.wrapping_add(*byte as u64);
        }
    }
}

impl FutexKey {
    fn bucket_index(&self) -> usize {
        let mut hasher = FutexHasher(0);
        self.hash(&mut hasher);
        hasher.finish() as usize % FUTEX_BUCKET_COUNT
    }
}

pub type FutexMask = u32;

#[derive(Debug)]
struct FutexQ {
    key: FutexKey,
    task: *const Task,
    link: LinkedListLink,
    mask: FutexMask,
}
unsafe impl Send for FutexQ {}
unsafe impl Sync for FutexQ {}

intrusive_adapter!(FutexQAdapter = Box<FutexQ>: FutexQ { link => LinkedListLink });

type FutexQueue = intrusive_collections::LinkedList<FutexQAdapter>;

const FUTEX_BUCKET_COUNT: usize = 256;

static FUTEX_BUCKETS: UnsafeInited<[SpinMutex<FutexQueue>; FUTEX_BUCKET_COUNT]> =
    unsafe { UnsafeInited::uninit() };

pub fn init() {
    unsafe {
        UnsafeInited::init_unsafe(
            &FUTEX_BUCKETS,
            array::from_fn(|_| SpinMutex::new(FutexQueue::new(FutexQAdapter::new()))),
        );
    }
}

pub struct FutexGuard<'a> {
    queue: SpinMutexGuard<'a, FutexQueue>,
    key: &'a FutexKey,
}

impl FutexGuard<'_> {
    pub fn relaxed_atom_store(&self, addr: *mut u32, val: u32) -> Result<(), CopyError> {
        // FIXME: 应为原子操作
        debug_assert!(addr.is_aligned_to(4));
        copy_one_to_user(addr, &val)
    }
    pub fn relaxed_atom_load(&self, addr: *const u32) -> Result<u32, CopyError> {
        // FIXME: 应为原子操作
        debug_assert!(addr.is_aligned_to(4));
        copy_one_from_user(addr)
    }
}

pub enum FutexWaitError {
    Timeout,
    Signal,
}

impl FutexGuard<'_> {
    /// 执行 FUTEX_WAIT。
    ///
    /// 此函数执行期间会暂时释放针对 futex 字的锁，因此不会阻碍并发。
    /// 然而，其还会试图重新获取锁，因此最好将其视作一直持有锁，以避免死锁。
    pub fn wait(mut self, ddl_tick: Option<usize>, mask: FutexMask) -> Result<(), FutexWaitError> {
        let ftx_q = Box::new(FutexQ {
            key: self.key.clone(),
            task: cur_task(),
            link: LinkedListLink::new(),
            mask,
        });
        let ftx_q_ptr: *const FutexQ = &*ftx_q;
        self.queue.push_back(ftx_q);

        drop(self); // 释放锁

        let _timeout_guard;
        if let Some(ddl_tick) = ddl_tick {
            _timeout_guard = wake_task_when(cur_task(), ddl_tick);
        }

        Task::sleep_current();

        // 好消息：永远不会发生虚假唤醒！
        // 但要特别注意，因为 REQUEUE 的存在，唤醒时的 futex 字可能已经改变（不过 ftx_q_ptr 仍保证有效）
        let ftx_q = unsafe { ftx_q_ptr.as_ref_unchecked() }; // FIXME: 这貌似其实也是 UB？
        let mut new_lock = futex_lock(&ftx_q.key);
        debug_assert!(new_lock.queue.iter().any(|ftx_q| addr_eq(ftx_q, ftx_q_ptr)));
        unsafe { new_lock.queue.cursor_mut_from_ptr(ftx_q_ptr) }.remove();

        if ddl_tick.is_some_and(|ddl| get_cpu_tick() >= ddl) {
            Err(FutexWaitError::Timeout)
        } else if cur_task().find_signal_to_consume().is_some() {
            Err(FutexWaitError::Signal)
        } else {
            Ok(())
        }
    }

    pub fn wake(&mut self, count: usize, mask: FutexMask) -> usize {
        self.queue
            .iter()
            .filter(|futex_q| futex_q.key == *self.key && (futex_q.mask & mask != 0))
            .filter_map(|futex_q| unsafe { futex_q.task.as_ref_unchecked() }.try_wake())
            .take(count)
            .count()
    }
}

/// 为给定 futex word 上锁，使得内核态对同一个 [`FutexKey`] 的操作彼此互斥。返回锁守卫。
///
/// 注意这并不能阻止用户态操作，因此原子操作以及其它同步措施仍是必要的。此外，它并不能强制执行互斥，全凭调用者自觉。
///
/// 此外，它不反向保证内核态对不同地址的操作彼此不互斥，因此绝对不要试图同时获取两次此锁！
#[must_use = "返回的 RAII 守卫在被丢弃时会自动解锁，丢弃返回值毫无意义"]
pub fn futex_lock<'a>(key: &'a FutexKey) -> FutexGuard<'a> {
    FutexGuard {
        queue: FUTEX_BUCKETS[key.bucket_index()].lock(),
        key,
    }
}

pub fn futex_requeue(mut from: FutexGuard, to_key: &FutexKey, count: usize) -> usize {
    if count == 0 {
        return 0;
    }

    let mut tmp_queue = intrusive_collections::LinkedList::new(FutexQAdapter::new());
    let mut actual = 0usize;

    let mut cursor = from.queue.front_mut();
    while let Some(futex_q) = cursor.get() {
        if futex_q.key == *from.key {
            let mut node = unsafe { cursor.remove().unwrap_unchecked() };

            node.key = to_key.clone();

            tmp_queue.push_back(node);
            actual += 1;
            if actual == count {
                break;
            }
        } else {
            cursor.move_next();
        }
    }

    drop(from); // 防止死锁

    if actual > 0 {
        let mut to = futex_lock(to_key);
        to.queue.cursor_mut().splice_before(tmp_queue);
    }

    actual
}
