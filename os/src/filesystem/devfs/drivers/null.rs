use core::mem::MaybeUninit;

use crate::{
    filesystem::{
        devfs::drivers::Driver,
        vfs::{self, VfsError, VfsResult},
    },
    syscall::filesystem::IoctlCommand,
};

pub struct NullDriver;

impl Driver for NullDriver {
    fn kread(
        &self,
        _visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        _len: u64,
    ) -> VfsResult<u64> {
        Ok(0)
    }

    fn kwrite(
        &self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        const DUMMY_BUFFER_BYTES: usize = 1024;
        let mut dummy_buffer = [MaybeUninit::uninit(); DUMMY_BUFFER_BYTES];

        let mut rest = len;
        while rest > 0 {
            let cnt = rest.min(DUMMY_BUFFER_BYTES as u64) as usize;

            let actual = visitor(&mut dummy_buffer[..cnt])?;
            rest -= actual;

            if cnt != actual as usize {
                break;
            }
        }

        Ok(len - rest)
    }

    fn fseek(&self, _seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        Ok(0)
    }

    fn ioctl(&self, _cmd: IoctlCommand, _arg: *mut ()) -> VfsResult<usize> {
        Err(VfsError::OperationNotSupported)
    }
}

impl NullDriver {
    #[inline]
    pub fn new() -> Self {
        Self
    }
}
