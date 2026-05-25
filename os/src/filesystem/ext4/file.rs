use core::slice;

use ext4_rs::ext4_defs::Ext4InodeRef;
use uninit::extension_traits::MaybeUninitExt;

use crate::{
    basic::bstr::ByteString,
    filesystem::{
        ext4::{filesystem::Ext4Fs, inode::Ext4INode},
        utils::{FileIoPgCacheMissResult, file_io_common},
        vfs::{self, FileFlags, FileOps, VfsError, VfsResult, default_fseek},
    },
    memory::{
        PAGE_BYTES,
        uaccess::{copy_from_user_uninit, copy_to_user},
    },
};

pub struct Ext4File;

impl FileOps for Ext4File {
    fn read(&mut self, mut user_buf: *mut u8, len: u64) -> VfsResult<u64> {
        let this = vfs::File::from_dyn_mut(self);

        let inode = unsafe { this.inode.as_ref() };
        let fs = unsafe { inode.fs.cast::<vfs::FileSystem<Ext4Fs>>().as_ref() };

        let inode_read_guard = inode.data.read();

        let pos = &mut this.data.get_mut().pos;

        file_io_common(
            false,
            pos,
            inode_read_guard.page_cache.as_ref().unwrap(),
            len.min(inode_read_guard.file_size - *pos),
            |buf| {
                let k_src = unsafe { buf.assume_init_by_ref() };
                copy_to_user(user_buf, k_src)?;
                user_buf = unsafe { user_buf.add(buf.len()) };
                Ok(())
            },
            |page_cache, page_index| {
                let offset = page_index as usize * PAGE_BYTES;
                let page_meta_guard = page_cache.alloc_without_insert(page_index);
                let buf = page_meta_guard.visit_memory();
                let buf = unsafe { slice::from_raw_parts_mut(buf.as_ptr() as *mut u8, buf.len()) };
                let read_bytes = fs.ops.ext4.read_at(inode.id, offset, buf)?;
                if read_bytes != 0 {
                    page_cache.insert(page_index, page_meta_guard);
                    Ok(FileIoPgCacheMissResult::Retry)
                } else {
                    Ok(FileIoPgCacheMissResult::Break)
                }
            },
        )
    }

    fn write(&mut self, mut user_data: *const u8, len: u64) -> VfsResult<u64> {
        let this = vfs::File::from_dyn_mut(self);

        let inode = unsafe { this.inode.as_ref() };
        let fs = unsafe { inode.fs.cast::<vfs::FileSystem<Ext4Fs>>().as_ref() };

        let mut inode_write_guard = inode.data.write();

        let this_data_mut = this.data.get_mut();
        let pos = &mut this_data_mut.pos;

        if this_data_mut.flags.contains(FileFlags::Append) {
            *pos = inode_write_guard.file_size;
        }

        let old_pos = *pos;

        let ret = file_io_common(
            true,
            pos,
            inode_write_guard.page_cache.as_ref().unwrap(),
            len,
            |buf| {
                copy_from_user_uninit(buf, user_data)?;
                user_data = unsafe { user_data.add(buf.len()) };
                Ok(())
            },
            |page_cache, page_index| {
                let offset = page_index as usize * PAGE_BYTES;
                let page_meta = page_cache.alloc(page_index);
                let buf = page_meta.visit_memory();
                let buf = unsafe { slice::from_raw_parts_mut(buf.as_ptr() as *mut u8, buf.len()) };
                fs.ops.ext4.read_at(inode.id, offset, buf)?;
                Ok(FileIoPgCacheMissResult::Retry)
            },
        )?;

        inode_write_guard.file_size = inode_write_guard.file_size.max(old_pos + len);
        Ok(ret)
    }

    fn fseek(&mut self, seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        default_fseek(self, seek_from)
    }

    fn fsync(&mut self) -> VfsResult<()> {
        todo!()
    }

    fn get_dir(&self, mut pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        let this = vfs::File::from_dyn(self);
        let inode = unsafe { this.inode.cast::<vfs::INode<Ext4INode>>().as_ref() };
        let ext4 = unsafe { &inode.fs.cast::<vfs::FileSystem<Ext4Fs>>().as_ref().ops.ext4 };

        let inode_rguard = inode.data.read();

        if !inode_rguard.ops.inode.is_dir() {
            return Err(VfsError::NotADirectory);
        }

        let inode_ref = Ext4InodeRef {
            inode_num: inode.id,
            inode: inode_rguard.ops.inode,
        };

        use ext4_rs::ext4_defs::{Block, Ext4DirEntry, Ext4DirEntryTail};

        const BLOCK_SIZE: u64 = ext4_rs::BLOCK_SIZE as u64;

        let inode_size = inode_ref.inode.size();
        while pos < inode_size {
            let iblock = pos / BLOCK_SIZE;
            let block_offset = pos % BLOCK_SIZE;

            let search_path = ext4.find_extent(&inode_ref, iblock as u32);

            let Ok(path) = search_path else {
                pos = (iblock + 1) * BLOCK_SIZE;
                continue;
            };

            let pblock = path.path.last().unwrap().pblock;
            let ext4block = Block::load(&ext4.block_device, (pblock * BLOCK_SIZE) as usize);

            if block_offset >= BLOCK_SIZE - (size_of::<Ext4DirEntryTail>() as u64) {
                pos = (iblock + 1) * BLOCK_SIZE;
                continue;
            }

            let de: Ext4DirEntry = ext4block.read_offset_as(block_offset as usize);
            let entry_len: u64 = de.entry_len().into();

            if entry_len == 0 {
                pos = (iblock + 1) * BLOCK_SIZE;
                continue;
            }

            if de.unused() {
                pos += entry_len;
                continue;
            }

            let next_pos = pos + entry_len;
            return Ok(Some(vfs::IterateDirData {
                name: ByteString(de.get_name()),
                next_pos,
                inode_id: de.inode,
                entry_type: match de.get_de_type() {
                    4 => vfs::IterateDirEntryType::Directory,
                    8 => vfs::IterateDirEntryType::RegularFile,
                    10 => vfs::IterateDirEntryType::Symlink,
                    _ => vfs::IterateDirEntryType::Unknown,
                },
            }));
        }

        Ok(None)
    }
}
