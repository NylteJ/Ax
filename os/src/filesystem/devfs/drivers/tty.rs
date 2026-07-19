use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicU64, Ordering},
};

use uninit::extension_traits::MaybeUninitExt;

use crate::{
    arch::{self, interfaces::Basic},
    basic::sync::SpinMutex,
    filesystem::{
        devfs::drivers::Driver,
        vfs::{VfsError, VfsResult},
    },
    memory::uaccess::{copy_one_from_user, copy_one_to_user},
    syscall::filesystem::IoctlCommand,
};

pub struct TtyDriver {
    win_size: AtomicU64,
    termios: SpinMutex<Termios>,
}

#[repr(C)]
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy)]
struct WinSize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

impl WinSize {
    pub fn standard_terminal() -> Self {
        Self {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }

    pub fn into_u64(self) -> u64 {
        bytemuck::cast(self)
    }
    pub fn from_u64(val: u64) -> Self {
        bytemuck::cast(val)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Termios {
    /// 输入模式
    pub c_iflag: u32,
    /// 输出模式
    pub c_oflag: u32,
    /// 控制模式
    pub c_cflag: u32,
    /// 本地模式
    pub c_lflag: u32,
    /// 特殊字符
    pub c_cc: [u8; 20],
}

impl Default for Termios {
    fn default() -> Self {
        Self {
            c_iflag: 27906,
            c_oflag: 5,
            c_cflag: 1215,
            c_lflag: 35387,
            c_cc: [
                0, 3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 255, 18, 15, 23, 22, 255, 0, 0,
            ],
        }
    }
}

impl Driver for TtyDriver {
    fn kread(&self, visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>, len: u64) -> VfsResult<u64> {
        let len = len as usize;

        const BUFFER_BYTES: usize = 128;

        let mut buffer = [MaybeUninit::uninit(); BUFFER_BYTES];

        let mut i = 0;
        while i < len {
            let index = i % BUFFER_BYTES;
            let chr =
                *buffer[index].write(arch::NativeBasic::input_u8().map_err(|_| VfsError::IoError)?);

            if index == BUFFER_BYTES - 1 {
                visitor(unsafe { buffer.assume_init_by_ref() })?;
            }

            i += 1;
            if chr == b'\n' {
                break; // 行缓冲
            }
        }

        let tail = i % BUFFER_BYTES;
        if tail != 0 {
            visitor(unsafe { buffer[..tail].assume_init_by_ref() })?;
        }

        Ok(i as u64)
    }

    fn kwrite(
        &self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        let init_len = len;
        let mut len = len as usize;

        const BUFFER_BYTES: usize = 1024;

        let mut buffer = [MaybeUninit::uninit(); BUFFER_BYTES];

        while len > 0 {
            let count = len.min(BUFFER_BYTES);

            let cnt = visitor(&mut buffer[..count])? as usize;
            let buf = unsafe { buffer[..cnt].assume_init_by_ref() };
            for byte in buf {
                arch::NativeBasic::print_u8(*byte).map_err(|_| VfsError::IoError)?;
            }

            len -= cnt;
            if cnt != count {
                break;
            }
        }

        Ok(init_len - len as u64)
    }

    fn ioctl(&self, cmd: IoctlCommand, arg: *mut ()) -> VfsResult<usize> {
        use IoctlCommand::*;
        match cmd {
            TCGETS => {
                let termios = *self.termios.lock();
                copy_one_to_user(arg.cast(), &termios)?;
                Ok(0)
            }
            TCSETS => {
                let termios: Termios = copy_one_from_user(arg.cast())?;
                *self.termios.lock() = termios;
                Ok(0)
            }

            TCSETSW => todo!(),
            TCSETSF => todo!(),
            TCGETA => todo!(),
            TCSETA => todo!(),
            TCSETAW => todo!(),
            TCSETAF => todo!(),
            TCSBRK => todo!(),
            TIOCGPGRP => todo!(),
            TIOCSPGRP => todo!(),

            TIOCGWINSZ => {
                let win_size_u64 = self.win_size.load(Ordering::Relaxed);
                copy_one_to_user(arg.cast(), &WinSize::from_u64(win_size_u64))?;
                Ok(0)
            }
            TIOCSWINSZ => {
                let win_size: WinSize = copy_one_from_user(arg.cast())?;
                self.win_size.store(win_size.into_u64(), Ordering::Relaxed);
                // TODO: SIGWINCH
                Ok(0)
            }

            _ => return Err(VfsError::BadIoctl),
        }
    }
}

impl TtyDriver {
    pub fn new() -> Self {
        Self {
            win_size: AtomicU64::new(WinSize::standard_terminal().into_u64()),
            termios: SpinMutex::new(Termios::default()),
        }
    }
}
