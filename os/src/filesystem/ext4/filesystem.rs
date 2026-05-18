use core::any::Any;

use alloc::{sync::Arc, vec::Vec};

use crate::{
    devices::block_device::BlockDevice,
    filesystem::{
        ext4::inode::Ext4INode,
        vfs::{self, FileSystemOps, VfsResult},
    },
    memory::PAGE_BYTES,
};

pub struct Ext4Fs {
    pub(super) ext4: ext4_rs::Ext4,
}

impl FileSystemOps for Ext4Fs {
    fn writeback_page(
        &self,
        inode: &vfs::INode,
        page_index: u64,
        page_data: &[u8; PAGE_BYTES],
    ) -> VfsResult<()> {
        debug_assert!((&inode.data.read().ops as &dyn Any).is::<Ext4INode>());

        let ino_data_rguard = inode.data.read();
        let file_size = ino_data_rguard.file_size;

        let mut inode_ref = ext4_rs::ext4_defs::Ext4InodeRef {
            inode_num: inode.id,
            inode: unsafe {
                (&raw const ino_data_rguard.ops)
                    .cast::<Ext4INode>()
                    .as_ref_unchecked()
            }
            .inode,
        };

        drop(ino_data_rguard);

        let offset = page_index * PAGE_BYTES as u64;
        let actual_size = file_size - offset;
        let end_off = (actual_size as usize).min(PAGE_BYTES);

        self.ext4
            .write_at_2(&mut inode_ref, offset as usize, &page_data[..end_off])?;

        Ok(())
    }

    fn read_to_page(
        &self,
        inode: &vfs::INode,
        page_index: u64,
        buffer: &mut [core::mem::MaybeUninit<u8>; PAGE_BYTES],
    ) -> VfsResult<usize> {
        debug_assert!((&inode.data.read().ops as &dyn Any).is::<Ext4INode>());

        let ino_data_rguard = inode.data.read();

        let mut inode_ref = ext4_rs::ext4_defs::Ext4InodeRef {
            inode_num: inode.id,
            inode: unsafe {
                (&raw const ino_data_rguard.ops)
                    .cast::<Ext4INode>()
                    .as_ref_unchecked()
            }
            .inode,
        };

        drop(ino_data_rguard);

        Ok(self
            .ext4
            .read_at_2(&mut inode_ref, page_index as usize * PAGE_BYTES, unsafe {
                buffer.assume_init_mut()
            })?)
    }
}

impl Ext4Fs {
    pub fn new(block_device: &'static dyn BlockDevice) -> Self {
        Self {
            ext4: ext4_rs::Ext4::open(Arc::new(Ext4BlockDevice(block_device))),
        }
    }
}

struct Ext4BlockDevice(&'static dyn BlockDevice);

impl ext4_rs::BlockDevice for Ext4BlockDevice {
    fn read_offset(&self, offset: usize) -> Vec<u8> {
        let block_size = self.0.block_size();
        static_assertions::const_assert_ne!(ext4_rs::BLOCK_SIZE, 0);

        let start_block = offset / block_size;
        let start_off = offset % block_size;
        let end_offset = offset + ext4_rs::BLOCK_SIZE - 1;
        let end_block = end_offset / block_size;
        let total_blocks = end_block - start_block + 1;
        let buf_len = total_blocks * block_size;

        let mut buf = Vec::with_capacity(buf_len);

        let read_bytes = self
            .0
            .read(start_block, &mut buf.spare_capacity_mut()[..buf_len])
            .expect("读取不应失败（也没有报告失败的接口）")
            .len();
        assert_eq!(read_bytes, buf_len);

        unsafe { buf.set_len(buf_len) };

        buf.drain(..start_off);
        buf.truncate(ext4_rs::BLOCK_SIZE);

        buf
    }

    fn write_offset(&self, offset: usize, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let block_size = self.0.block_size();
        let start_block = offset / block_size;
        let start_off = offset % block_size;
        let end_offset = offset + data.len() - 1;
        let end_block = end_offset / block_size;
        let end_off = end_offset % block_size;

        let read_block = |block_id: usize| -> Vec<u8> {
            let mut buf = Vec::with_capacity(block_size);
            let read_bytes = self
                .0
                .read(block_id, &mut buf.spare_capacity_mut()[..block_size])
                .expect("读取不应失败（也没有报告失败的接口）")
                .len();
            assert_eq!(read_bytes, block_size);
            unsafe { buf.set_len(block_size) };
            buf
        };

        let write_block = |block_id: usize, data: &[u8]| {
            assert_eq!(data.len(), block_size);
            self.0
                .write(block_id, data)
                .expect("写入不应失败（也没有报告失败的接口）");
        };

        // 整个写入落在一个块内
        if start_block == end_block {
            let mut block_data = read_block(start_block);
            block_data[start_off..=end_off].copy_from_slice(data);
            write_block(start_block, &block_data);
            return;
        }

        let mut data_offset = 0;

        // 处理开头不完整的块
        if start_off != 0 {
            let mut block_data = read_block(start_block);
            let len = block_size - start_off;
            block_data[start_off..].copy_from_slice(&data[..len]);
            write_block(start_block, &block_data);
            data_offset += len;
        }

        // 处理中间完整的块
        let mid_start = if start_off == 0 {
            start_block
        } else {
            start_block + 1
        };
        let mid_end = if end_off == block_size - 1 {
            end_block
        } else {
            end_block - 1
        };
        if mid_start <= mid_end {
            let mid_blocks = mid_end - mid_start + 1;
            let mid_bytes = mid_blocks * block_size;
            let slice = &data[data_offset..data_offset + mid_bytes];
            self.0
                .write(mid_start, slice)
                .expect("写入不应失败（也没有报告失败的接口）");
            data_offset += mid_bytes;
        }

        // 处理结尾不完整的块
        if end_off != block_size - 1 {
            let mut block_data = read_block(end_block);
            let len = end_off + 1;
            block_data[..len].copy_from_slice(&data[data_offset..]);
            write_block(end_block, &block_data);
        }
    }
}
