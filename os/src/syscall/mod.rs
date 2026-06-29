use core::{
    arch::asm,
    convert::Infallible,
    ops::{ControlFlow, FromResidual, Try},
    ptr::NonNull,
};

use alloc::collections::TryReserveError;

use crate::{
    filesystem::vfs::VfsError,
    memory::uaccess::CopyError,
    multitask::{
        InsertFDError,
        futex::FutexWaitError,
        task::{ForkError, LoadElfError},
        wait::SleepError,
    },
};

pub mod common;

pub mod basic;
pub mod clock;
pub mod epoll;
pub mod filesystem;
pub mod memory;
pub mod misc;
pub mod process;
pub mod signal;
pub mod sync;
pub mod user;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct SyscallFn {
    ptr: NonNull<()>,
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

    pub fn addr(&self) -> usize {
        self.ptr.as_ptr().addr()
    }
}

static_assertions::const_assert_eq!(size_of::<Option<SyscallFn>>(), size_of::<*const ()>());

const MAX_SYSCALL_COUNT: usize = 512;

pub static TABLE: [Option<SyscallFn>; MAX_SYSCALL_COUNT] = const {
    use crate::syscall::*;

    let mut table: [Option<SyscallFn>; MAX_SYSCALL_COUNT] = [None; MAX_SYSCALL_COUNT];

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
    define!(153, basic::times);
    define!(261, basic::prlimit64);
    define!(163, basic::getrlimit);

    define!(114, clock::clock_getres);
    define!(113, clock::clock_gettime);
    define!(112, clock::clock_settime);

    define!(20, epoll::epoll_create1);
    define!(21, epoll::epoll_ctl);

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
    define!(66, filesystem::writev);
    define!(65, filesystem::readv);
    define!(67, filesystem::pread64);
    define!(68, filesystem::pwrite64);
    define!(69, filesystem::preadv);
    define!(70, filesystem::pwritev);
    define!(79, filesystem::fstatat);
    define!(25, filesystem::fcntl);
    define!(291, filesystem::statx);
    define!(76, filesystem::splice);
    define!(71, filesystem::sendfile);
    define!(73, filesystem::ppoll);
    define!(29, filesystem::ioctl);
    define!(62, filesystem::lseek);
    define!(78, filesystem::readlinkat);
    define!(36, filesystem::symlinkat);
    define!(48, filesystem::faccessat);
    define!(52, filesystem::fchmod);
    define!(53, filesystem::fchmodat);
    define!(46, filesystem::ftruncate64);
    define!(54, filesystem::fchownat);
    define!(55, filesystem::fchown);
    define!(223, filesystem::fadvise64_64);
    define!(88, filesystem::utimensat);

    define!(214, memory::brk);
    define!(222, memory::mmap);
    define!(215, memory::munmap);
    define!(226, memory::mprotect);
    define!(233, memory::madvise);

    define!(172, process::getpid);
    define!(173, process::getppid);
    define!(178, process::gettid);
    define!(220, process::clone);
    define!(435, process::clone3);
    define!(221, process::execve);
    define!(260, process::wait4);
    define!(101, process::nanosleep);
    define!(96, process::set_tid_address);
    define!(94, process::exit_group);
    define!(154, process::setpgid);
    define!(155, process::getpgid);

    define!(134, signal::rt_sigaction);
    define!(129, signal::kill);
    define!(131, signal::tgkill);
    define!(130, signal::tkill);
    define!(135, signal::rt_sigprocmask);
    define!(137, signal::rt_sigtimedwait);
    define!(139, signal::rt_sigreturn);

    define!(98, sync::futex);

    define!(174, user::getuid);
    define!(175, user::geteuid);
    define!(176, user::getgid);
    define!(177, user::getegid);
    define!(147, user::setresuid);
    define!(148, user::getresuid);
    define!(149, user::setresgid);
    define!(150, user::getresgid);
    define!(145, user::setreuid);
    define!(143, user::setregid);
    define!(146, user::setuid);
    define!(144, user::setgid);

    define!(169, misc::gettimeofday);
    define!(160, misc::uname);
    define!(99, misc::set_robust_list);
    define!(100, misc::get_robust_list);
    define!(278, misc::getrandom);

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
            CopyError::CapacityOverflow => err_codes::EINVAL,
            CopyError::OutOfMemory => err_codes::ENOMEM,
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
            VfsError::IsPipeOrSocket => ESPIPE,
            VfsError::BadPipeEnd | VfsError::BrokenPipe => EPIPE,
            VfsError::BadIoctl => EINVAL,
            VfsError::TooManyLinks => ELOOP,
            VfsError::ArithmeticOverflow => EOVERFLOW,
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
            LoadElfError::NoLoadSegment => ENOEXEC,
            LoadElfError::BadElfHead => ENOEXEC,
            LoadElfError::NotEnoughMemory => ENOMEM,
            LoadElfError::BadInterpreter => ENOEXEC,
        }
    }
}

impl FromResidual<Result<Infallible, TryReserveError>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, TryReserveError>) -> Self {
        use alloc::collections::TryReserveErrorKind::*;
        use err_codes::*;
        match residual.unwrap_err().kind() {
            CapacityOverflow => EINVAL,
            AllocError { .. } => ENOMEM,
        }
    }
}

impl FromResidual<Result<Infallible, InsertFDError>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, InsertFDError>) -> Self {
        use err_codes::*;
        match residual.unwrap_err() {
            InsertFDError::ExceedRLimit => EMFILE,
        }
    }
}

impl FromResidual<Result<Infallible, FutexWaitError>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, FutexWaitError>) -> Self {
        use err_codes::*;
        match residual.unwrap_err() {
            FutexWaitError::Timeout => ETIMEDOUT,
            FutexWaitError::Signal => EINTR,
        }
    }
}

impl FromResidual<Result<Infallible, ForkError>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, ForkError>) -> Self {
        use err_codes::*;
        match residual.unwrap_err() {
            ForkError::NotEnoughTid => EAGAIN,
            ForkError::NotEnoughMemory => ENOMEM,
            ForkError::CopyError(copy_error) => Err(copy_error)?,
        }
    }
}

impl FromResidual<Result<Infallible, SleepError>> for SyscallResult {
    fn from_residual(residual: Result<Infallible, SleepError>) -> Self {
        use err_codes::*;
        match residual.unwrap_err() {
            SleepError::Interrupted(_) => EINTR,
        }
    }
}

impl SyscallResult {
    pub const fn success(val: usize) -> Self {
        debug_assert!(val.cast_signed() >= 0);
        Self { value: val }
    }

    pub const fn raw(self) -> usize {
        self.value
    }
}

mod err_codes {
    macro_rules! err_code {
        ($name:ident, $val:expr) => {
            #[allow(unused)]
            pub const $name: SyscallResult = SyscallResult {
                value: -($val as isize) as usize,
            };
        };
        ($name:ident, $val:expr, $doc:expr) => {
            #[doc = $doc]
            #[allow(unused)]
            pub const $name: SyscallResult = SyscallResult {
                value: -($val as isize) as usize,
            };
        };
    }

    use super::SyscallResult;

    // 这部分代码曾源于 NighthawkOS 的 lib/systype/src/error.rs，但已删去了所有独创性表达
    err_code!(EPERM, 1);
    err_code!(ENOENT, 2);
    err_code!(ESRCH, 3);
    err_code!(EINTR, 4);
    err_code!(EIO, 5);
    err_code!(ENXIO, 6);
    err_code!(E2BIG, 7);
    err_code!(ENOEXEC, 8);
    err_code!(EBADF, 9);
    err_code!(ECHILD, 10);
    err_code!(EAGAIN, 11);
    err_code!(ENOMEM, 12);
    err_code!(EACCES, 13);
    err_code!(EFAULT, 14);
    err_code!(ENOTBLK, 15);
    err_code!(EBUSY, 16);
    err_code!(EEXIST, 17);
    err_code!(EXDEV, 18);
    err_code!(ENODEV, 19);
    err_code!(ENOTDIR, 20);
    err_code!(EISDIR, 21);
    err_code!(EINVAL, 22);
    err_code!(ENFILE, 23);
    err_code!(EMFILE, 24);
    err_code!(ENOTTY, 25);
    err_code!(ETXTBSY, 26);
    err_code!(EFBIG, 27);
    err_code!(ENOSPC, 28);
    err_code!(ESPIPE, 29);
    err_code!(EROFS, 30);
    err_code!(EMLINK, 31);
    err_code!(EPIPE, 32);
    err_code!(EDOM, 33);
    err_code!(ERANGE, 34);
    err_code!(EDEADLK, 35);
    err_code!(ENAMETOOLONG, 36);
    err_code!(ENOLCK, 37);
    err_code!(ENOSYS, 38);
    err_code!(ENOTEMPTY, 39);
    err_code!(ELOOP, 40);
    err_code!(ENODATA, 61);
    err_code!(EOVERFLOW, 75);
    err_code!(ENOTSOCK, 88);
    err_code!(ENOPROTOOPT, 92);
    err_code!(EPROTONOSUPPORT, 93);
    err_code!(EOPNOTSUPP, 95);
    err_code!(EAFNOSUPPORT, 97);
    err_code!(EADDRINUSE, 98);
    err_code!(EADDRNOTAVAIL, 99);
    err_code!(ECONNRESET, 104);
    err_code!(EISCONN, 106);
    err_code!(ENOTCONN, 107);
    err_code!(ETIMEDOUT, 110);
    err_code!(ECONNREFUSED, 111);
    err_code!(EINPROGRESS, 115);
    err_code!(ESTALE, 116);
    err_code!(ECANCELED, 125);

    err_code!(
        E_SPECIAL_RESTART_SYSCALL,
        7777,
        "用于指定了 SA_RESTART 的信号处理程序"
    );
}

pub const ERRORCODE_NOSYS: usize = err_codes::ENOSYS.value;
pub const ERRORCODE_SPECIAL_RESTART_SYSCALL: usize = err_codes::E_SPECIAL_RESTART_SYSCALL.value;

/// 系统调用 ID 到名称的映射表，包括了尽可能多的系统调用（含未实现的那些），旨在用于调试输出。
pub const SYSCALL_ID_NAME_MAP: [&str; MAX_SYSCALL_COUNT] = {
    let mut map = ["???"; MAX_SYSCALL_COUNT];

    map[0] = "io_setup";
    map[1] = "io_destroy";
    map[2] = "io_submit";
    map[3] = "io_cancel";
    map[4] = "io_getevents";
    map[5] = "setxattr";
    map[6] = "lsetxattr";
    map[7] = "fsetxattr";
    map[8] = "getxattr";
    map[9] = "lgetxattr";
    map[10] = "fgetxattr";
    map[14] = "removexattr";
    map[16] = "fremovexattr";
    map[17] = "getcwd";
    map[19] = "eventfd2";
    map[20] = "epoll_create1";
    map[21] = "epoll_ctl";
    map[22] = "epoll_pwait";
    map[23] = "dup";
    map[24] = "dup3";
    map[25] = "fcntl";
    map[26] = "inotify_init1";
    map[27] = "inotify_add_watch";
    map[28] = "inotify_rm_watch";
    map[29] = "ioctl";
    map[33] = "mknodat";
    map[34] = "mkdirat";
    map[35] = "unlinkat";
    map[36] = "symlinkat";
    map[37] = "linkat";
    map[39] = "umount2";
    map[40] = "mount";
    map[43] = "statfs";
    map[45] = "truncate64";
    map[46] = "ftruncate64";
    map[47] = "fallocate";
    map[48] = "faccessat";
    map[49] = "chdir";
    map[50] = "fchdir";
    map[51] = "chroot";
    map[52] = "fchmod";
    map[53] = "fchmodat";
    map[54] = "fchownat";
    map[55] = "fchown";
    map[56] = "openat";
    map[57] = "close";
    map[59] = "pipe2";
    map[61] = "getdents64";
    map[62] = "lseek";
    map[63] = "read";
    map[64] = "write";
    map[65] = "readv";
    map[66] = "writev";
    map[67] = "pread64";
    map[68] = "pwrite64";
    map[69] = "preadv";
    map[70] = "pwritev";
    map[71] = "sendfile";
    map[72] = "pselect6";
    map[73] = "ppoll";
    map[74] = "signalfd64";
    map[76] = "splice";
    map[78] = "readlinkat";
    map[79] = "fstatat";
    map[80] = "fstat";
    map[81] = "sync";
    map[82] = "fsync";
    map[85] = "timerfd_create";
    map[88] = "utimensat";
    map[90] = "capget";
    map[91] = "capset";
    map[93] = "exit";
    map[94] = "exit_group";
    map[95] = "waitid";
    map[96] = "set_tid_address";
    map[98] = "futex";
    map[99] = "set_robust_list";
    map[100] = "get_robust_list";
    map[101] = "nanosleep";
    map[102] = "getitimer";
    map[103] = "setitimer";
    map[107] = "timer_create";
    map[110] = "timer_settime";
    map[112] = "clock_settime";
    map[113] = "clock_gettime";
    map[114] = "clock_getres";
    map[115] = "clock_nanosleep";
    map[116] = "syslog";
    map[119] = "sched_setscheduler";
    map[120] = "sched_getscheduler";
    map[121] = "sched_getparam";
    map[122] = "sched_setaffinity";
    map[123] = "sched_getaffinity";
    map[124] = "sched_yield";
    map[129] = "kill";
    map[130] = "tkill";
    map[131] = "tgkill";
    map[132] = "sigaltstack";
    map[133] = "rt_sigsuspend";
    map[134] = "rt_sigaction";
    map[135] = "rt_sigprocmask";
    map[136] = "rt_sigpending";
    map[137] = "rt_sigtimedwait";
    map[139] = "rt_sigreturn";
    map[143] = "setregid";
    map[144] = "setgid";
    map[145] = "setreuid";
    map[146] = "setuid";
    map[147] = "setresuid";
    map[148] = "getresuid";
    map[149] = "setresgid";
    map[150] = "getresgid";
    map[153] = "times";
    map[154] = "setpgid";
    map[155] = "getpgid";
    map[156] = "getsid";
    map[157] = "setsid";
    map[158] = "getgroups";
    map[159] = "setgroups";
    map[160] = "uname";
    map[163] = "getrlimit";
    map[165] = "getrusage";
    map[166] = "umask";
    map[167] = "prctl";
    map[169] = "gettimeofday";
    map[171] = "adjtimex";
    map[172] = "getpid";
    map[173] = "getppid";
    map[174] = "getuid";
    map[175] = "geteuid";
    map[176] = "getgid";
    map[177] = "getegid";
    map[178] = "gettid";
    map[179] = "sysinfo";
    map[194] = "shmget";
    map[195] = "shmctl";
    map[196] = "shmat";
    map[197] = "shmdt";
    map[198] = "socket";
    map[199] = "socketpair";
    map[200] = "bind";
    map[201] = "listen";
    map[202] = "accept";
    map[203] = "connect";
    map[204] = "getsockname";
    map[205] = "getpeername";
    map[206] = "sendto";
    map[207] = "recvfrom";
    map[208] = "setsockopt";
    map[209] = "getsockopt";
    map[210] = "shutdown";
    map[214] = "brk";
    map[215] = "munmap";
    map[216] = "mremap";
    map[217] = "addkey";
    map[219] = "keyctl";
    map[220] = "clone";
    map[221] = "execve";
    map[222] = "mmap";
    map[223] = "fadvise64_64";
    map[226] = "mprotect";
    map[227] = "msync";
    map[228] = "mlock";
    map[229] = "munlock";
    map[233] = "madvise";
    map[236] = "getmempolicy";
    map[241] = "perf_event_open";
    map[242] = "accept4";
    map[243] = "recvmmsg";
    map[260] = "wait4";
    map[261] = "prlimit64";
    map[262] = "fanotify_init";
    map[263] = "fanotify_mark";
    map[264] = "name_to_handle_at";
    map[265] = "open_by_handle_at";
    map[266] = "clock_adjtime";
    map[269] = "sendmmsg";
    map[276] = "renameat2";
    map[278] = "getrandom";
    map[279] = "memfd_create";
    map[280] = "bpf";
    map[282] = "userfaultfd";
    map[283] = "membarrier";
    map[285] = "copy_file_range";
    map[288] = "pkey_mprotect";
    map[289] = "pkey_alloc";
    map[290] = "pkey_free";
    map[291] = "statx";
    map[424] = "pidfd_send_signal";
    map[425] = "io_uring_setup";
    map[426] = "io_uring_enter";
    map[427] = "io_uring_register";
    map[428] = "open_tree";
    map[429] = "move_mount";
    map[430] = "fsopen";
    map[431] = "fsconfig";
    map[432] = "fsmount";
    map[433] = "fspick";
    map[434] = "pidfd_open";
    map[435] = "clone3";
    map[436] = "close_range";
    map[438] = "pidfd_getfd";
    map[447] = "memfd_secret";

    map
};
