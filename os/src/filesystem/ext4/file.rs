use core::slice;

use uninit::extension_traits::MaybeUninitExt;

use crate::{
    filesystem::{
        ext4::filesystem::Ext4Fs,
        utils::{FileIoPgCacheMissResult, file_io_common},
        vfs::{self, FileFlags, FileOps, VfsResult, default_fseek},
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

    fn get_dir(&self, pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        todo!()
    }
}
