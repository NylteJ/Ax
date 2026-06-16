use core::mem::MaybeUninit;

use crate::{
    filesystem::{
        devfs::drivers::{Driver, null::NullDriver},
        vfs::{self, VfsError, VfsResult},
    },
    memory::{PAGE_BYTES, ZERO_PAGE},
    syscall::filesystem::IoctlCommand,
};

pub struct ZeroDriver;

impl Driver for ZeroDriver {
    fn kread(&self, visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>, len: u64) -> VfsResult<u64> {
        let mut rest = len;

        while rest > 0 {
            let cnt = rest.min(PAGE_BYTES as u64) as usize;

            let actual = visitor(&ZERO_PAGE.as_bytes()[..cnt])?;
            rest -= actual;

            if cnt != actual as usize {
                break;
            }
        }

        Ok(len - rest)
    }

    fn kwrite(
        &self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        NullDriver::new().kwrite(visitor, len)
    }

    fn fseek(&self, _seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        Ok(0)
    }

    fn ioctl(&self, _cmd: IoctlCommand, _arg: *mut ()) -> VfsResult<usize> {
        Err(VfsError::OperationNotSupported)
    }
}

impl ZeroDriver {
    #[inline]
    pub fn new() -> Self {
        Self
    }
}
