use core::{mem::MaybeUninit, sync::atomic::Ordering};

use intrusive_collections::UnsafeRef;

use crate::{
    filesystem::vfs::{PageCache, VfsResult},
    memory::{PAGE_BYTES, page_meta::PageFlags},
};

pub enum FileIoPgCacheMissResult {
    Retry,
    Break,
}

pub fn file_io_common(
    write: bool,
    pos: &mut u64,
    page_cache: &PageCache,
    len: u64,
    mut op: impl FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<()>,
    mut when_pgcache_miss: impl FnMut(&PageCache, u64) -> VfsResult<FileIoPgCacheMissResult>,
) -> VfsResult<u64> {
    let ret = 'do_op_and_get_ret: {
        let pos = *pos;

        let begin_page_index = pos / PAGE_BYTES as u64;
        let end_page_index = (pos + len).div_ceil(PAGE_BYTES as u64);

        for page_index in begin_page_index..end_page_index {
            let page = loop {
                if let Some(page) = page_cache.get(page_index) {
                    break page;
                }

                match when_pgcache_miss(page_cache, page_index)? {
                    FileIoPgCacheMissResult::Retry => (),
                    FileIoPgCacheMissResult::Break => {
                        if page_index == begin_page_index {
                            break 'do_op_and_get_ret 0;
                        } else {
                            let bytes = (page_index - begin_page_index) * PAGE_BYTES as u64
                                - pos % PAGE_BYTES as u64;
                            break 'do_op_and_get_ret bytes;
                        }
                    }
                }
            };

            let from = if page_index == begin_page_index {
                pos % PAGE_BYTES as u64
            } else {
                0
            };

            let to = if page_index == end_page_index - 1 {
                (pos + len) % PAGE_BYTES as u64
            } else {
                PAGE_BYTES as u64
            };

            op(unsafe { page.visit_memory_mut_off(from as usize, to as usize) })?;

            if write {
                let old_flags = page
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
                        .push_front(unsafe { UnsafeRef::from_raw(page) });
                }
            }
        }

        len
    };

    *pos += ret;
    Ok(ret)
}
