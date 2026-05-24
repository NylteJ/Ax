#![allow(unused)] // FIXME: temp
use core::{
    arch::asm,
    convert::Infallible,
    mem::transmute,
    ops::{ControlFlow, FromResidual, Try},
    ptr::NonNull,
};

use crate::{
    basic::sync::Mutex,
    filesystem::vfs::{VfsError, VfsResult},
    memory::uaccess::CopyError,
    multitask::task::LoadElfError,
};

pub mod basic;
pub mod filesystem;
pub mod process;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct SyscallFn {
    pub(self) ptr: NonNull<()>,
}

// SAFETY: 外部无法构造 SyscallFn，只能访问 TABLE，它是只读的，也没有内部可变性
unsafe impl Sync for SyscallFn {}

impl SyscallFn {
    pub fn call(&self, args: &[usize; 6]) -> isize {
        let func_addr = self.ptr.as_ptr() as usize;

        let ret: isize;

        unsafe {
            #[cfg(target_arch = "riscv64")]
            asm!(
                "jalr ra, {func}",
                func = in(reg) func_addr,
                in("a0") args[0],
                in("a1") args[1],
                in("a2") args[2],
                in("a3") args[3],
                in("a4") args[4],
                in("a5") args[5],
                lateout("a0") ret,
                clobber_abi("C"),
            );
            #[cfg(target_arch = "loongarch64")]
            asm!(
                "jirl $ra, {func}, 0",
                func = in(reg) func_addr,
                in("$a0") args[0],
                in("$a1") args[1],
                in("$a2") args[2],
                in("$a3") args[3],
                in("$a4") args[4],
                in("$a5") args[5],
                lateout("$a0") ret,
                clobber_abi("C"),
            );
        }

        ret
    }
}

static_assertions::const_assert_eq!(size_of::<Option<SyscallFn>>(), size_of::<*const ()>());

pub static TABLE: [Option<SyscallFn>; 512] = const {
    use crate::syscall::*;

    let mut table: [Option<SyscallFn>; 512] = [None; 512];

    macro_rules! define {
        ($id:expr, $func:expr) => {
            assert!(table[$id].is_none());
            table[$id] = Some(SyscallFn {
                ptr: NonNull::new($func as *mut ()).unwrap(),
            });
        };
    }
    define!(93, basic::exit);
    define!(124, basic::yield_);

    define!(17, filesystem::getcwd);
    define!(59, filesystem::pipe2);
    define!(23, filesystem::dup);
    define!(24, filesystem::dup3);
    define!(49, filesystem::chdir);
    define!(56, filesystem::openat);
    define!(57, filesystem::close);
    define!(61, filesystem::getdents64);
    define!(63, filesystem::read);
    define!(64, filesystem::write);
    define!(37, filesystem::linkat);
    define!(35, filesystem::unlinkat);
    define!(34, filesystem::mkdirat);
    define!(39, filesystem::umount2);
    define!(40, filesystem::mount);
    define!(80, filesystem::fstat);

    define!(172, process::getpid);
    define!(173, process::getppid);
    define!(178, process::gettid);
    define!(220, process::clone);
    define!(221, process::execve);
    define!(260, process::wait4);

    table
};

#[repr(C)]
#[must_use]
pub struct SyscallResult {
    value: usize,
}

impl Try for SyscallResult {
    type Output = usize;

    type Residual = Result<Infallible, isize>;

    fn from_output(output: Self::Output) -> Self {
        debug_assert!(output.cast_signed() >= 0);
        Self { value: output }
    }

    fn branch(self) -> ControlFlow<Self::Residual, Self::Output> {
        match self.value.cast_signed() {
            x @ ..0 => ControlFlow::Break(Err(x)),
            x @ 0.. => ControlFlow::Continue(x as usize),
        }
    }
}

impl FromResidual<Result<Infallible, isize>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, isize>) -> Self {
        debug_assert!(residual.unwrap_err() < 0);
        Self {
            value: residual.unwrap_err() as usize,
        }
    }
}

impl FromResidual<Result<Infallible, CopyError>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, CopyError>) -> Self {
        match residual.unwrap_err() {
            CopyError::UserPointerNotInUserSpace | CopyError::BadAccess => err_codes::EFAULT,
            CopyError::TooLong => err_codes::ENAMETOOLONG,
        }
    }
}

impl FromResidual<Result<Infallible, SyscallResult>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, SyscallResult>) -> Self {
        residual.unwrap_err()
    }
}

impl FromResidual<Result<Infallible, VfsError>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, VfsError>) -> Self {
        use err_codes::*;
        match residual.unwrap_err() {
            VfsError::NotYetImplemented => EOPNOTSUPP,
            VfsError::FileAlreadyExists => EEXIST,
            VfsError::FileNotFound => ENOENT,
            VfsError::NotADirectory => ENOTDIR,
            VfsError::IsADirectory => EISDIR,
            VfsError::DirectoryNotEmpty => ENOTEMPTY,
            VfsError::FilesystemMismatch => EINVAL,
            VfsError::OperationNotSupported => EOPNOTSUPP,
            VfsError::OutOfRange => ERANGE,
            VfsError::DeviceNotFound => ENXIO,
            VfsError::InvalidUserPointer(copy_error) => Self::from_residual(Err(copy_error)),
            VfsError::InvalidArguments(_) => EINVAL,
            VfsError::IoError => EIO,
            VfsError::Other(_) => EINVAL,
        }
    }
}

impl FromResidual<Result<Infallible, LoadElfError>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, LoadElfError>) -> Self {
        use err_codes::*;
        match residual.unwrap_err() {
            LoadElfError::ArchMismatch => ENOEXEC,
            LoadElfError::BadAlignment => ENOEXEC,
            LoadElfError::NotYetImplemented => EOPNOTSUPP,
        }
    }
}

impl SyscallResult {
    pub fn success(val: usize) -> Self {
        debug_assert!(val.cast_signed() >= 0);
        Self { value: val }
    }

    pub fn raw(self) -> usize {
        self.value
    }
}

pub(self) mod err_codes {
    macro_rules! err_code {
        ($name:ident, $val:expr) => {
            pub const $name: SyscallResult = SyscallResult {
                value: -($val as isize) as usize,
            };
        };
        ($name:ident, $val:expr, $doc:expr) => {
            #[doc = $doc]
            pub const $name: SyscallResult = SyscallResult {
                value: -($val as isize) as usize,
            };
        };
    }

    use super::SyscallResult;

    // vvv 以下代码衍生自 NighthawkOS 的 lib/systype/src/error.rs，有修改 vvv

    err_code!(EPERM, 1, "Operation not permitted");
    err_code!(ENOENT, 2, "No such file or directory");
    err_code!(ESRCH, 3, "No such process");
    err_code!(EINTR, 4, "Interrupted system call");
    err_code!(EIO, 5, "I/O error");
    err_code!(ENXIO, 6, "No such device or address");
    err_code!(E2BIG, 7, "Argument list too long");
    err_code!(ENOEXEC, 8, "Exec format error");
    err_code!(EBADF, 9, "Bad file number");
    err_code!(ECHILD, 10, "No child processes");
    err_code!(EAGAIN, 11, "Try again");
    err_code!(ENOMEM, 12, "Out of memory");
    err_code!(EACCES, 13, "Permission denied");
    err_code!(EFAULT, 14, "Bad address");
    err_code!(ENOTBLK, 15, "Block device required");
    err_code!(EBUSY, 16, "Device or resource busy");
    err_code!(EEXIST, 17, "File exists");
    err_code!(EXDEV, 18, "Cross-device link");
    err_code!(ENODEV, 19, "No such device");
    err_code!(ENOTDIR, 20, "Not a directory");
    err_code!(EISDIR, 21, "Is a directory");
    err_code!(EINVAL, 22, "Invalid argument");
    err_code!(ENFILE, 23, "File table overflow");
    err_code!(EMFILE, 24, "Too many open files");
    err_code!(ENOTTY, 25, "Not a typewriter");
    err_code!(ETXTBSY, 26, "Text file busy");
    err_code!(EFBIG, 27, "File too large");
    err_code!(ENOSPC, 28, "No space left on device");
    err_code!(ESPIPE, 29, "Illegal seek");
    err_code!(EROFS, 30, "Read-only file system");
    err_code!(EMLINK, 31, "Too many links");
    err_code!(EPIPE, 32, "Broken pipe");
    err_code!(EDOM, 33, "Math argument out of domain of func");
    err_code!(ERANGE, 34, "Math result not representable");
    err_code!(EDEADLK, 35, "Resource deadlock would occur");
    err_code!(ENAMETOOLONG, 36, "File name too long");
    err_code!(ENOLCK, 37, "No record locks available");
    err_code!(ENOSYS, 38, "Invalid system call number");
    err_code!(ENOTEMPTY, 39, "Directory not empty");
    err_code!(ELOOP, 40, "Trap in Infinite loop");
    err_code!(ENODATA, 61, "No data");
    err_code!(EOVERFLOW, 75, "Too much data");
    err_code!(ENOTSOCK, 88, "Socket operation on non-socket");
    err_code!(ENOPROTOOPT, 92, "Protocol not available");
    err_code!(EPROTONOSUPPORT, 93, "Protocol not supported");
    err_code!(EOPNOTSUPP, 95, "Unsupported");
    err_code!(
        EAFNOSUPPORT,
        97,
        "EAFNOSUPPORT 远程端点指定的地址族不能与这种类型的套接字一起使用"
    );
    err_code!(EADDRINUSE, 98, "Socket address is already in use");
    err_code!(EADDRNOTAVAIL, 99, "Address not available");
    err_code!(ECONNRESET, 104, "Connection reset");
    err_code!(EISCONN, 106, "Transport endpoint is already connected");
    err_code!(ENOTCONN, 107, "Transport endpoint is not connected");
    err_code!(ECONNREFUSED, 111, "Connection refused");
    err_code!(
        EINPROGRESS,
        115,
        "The socket is nonblocking and the connection cannot be completed immediately.(connect.2)"
    );
    err_code!(ESTALE, 116, "Stale file.");
    err_code!(ECANCELED, 125, "operation is cancelled");
    err_code!(EUTFFAIL, 188, "UTF-8 Convert Failed");

    // ^^^ 以上代码衍生自 NighthawkOS 的 lib/systype/src/error.rs，有修改 ^^^
}

pub const ERRORCODE_NOSYS: usize = err_codes::ENOSYS.value;
