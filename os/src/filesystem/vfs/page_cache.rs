use core::{ops::Deref, ptr::NonNull, sync::atomic::Ordering};

use intrusive_collections::{LinkedListLink, UnsafeRef, intrusive_adapter};
use xarray::XArray;

use crate::{
    basic::{extend_lifetime, sync::RwLock},
    filesystem::vfs::INode,
    memory::{
        frame_allocator::FRAME_ALLOCATOR,
        page_meta::{PageFlags, PageMeta, PageMetaGuard, PageType},
    },
};

pub struct PageCacheData {
    /// 实际页缓存
    pub pages: XArray<PageMetaGuard>,
    /// 脏页链表头
    pub dirty_sent: intrusive_collections::LinkedList<PageMetaAdapter>,
}

intrusive_adapter!(pub PageMetaAdapter = UnsafeRef<PageMeta>: PageMeta { page_type.File.dirty => LinkedListLink });

pub struct PageCache {
    /// 对应 INode 指针，永远有效且不会改变
    pub inode: NonNull<INode>,
    pub data: RwLock<PageCacheData>,
}

impl PageCache {
    /// 初始化新 INode 的页缓存字段。页缓存包含自引用结构，因此不得不这么初始化。
    pub unsafe fn init(inode: &mut INode) {
        let inode_ptr = NonNull::from_mut(inode);

        let page_cache = &mut inode.data.get_mut().page_cache;

        let old_page_cache = page_cache.replace(PageCache {
            inode: inode_ptr,
            data: RwLock::new(PageCacheData {
                pages: XArray::new(),
                dirty_sent: intrusive_collections::LinkedList::new(PageMetaAdapter::new()),
            }),
        });

        debug_assert!(old_page_cache.is_none());
    }

    pub fn get<'a>(&'a self, page_index: u64) -> Option<&'a PageMeta> {
        let lock_guard = self.data.read();
        let page_meta = lock_guard.pages.load(page_index)?;
        let page_meta: &PageMeta = page_meta.deref();
        // SAFETY: 由于返回引用的实际生命周期一直延续到最后一个 PageMetaGuard 被 drop，
        // 而我们又引用了 self（因此 xarray 内部的 PageMetaGuard 不能被 drop），这一转换是安全的。
        Some(unsafe { extend_lifetime(page_meta) })
    }

    pub fn insert<'a>(&'a self, page_index: u64, page_meta_guard: PageMetaGuard) -> &'a PageMeta {
        // SAFETY: 见 Self::get
        let meta_ref = unsafe { extend_lifetime(page_meta_guard.deref()) };

        self.data.write().pages.store(page_index, page_meta_guard);

        meta_ref
    }

    pub fn alloc<'a>(&'a self, page_index: u64) -> &'a PageMeta {
        self.insert(page_index, self.alloc_without_insert(page_index))
    }

    pub fn get_or_alloc(&self, page_index: u64) -> &PageMeta {
        self.get(page_index)
            .unwrap_or_else(|| self.alloc(page_index))
    }

    pub fn alloc_without_insert(&self, page_index: u64) -> PageMetaGuard {
        FRAME_ALLOCATOR.alloc_one_as(PageType::File {
            page_index,
            dirty: LinkedListLink::new(),
        })
    }

    pub fn truncate(&self, new_page_count: u64) {
        let node = unsafe { self.inode.as_ref() };
        let fs = unsafe { node.fs.as_ref() };

        let mut data_guard = self.data.write();
        let data = &mut *data_guard;

        let mut cursor = data.pages.cursor_mut(new_page_count);
        cursor.next();
        while let Some(removed) = cursor.remove() {
            cursor.next();

            let old_flags = removed.flags.load(Ordering::Relaxed);
            let old_flags = PageFlags::from_bits_retain(old_flags);

            if old_flags.contains(PageFlags::Dirty) {
                fs.writeback_page(node, &removed);
                unsafe { data.dirty_sent.cursor_mut_from_ptr(&*removed) }.remove();
            }
        }
    }
}
