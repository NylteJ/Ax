use core::{
    cell::SyncUnsafeCell,
    hint::unreachable_unchecked,
    marker::{PhantomData, PhantomPinned},
    mem::{ManuallyDrop, MaybeUninit},
    ops::Deref,
    pin::Pin,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
};

use bitflags::bitflags;
use intrusive_collections::{LinkedListLink, UnsafeRef};
use uninit::extension_traits::MaybeUninitExt;

use crate::{
    basic::{cold_path, shared_with_asm::SharedWithAsm},
    filesystem::vfs::PageCache,
    memory::{
        MEMORY_SIZE, MEMORY_START, PAGE_BYTES,
        address::{PhysAddr, PhysPageNum},
        frame_allocator::FRAME_ALLOCATOR,
    },
};

pub struct PageMeta {
    /// 引用计数
    ref_count: AtomicU32,
    /// 页标志
    pub flags: AtomicU8,
    /// 页类型，注意它在整个页的生命周期内为常量，因此不必是原子的
    pub page_type: PageType,

    _pin: PhantomPinned, // 这个结构体不能被移动，因为它的地址就是它的 ID
}

static_assertions::assert_not_impl_any!(PageMeta: Drop); // 因此我们不必操心销毁问题

pub struct PageMetas {
    pages: [SyncUnsafeCell<MaybeUninit<PageMeta>>; MEMORY_SIZE / PAGE_BYTES],
}

pub static PAGE_METADATAS: PageMetas = PageMetas {
    pages: [const { SyncUnsafeCell::new(MaybeUninit::uninit()) }; MEMORY_SIZE / PAGE_BYTES],
};

#[derive(Debug, Clone)]
pub enum PageType {
    /// 此页是匿名页，没有后备存储
    Anonymous,
    /// 此页是文件页
    File {
        page_index: u64,
        dirty: LinkedListLink,
    },

    /// 此页是虚设页，其不仅没有后备存储，也不实际占用任何资源。其析构是空操作
    Phantom,
}

unsafe impl Send for PageType {}
unsafe impl Sync for PageType {}

type PageFlagsUnderlying = u8;

bitflags! {
    pub struct PageFlags: PageFlagsUnderlying {
        /// 此页为脏页，其内容自创建以来被修改过
        const Dirty = 1 << 0;
        /// 此页正在回写中；只对文件页有效
        const WritingBack = 1 << 2;
    }
}

static_assertions::assert_type_eq_all!(PageFlagsUnderlying, u8);

const METAS_ARRAY_OFFSET: usize = MEMORY_START / PAGE_BYTES;
static_assertions::const_assert_eq!(MEMORY_START % PAGE_BYTES, 0);

impl PageMeta {
    fn get_index(&self) -> usize {
        let my_addr = self as *const Self as usize;
        let array_addr = &raw const PAGE_METADATAS.pages as usize;

        let offset_bytes = my_addr - array_addr;

        debug_assert!(offset_bytes % size_of::<PageMeta>() == 0);
        offset_bytes / size_of::<PageMeta>()
    }

    fn inc_ref(self: Pin<&Self>) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    fn dec_ref_may_release_res(self: Pin<&Self>) {
        let old = self.ref_count.fetch_sub(1, Ordering::Release);

        if old == 1 {
            match self.page_type {
                PageType::Anonymous | PageType::File { .. } => {
                    let physical_page_num = self.get_index() + METAS_ARRAY_OFFSET;

                    FRAME_ALLOCATOR.lock().dealloc(physical_page_num, 1);

                    // crate::println!(
                    //     "Page {physical_page_num:#x} released (type: {:?})",
                    //     self.page_type
                    // );
                }

                PageType::Phantom => cold_path(), // 什么都不做，下一个减少引用计数的会将其回绕到 MAX
            }
        }
    }

    pub fn get_page_num(&self) -> PhysPageNum<0> {
        PhysPageNum::<0>(self.get_index() + METAS_ARRAY_OFFSET)
    }
    pub fn get_addr(&self) -> usize {
        let page_num = self.get_page_num();
        PhysAddr::from(page_num).to_virtual_address().0
    }

    pub fn visit_memory(&self) -> &[MaybeUninit<u8>; PAGE_BYTES] {
        unsafe { (self.get_addr() as *const [MaybeUninit<u8>; PAGE_BYTES]).as_ref_unchecked() }
    }
    pub fn visit_memory_off(&self, begin: usize, end: usize) -> &[MaybeUninit<u8>] {
        let page_mem = self.visit_memory();
        &page_mem[begin..end]
    }

    pub unsafe fn visit_memory_mut(&self) -> &mut [MaybeUninit<u8>; PAGE_BYTES] {
        unsafe { (self.get_addr() as *mut [MaybeUninit<u8>; PAGE_BYTES]).as_mut_unchecked() }
    }
    pub unsafe fn visit_memory_mut_off(&self, begin: usize, end: usize) -> &mut [MaybeUninit<u8>] {
        let page_mem = unsafe { self.visit_memory_mut() };
        &mut page_mem[begin..end]
    }

    pub unsafe fn ref_count(&self) -> &AtomicU32 {
        &self.ref_count
    }

    pub unsafe fn pagecache_mark_dirty(&self, page_cache: &PageCache) {
        let PageType::File { page_index, dirty } = &self.page_type else {
            unsafe { unreachable_unchecked() };
        };

        let old_flags = self
            .flags
            .fetch_or(PageFlags::Dirty.bits(), Ordering::Acquire);
        let old_flags = PageFlags::from_bits_retain(old_flags);

        if !old_flags.intersects(PageFlags::Dirty | PageFlags::WritingBack) {
            log::trace!(
                "INode {:#x} Page {page_index} become Dirty",
                page_cache.inode.addr()
            );
            page_cache
                .data
                .write()
                .dirty_sent
                .push_front(unsafe { UnsafeRef::from_raw(self) });
        }
    }
}

impl PageMetas {
    /// 标记某一位置的页为开始使用
    ///
    /// # Safety
    /// 1. 此函数使用内部可变性且不进行同步，调用方必须保证自己是该 `page_num` 唯一的调用者。
    /// 2. 原位置的页必须不正在被使用。
    /// 3. 此模块只负责资源回收，调用方必须保证资源在返回引用及其所有衍生物的生命周期结束前被获取。
    ///    （TODO：将资源获取和这些合起来再封一层，获取和回收不在一个模块里还是太超模了）
    #[must_use = "返回值包含 RAII 语义，丢弃它轻则让此调用完全无意义，重则会带来内存安全问题"]
    pub unsafe fn mark_new_page(
        &'static self,
        page_num: PhysPageNum<0>,
        page_type: PageType,
    ) -> PageMetaGuard {
        let index = page_num.0 - METAS_ARRAY_OFFSET;
        let item_ptr = self.pages[index].get();

        let page_is_phantom = matches!(page_type, PageType::Phantom);

        // SAFETY: 调用方保证了唯一性，因此此处的可变性是安全的。至于非空，.get() 保证了这点。
        let item = unsafe { item_ptr.as_mut_unchecked() };

        let raw_ref = item.write(PageMeta {
            ref_count: AtomicU32::new(if page_is_phantom { u32::MAX } else { 1 }),
            flags: AtomicU8::new(PageFlags::empty().bits()),
            page_type,
            _pin: PhantomPinned,
        });

        // crate::println!("Page {:#x} allocated", page_num.0);

        // SAFETY: PageMetas 的成员为模块私有，外界只能使用 PAGE_METADATAS，它不可变因此不可能被移动
        PageMetaGuard {
            reference: unsafe { Pin::new_unchecked(raw_ref) },
        }
    }
}

pub struct PageMetaGuard {
    reference: Pin<&'static PageMeta>,
}

impl PageMetaGuard {
    pub fn is_unique_ref(&self) -> bool {
        return self.ref_count.load(Ordering::Acquire) == 1;
    }
}

impl Deref for PageMetaGuard {
    type Target = PageMeta;

    fn deref(&self) -> &Self::Target {
        self.reference.get_ref()
    }
}

impl Drop for PageMetaGuard {
    fn drop(&mut self) {
        self.reference.dec_ref_may_release_res();
    }
}

impl Clone for PageMetaGuard {
    fn clone(&self) -> Self {
        let reference = self.reference;
        reference.inc_ref();
        Self { reference }
    }
}

unsafe impl SharedWithAsm for PageMetaGuard {
    type AsmType = PhysPageNum<0>;

    unsafe fn into_asm(self) -> Self::AsmType {
        let this = ManuallyDrop::new(self);
        PhysPageNum(this.get_index() + METAS_ARRAY_OFFSET)
    }

    unsafe fn from_asm(asm_value: Self::AsmType) -> Self {
        let index = asm_value.0 - METAS_ARRAY_OFFSET;
        let ptr = &PAGE_METADATAS.pages[index].get();
        let raw_ref = unsafe { ptr.as_ref_unchecked() };
        let raw_ref = unsafe { raw_ref.assume_init_by_ref() };
        Self {
            reference: unsafe { Pin::new_unchecked(raw_ref) },
        }
    }
}

pub struct PageMetaGuardRef<'a> {
    inner: ManuallyDrop<PageMetaGuard>,
    _phantom: PhantomData<&'a PageMetaGuard>,
}

impl<'a> Deref for PageMetaGuardRef<'a> {
    type Target = PageMetaGuard;

    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}

unsafe impl xarray::ItemEntry for PageMetaGuard {
    type Ref<'a>
        = PageMetaGuardRef<'a>
    where
        Self: 'a;

    fn into_raw(self) -> *const () {
        let this = ManuallyDrop::new(self);
        (this.reference.get_ref() as *const PageMeta).cast()
    }

    unsafe fn from_raw(raw: *const ()) -> Self {
        Self {
            reference: unsafe { Pin::new_unchecked((raw as *const PageMeta).as_ref_unchecked()) },
        }
    }

    unsafe fn raw_as_ref<'a>(raw: *const ()) -> Self::Ref<'a> {
        PageMetaGuardRef {
            inner: ManuallyDrop::new(unsafe { Self::from_raw(raw) }),
            _phantom: PhantomData,
        }
    }
}
