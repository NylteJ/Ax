use core::{ops::Deref, ptr::NonNull};

use xarray::XArray;

use crate::{
    basic::{extend_lifetime, sync::RwLock},
    filesystem::vfs::INode,
    memory::{
        address::PhysPageNum,
        frame_allocator::FRAME_ALLOCATOR,
        page_meta::{PAGE_METADATAS, PageMeta, PageMetaGuard, PageType},
    },
};

pub struct PageCacheData {
    /// 实际页缓存
    pub pages: XArray<PageMetaGuard>,
}

pub struct PageCache {
    /// 对应 INode 指针，永远有效且不会改变
    pub inode: NonNull<INode>,
    pub data: RwLock<PageCacheData>,
}

impl PageCache {
    pub fn new(inode: &INode) -> Self {
        PageCache {
            inode: NonNull::from_ref(inode),
            data: RwLock::new(PageCacheData {
                pages: XArray::new(),
            }),
        }
    }

    pub fn get<'a>(&'a self, page_index: u64) -> Option<&'a PageMeta> {
        let lock_guard = self.data.read();
        let page_meta = lock_guard.pages.load(page_index)?;
        let page_meta: &PageMeta = page_meta.deref();
        // SAFETY: 由于返回引用的实际生命周期一直延续到最后一个 PageMetaGuard 被 drop，
        // 而我们又引用了 self（因此 xarray 内部的 PageMetaGuard 不能被 drop），这一转换是安全的。
        Some(unsafe { extend_lifetime::<'a>(page_meta) })
    }

    pub fn insert(&self, page_index: u64, page_meta_guard: PageMetaGuard) {
        self.data.write().pages.store(page_index, page_meta_guard);
    }

    pub fn alloc<'a>(&'a self, page_index: u64) -> &'a PageMeta {
        let phys_page = PhysPageNum(
            FRAME_ALLOCATOR
                .lock()
                .alloc(1)
                .expect("应当能分配物理页作为缓存"), // TODO: 回收机制（可以为每个 FRAME_ALLOCATOR 的对应调用都加上）
        );

        let page_type = PageType::File {
            page_cache: NonNull::from_ref(self),
            page_index,
        };

        let guard = unsafe { PAGE_METADATAS.mark_new_page(phys_page, page_type) };

        // SAFETY: 见 Self::get
        let meta_ref = unsafe { extend_lifetime::<'a>(guard.deref()) };

        self.insert(page_index, guard);

        meta_ref
    }

    pub fn get_or_alloc(&self, page_index: u64) -> &PageMeta {
        self.get(page_index)
            .unwrap_or_else(|| self.alloc(page_index))
    }
}
