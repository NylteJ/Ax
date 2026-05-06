use core::mem::MaybeUninit;

use crate::{
    filesystem::vfs::{PageCache, VfsResult},
    memory::PAGE_BYTES,
};

pub enum FileIoPgCacheMissResult {
    Retry,
    Break,
}

pub fn file_io_common(
    pos: &mut u64,
    page_cache: &PageCache,
    len: u64,
    mut op: impl FnMut(&[MaybeUninit<u8>]) -> VfsResult<()>,
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

            let k_mem = page.visit_memory_off(from as usize, to as usize);

            op(k_mem)?;
        }

        len
    };

    *pos += ret;
    Ok(ret)
}
