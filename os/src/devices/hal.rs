use core::{mem::MaybeUninit, ptr::NonNull, slice};

use virtio_drivers::{BufferDirection, Hal, PhysAddr};

use crate::memory::{
    PAGE_BYTES,
    address::{PhysAddr as MyPhysAddr, PhysPageNum, VirtAddr},
    frame_allocator::FRAME_ALLOCATOR,
};

pub struct VirtIOHal;

static_assertions::const_assert_eq!(virtio_drivers::PAGE_SIZE, crate::memory::PAGE_BYTES);

unsafe impl Hal for VirtIOHal {
    // 我们已经断言 PCI 设备支持硬件维护一致性，因此这里可以直接像普通分配器一样分配内存

    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let page = FRAME_ALLOCATOR.alloc(pages);

        // 注意：这里分配的页直接交给库，不由我们管理，因此也不用注册到 PAGE_METADATAS
        log::debug!(
            "为 VirtIO 设备分配了 {pages} 页物理内存，起始页号 {:#x}",
            page.0
        );

        let paddr = MyPhysAddr::from(page);
        let vaddr = paddr.to_virtual_address();

        let page = unsafe {
            slice::from_raw_parts_mut(vaddr.0 as *mut MaybeUninit<u8>, pages * PAGE_BYTES)
        };
        unsafe { page.as_mut_ptr().write_bytes(0, page.len()) };

        (paddr.0 as PhysAddr, NonNull::from_mut(page).cast())
    }

    unsafe fn dma_dealloc(paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        debug_assert_eq!(
            MyPhysAddr(paddr as usize).to_virtual_address().0,
            vaddr.addr().get()
        );

        let page = PhysPageNum::<0>::from(MyPhysAddr(paddr as usize));
        FRAME_ALLOCATOR.dealloc(page, pages);

        log::debug!(
            "为 VirtIO 设备解分配了 {pages} 页物理内存，起始页号 {:#x}",
            page.0
        );

        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        let paddr = MyPhysAddr(paddr as usize);
        let vaddr = paddr.to_virtual_address();
        unsafe { NonNull::new_unchecked(vaddr.0 as *mut u8) }
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        // 我们并不区分所谓“可共享内存”，因此直接转换即可
        let vaddr = VirtAddr(buffer.as_ptr().addr());
        let paddr = vaddr.to_physical_address();
        paddr.0 as PhysAddr
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // no-op
    }
}
