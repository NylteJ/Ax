use core::mem::MaybeUninit;

use rand::{Rng, SeedableRng, rngs::ChaCha8Rng};
use uninit::extension_traits::MaybeUninitExt;

use crate::{
    basic::{
        sync::SpinMutex,
        timer::{CLOCK_FREQ, get_cpu_tick},
    },
    filesystem::{
        devfs::drivers::Driver,
        vfs::{VfsError, VfsResult},
    },
    syscall::filesystem::IoctlCommand,
};

/// 密码学安全的伪随机数发生器
pub struct URandomDriver {
    pub engine: SpinMutex<ChaCha8Rng>,
}

impl URandomDriver {
    pub fn new() -> Self {
        // 由于硬件运行的速度很大程度上是确定的（特别是在只有内核运行时），每个 CPU tick 真正有效的熵几乎只有后面几位

        let us = *CLOCK_FREQ / 1_000_000;
        let modulo = if us > 0 {
            us
        } else {
            log::warn!(
                "CPU 时钟频率过低({})，/dev/urandom 的随机性可能会受到影响",
                *CLOCK_FREQ
            );
            *CLOCK_FREQ
        };

        let mut seed: u64 = 0;
        for _ in 0..16 {
            seed = seed.wrapping_add((get_cpu_tick() % modulo) as u64);
        }

        Self {
            engine: SpinMutex::new(ChaCha8Rng::seed_from_u64(seed)),
        }
    }
}

impl Driver for URandomDriver {
    fn kread(&self, visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>, len: u64) -> VfsResult<u64> {
        let mut rest = len;

        const CHUNK_SIZE: usize = 256;

        let mut chunk = [0; CHUNK_SIZE];
        while rest > 0 {
            let count = CHUNK_SIZE.min(rest as usize);
            self.engine.lock().fill_bytes(&mut chunk);

            let visited = visitor(&chunk[..count])?;

            rest -= visited;
            if visited != count as u64 {
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
        // 从外部注入熵
        // 对我们使用的 ChaChaN 系列算法，32 字节（256 位）的随机数种子足以表达其全部熵，
        // 因此直接混合并重新播种即可
        // 混合操作是必需的，因为外部输入很可能不可信，混合可以保证至少不丢失熵

        let mut buf = [MaybeUninit::uninit(); 32];
        let mut xored = [0; 32];

        let mut rest = len;

        while rest > 0 {
            let count = buf.len().min(rest as usize);

            let written = visitor(&mut buf[..count])?;

            let written_buf = unsafe { buf[..count].assume_init_by_ref() };
            for (xored, value) in xored.iter_mut().zip(written_buf.iter()) {
                *xored ^= *value;
            }

            rest -= written;
            if written != count as u64 {
                break;
            }
        }

        let mut engine = self.engine.lock();
        let mut seed = engine.get_seed();
        for (s, x) in seed.iter_mut().zip(xored.into_iter()) {
            *s ^= x;
        }
        *engine = ChaCha8Rng::from_seed(seed);

        Ok(len - rest)
    }

    fn ioctl(&self, _cmd: IoctlCommand, _arg: *mut ()) -> VfsResult<usize> {
        Err(VfsError::OperationNotSupported)
    }
}
