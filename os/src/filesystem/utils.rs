use core::mem::MaybeUninit;

use crate::filesystem::vfs::{PageCache, VfsResult};

pub enum FileIoPgCacheMissResult {
    Retry,
    Break,
}

/// 处理文件 I/O 的通用部分逻辑。返回已处理的字节数，保证 pos 只在成功时更新，且更新量等于返回值。
///
/// - 对于存在于页缓存中的页，它会计算对应的切片并直接调用 `op` 进行处理；
///   - 若 `op` 返回的实际处理字节数不等于切片长度，则停止处理并返回已处理的字节数。
/// - 对于不位于页缓存中的页，它会调用 `when_pgcache_miss` 来试图读取页缓存
///   - 若其返回 [`Retry`][`FileIoPgCacheMissResult::Retry`] 则重新试图查找页缓存
///   - 若其返回 [`Break`][`FileIoPgCacheMissResult::Break`] 则停止处理并返回已处理的字节数。
///
/// 无论如何，失败（返回 `VfsError`）时，尽管已经发生的操作无法撤销，pos 不会改变。
pub fn file_io_common(
    write: bool,
    pos: &mut u64,
    page_cache: &PageCache,
    len: u64,
    mut op: impl FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
    mut when_pgcache_miss: impl FnMut(&PageCache, u64) -> VfsResult<FileIoPgCacheMissResult>,
) -> VfsResult<u64> {
    let ret = 'do_op_and_get_ret: {
        let pos = *pos;

        const PAGE_BYTES: u64 = crate::memory::PAGE_BYTES as u64;

        let first_page_index = pos / PAGE_BYTES;
        let last_page_index = (pos + len).div_ceil(PAGE_BYTES);

        let bytes_before_page = |page_index: u64| {
            if page_index == first_page_index {
                0
            } else {
                (page_index - first_page_index) * PAGE_BYTES - pos % PAGE_BYTES
            }
        };

        for page_index in first_page_index..last_page_index {
            let page = loop {
                if let Some(page) = page_cache.get(page_index) {
                    break page;
                }

                match when_pgcache_miss(page_cache, page_index)? {
                    FileIoPgCacheMissResult::Retry => (),
                    FileIoPgCacheMissResult::Break => {
                        break 'do_op_and_get_ret bytes_before_page(page_index);
                    }
                }
            };

            let from = if page_index == first_page_index {
                pos % PAGE_BYTES
            } else {
                0
            };

            let to = if page_index == last_page_index - 1 {
                let raw = (pos + len) % PAGE_BYTES;
                if raw == 0 { PAGE_BYTES } else { raw }
            } else {
                PAGE_BYTES
            };

            let actual = op(unsafe { page.visit_memory_mut_off(from as usize, to as usize) })?;

            if actual > 0 && write {
                unsafe { page.pagecache_mark_dirty(page_cache) };
            }

            if actual != to - from {
                break 'do_op_and_get_ret bytes_before_page(page_index) + actual;
            }
        }

        len
    };

    *pos += ret;
    Ok(ret)
}
