use core::{
    cell::SyncUnsafeCell,
    marker::PhantomPinned,
    mem::{ManuallyDrop, MaybeUninit},
    ops::Deref,
    pin::Pin,
    sync::atomic::{Atomic, AtomicUsize, Ordering},
};

use bitflags::bitflags;

use crate::{
    basic::shared_with_asm::SharedWithAsm,
    memory::{
        MEMORY_SIZE, MEMORY_START, PAGE_BYTES, address::PhysPageNum,
        frame_allocator::FRAME_ALLOCATOR,
    },
};

pub struct PageMeta {
    /// 引用计数
    ref_count: AtomicUsize,
    /// 页标志
    flags: Atomic<PageFlagsUnderlying>,
    /// 页类型，注意它在整个页的生命周期内为常量，因此不必是原子的
    page_type: PageType,

    _pin: PhantomPinned, // 这个结构体不能被移动，因为它的地址就是它的 ID
}

static_assertions::assert_not_impl_any!(PageMeta: Drop); // 因此我们不必操心销毁问题

pub struct PageMetas {
    pub(self) pages: [SyncUnsafeCell<MaybeUninit<PageMeta>>; MEMORY_SIZE / PAGE_BYTES],
}

pub static PAGE_METADATAS: PageMetas = PageMetas {
    pages: [const { SyncUnsafeCell::new(MaybeUninit::uninit()) }; MEMORY_SIZE / PAGE_BYTES],
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageType {
    Anonymous,
    File(!), // TODO
}

type PageFlagsUnderlying = u8;

bitflags! {
    struct PageFlags: PageFlagsUnderlying {
        const Dirty = 1 << 0;
    }
}

const METAS_ARRAY_OFFSET: usize = MEMORY_START.div_exact(PAGE_BYTES).unwrap();

impl PageMeta {
    pub(self) fn get_index(&self) -> usize {
        let my_addr = self as *const Self as usize;
        let array_addr = &raw const PAGE_METADATAS.pages as usize;

        let offset_bytes = my_addr - array_addr;

        debug_assert!(offset_bytes % size_of::<PageMeta>() == 0);
        offset_bytes / size_of::<PageMeta>()
    }

    pub(self) fn inc_ref(self: Pin<&Self>) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    pub(self) fn dec_ref_may_release_res(self: Pin<&Self>) {
        let old = self.ref_count.fetch_sub(1, Ordering::Release);

        if old == 1 {
            match self.page_type {
                PageType::Anonymous => {
                    let physical_page_num = self.get_index() + METAS_ARRAY_OFFSET;

                    FRAME_ALLOCATOR.lock().dealloc(physical_page_num, 1);

                    crate::println!("Page {physical_page_num:#x} released");
                }
                _ => todo!(),
            }
        }
    }
}

impl PageMetas {
    /// 标记某一位置的页为开始使用
    ///
    /// # Safety
    /// 1. 此函数使用内部可变性且不进行同步，调用方必须保证自己是唯一的调用者。
    /// 2. 原位置的页必须不正在被使用。
    /// 3. 此模块只负责资源回收，调用方必须保证资源在返回引用及其所有衍生物的生命周期结束前被获取。
    ///    （TODO：将资源获取和这些合起来再封一层，获取和回收不在一个模块里还是太超模了）
    #[must_use = "返回值包含 RAII 语义，丢弃它轻则让此调用完全无意义，重则会带来内存安全问题"]
    pub unsafe fn mark_new_page<'a>(
        &'a self,
        page_num: PhysPageNum<0>,
        page_type: PageType,
    ) -> PageMetaGuard<'a> {
        let index = page_num.0 - METAS_ARRAY_OFFSET;
        let item_ptr = self.pages[index].get();

        // SAFETY: 调用方保证了唯一性，因此此处的可变性是安全的。至于非空，.get() 保证了这点。
        let item = unsafe { item_ptr.as_mut_unchecked() };

        let raw_ref = item.write(PageMeta {
            ref_count: AtomicUsize::new(1),
            flags: Atomic::<PageFlagsUnderlying>::new(PageFlags::empty().bits()),
            page_type,
            _pin: PhantomPinned,
        });

        crate::println!("Page {:#x} allocated", page_num.0);

        // SAFETY: PageMetas 的成员为模块私有，外界只能使用 PAGE_METADATAS，它不可变因此不可能被移动
        PageMetaGuard {
            reference: unsafe { Pin::new_unchecked(raw_ref) },
        }
    }
}

pub struct PageMetaGuard<'a> {
    pub(self) reference: Pin<&'a PageMeta>,
}

impl Deref for PageMetaGuard<'_> {
    type Target = PageMeta;

    fn deref(&self) -> &Self::Target {
        self.reference.get_ref()
    }
}

impl Drop for PageMetaGuard<'_> {
    fn drop(&mut self) {
        self.reference.dec_ref_may_release_res();
    }
}

impl Clone for PageMetaGuard<'_> {
    fn clone(&self) -> Self {
        let reference = self.reference;
        reference.inc_ref();
        Self { reference }
    }
}

unsafe impl SharedWithAsm for PageMetaGuard<'_> {
    type AsmType = PhysPageNum<0>;

    unsafe fn into_asm(self) -> Self::AsmType {
        let this = ManuallyDrop::new(self);
        PhysPageNum(this.get_index() + METAS_ARRAY_OFFSET)
    }

    unsafe fn from_asm(asm_value: Self::AsmType) -> Self {
        let index = asm_value.0 - METAS_ARRAY_OFFSET;
        let ptr = &PAGE_METADATAS.pages[index].get();
        let raw_ref = unsafe { ptr.as_ref_unchecked() };
        let raw_ref = unsafe { raw_ref.assume_init_ref() };
        Self {
            reference: unsafe { Pin::new_unchecked(raw_ref) },
        }
    }
}
