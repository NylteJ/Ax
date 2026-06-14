use core::{
    any::type_name,
    hash::Hash,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

use hashbrown::HashMap;

use crate::basic::lru::LruNode;

pub unsafe trait CachedValue {
    const DEBUG_NAME: &'static str = type_name::<Self>();

    fn mem_refcount(&self) -> &AtomicU32;
    fn in_cache(&self) -> *mut bool;

    fn my_lru(&self) -> &LruNode<Self>;
    fn in_lru(&self) -> *mut bool;
    fn lru_sent_lock(&self) -> impl Deref<Target = LruNode<Self>> + DerefMut + '_;

    type CacheKey: Eq + Hash;
    type CacheValue: Deref<Target = Self>;

    fn cache_wlock(
        &self,
    ) -> impl Deref<Target = HashMap<Self::CacheKey, Self::CacheValue>> + DerefMut + '_;

    fn remove_from_cache(
        &self,
        cache: &mut HashMap<Self::CacheKey, Self::CacheValue>,
    ) -> Self::CacheValue;
}

pub trait CachedValueArc: Deref
where
    Self::Target: CachedValue,
{
    unsafe fn from_exists_unchecked(cache: &Self::Target) -> Self;

    unsafe fn drop_inner_unchecked(&self);
}

pub(in super::super) unsafe fn get_value_arc_from_cache<VArc: CachedValueArc>(
    cache: &VArc::Target,
) -> VArc
where
    VArc::Target: CachedValue,
{
    debug_assert!(unsafe { *cache.in_cache() });

    let old_ref = cache.mem_refcount().load(Ordering::Relaxed);

    if old_ref == 0 {
        let mut lock_guard = cache.lru_sent_lock();
        match cache
            .mem_refcount()
            .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => unsafe {
                let old_in_lru = cache.in_lru().replace(false);
                if old_in_lru {
                    cache.my_lru().lru_drop(&mut lock_guard);
                }
                log::trace!(
                    "{} {:#x} 从缓存中复活",
                    VArc::Target::DEBUG_NAME,
                    (cache as *const VArc::Target).addr()
                );
            },
            Err(_) => {
                cache.mem_refcount().fetch_add(1, Ordering::Relaxed);
            }
        }
    } else {
        cache.mem_refcount().fetch_add(1, Ordering::Acquire);
    }

    unsafe { VArc::from_exists_unchecked(&cache) }
}

pub(in super::super) unsafe fn get_varc_from_value<VArc: CachedValueArc>(
    value: &VArc::Target,
) -> VArc
where
    VArc::Target: CachedValue,
{
    let old = value.mem_refcount().fetch_add(1, Ordering::Relaxed);
    debug_assert_ne!(old, 0);
    unsafe { VArc::from_exists_unchecked(value) }
}

pub(super) unsafe fn drop_cached_varc<VArc: CachedValueArc>(
    arc: &mut VArc,
    evictable_fn: impl FnOnce(&VArc::Target) -> bool,
) where
    VArc::Target: CachedValue,
{
    // 注意这里可以直接递减引用计数，因为我们允许 Cache 中存在引用计数归零但不在 LRU 中的对象
    let old_ref = arc.mem_refcount().fetch_sub(1, Ordering::Release);

    debug_assert!(old_ref > 0);

    if old_ref != 1 {
        return;
    }

    // 慢速路径，此时我们将引用计数减少到了 0，但注意其仍可能被 Cache 查找复活，因此要先获取 cache 的写锁
    let cache_write_guard = arc.cache_wlock();

    if arc.mem_refcount().load(Ordering::Relaxed) != 0 {
        return; // 被复活了，直接返回
    }

    // 此时没有复活，我们便是唯一引用
    if !evictable_fn(arc) {
        // 不可驱逐，因此不能加入 LRU 链表或销毁，直接返回
        // 其仍位于 Cache 中，因此实际上并没有资源泄漏
        return;
    }

    // 可驱逐，走正常 LRU / 销毁流程
    if unsafe { *arc.in_cache() } {
        // 位于 Cache 中
        let mut lru_guard = arc.lru_sent_lock();
        unsafe {
            let old_in_lru = arc.in_lru().replace(true);
            debug_assert!(!old_in_lru);
            lru_guard.lru_add(
                (*arc).deref(),
                (arc.my_lru() as *const LruNode<_>).addr()
                    - ((*arc).deref() as *const VArc::Target).addr(),
            )
        };
        // log::trace!(
        //     "{} {:#x} added to LRU list",
        //     VArc::Target::DEBUG_NAME,
        //     ((*arc).deref() as *const VArc::Target).addr()
        // );
    } else {
        // 不位于 Cache 中，直接销毁（别忘了先解锁）
        drop(cache_write_guard);
        unsafe { VArc::drop_inner_unchecked(arc) };
    }
}
