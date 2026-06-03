use core::{
    ffi::{c_char, c_int, c_long, c_short, c_uchar, c_uint, c_ulong, c_ushort, c_void},
    mem::{MaybeUninit, replace, swap},
    ops::FromResidual,
    pin::Pin,
    ptr::{self, NonNull, addr_eq, from_raw_parts, from_raw_parts_mut, null_mut},
    slice,
};

use alloc::{borrow::ToOwned, collections::vec_deque::VecDeque, str, sync::Arc, vec::Vec};
use bitflags::{Flags, bitflags};
use num_enum::TryFromPrimitive;
use uninit::AsMaybeUninit;

use crate::{
    basic::{
        DropGuard,
        bstr::{ByteStr, ByteString},
        extend_lifetime_mut,
        timer::{get_cpu_tick, us_to_cpu_tick},
    },
    filesystem::{
        PipeFile,
        devfs::drivers::DeviceId,
        filesystems::get_filesystem_type,
        vfs::{
            self, DEntryArc, File, FileArc, FileFlags, FileOps, FileSystem, INodeArc, INodeMode,
            IterateDirData, MountPoint, Path, PathToWalk, PollStatus, ROOT_PATH, VfsError,
            VfsResult, walk_path,
        },
    },
    memory::uaccess::{
        copy_from_user_uninit, copy_one_from_user, copy_one_to_user, copy_slice_to_user,
        copy_str_from_user, copy_to_user, copy_vec_from_user,
    },
    multitask::{FDTableEntry, FileDescriptorFlags, current_task, signal::SignalMask},
    syscall::{SyscallResult, common_types::TimeSpec},
};

use crate::syscall::err_codes::*;

pub(super) const PATH_LEN_HINT: usize = 64;
pub(super) const FILENAME_LEN_HINT: usize = 32;
pub(super) const PATH_LEN_MAX: usize = 32768;

fn get_file_from_fd(fd: isize) -> Result<FileArc, SyscallResult> {
    if fd < 0 {
        return Err(EBADF);
    }
    let fd = fd.cast_unsigned();

    let cur_task = unsafe { current_task().as_ref_unchecked() };
    let file = cur_task.files.read().get(fd).ok_or(EBADF)?.0.clone();

    Ok(file)
}

/// 功能：获取当前工作目录；
///
/// 输入：
///
/// - char *buf：一块缓存区，用于保存当前工作目录的字符串。当buf设为NULL，由系统来分配缓存区。
///
/// - size：buf缓存区的大小。
///
/// 返回值：成功执行，则返回当前工作目录的字符串的指针。失败，则返回NULL。
///
/// ```c
/// char *buf, size_t size;
/// long ret = syscall(SYS_getcwd, buf, size);
/// ```
///
/// 注意：这里文档的语义和 Linux 调用语义不一致，我们遵循 Linux 的语义，即不支持内核代分配缓冲区
pub extern "C" fn getcwd(buf: *mut c_char, size: usize) -> SyscallResult {
    if buf.is_null() || size == 0 {
        // POSIX.1-2001 leaves the behavior of getcwd() unspecified if buf is NULL.
        // EINVAL The size argument is zero and buf is not a null pointer.
        return EINVAL;
    }

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let mut str = VecDeque::<u8>::new();

    let mut cwd = cur_task.fs_data.read().current_work_dir.clone();
    while let Some(parent) = cwd.walk_step_back_until_root() {
        // TODO: 优化（可读性和性能两方面都要）
        let prt;
        let dentry = if cwd.0.addr_eq(&cwd.1.mount_to) {
            prt = cwd.1.mount_point.get_parent();
            &prt.0
        } else {
            &cwd.0
        };

        let reversed = dentry.data.lock().name.0.clone().into_iter().rev();
        for chr in reversed {
            str.push_front(chr);
        }
        str.push_front(b'/');
        cwd = parent;
    }

    debug_assert_eq!(cwd.0.data.lock().name, b"/");
    if str.is_empty() {
        str.push_back(b'/');
    }

    str.push_back(b'\0');

    if str.len() > size {
        // ERANGE The size argument is less than the length of the absolute
        //        pathname of the working directory, including the
        //        terminating null byte.  You need to allocate a bigger array
        //        and try again.
        return ERANGE;
    }

    let k_str = Vec::from(str);

    copy_to_user(buf as *mut u8, &k_str)?;

    SyscallResult::success(buf as usize)
}

/// 功能：创建管道；
///
/// 输入：
///
/// - fd[2]：用于保存2个文件描述符。其中，fd[0]为管道的读出端，fd[1]为管道的写入端。
///
/// 返回值：成功执行，返回0。失败，返回-1。
///
/// ```c
/// int fd[2];
/// int ret = syscall(SYS_pipe2, fd, 0);
/// ```
pub extern "C" fn pipe2(fd: *mut c_int, flags: usize) -> SyscallResult {
    let cloexec = FileOpenFlags::from_bits_retain(flags as _).contains(FileOpenFlags::O_CLOEXEC);

    let (reader, writer) = PipeFile::new_anonymous();

    let func = if cloexec {
        FDTableEntry::cloexec
    } else {
        FDTableEntry::no_flags
    };

    let reader = func(reader);
    let writer = func(writer);

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let out: [c_int; 2] = {
        let reader = cur_task.files.write().insert(reader) as _;
        let writer = cur_task.files.write().insert(writer) as _;
        [reader, writer]
    };

    copy_to_user(fd.cast(), bytemuck::cast_slice(&out))?;

    SyscallResult::success(0)
}

/// 功能：复制文件描述符；
///
/// 输入：
///
/// - fd：被复制的文件描述符。
///
/// 返回值：成功执行，返回新的文件描述符。失败，返回-1。
///
/// ```c
/// int fd;
/// int ret = syscall(SYS_dup, fd);
/// ```
pub extern "C" fn dup(fd: isize) -> SyscallResult {
    if fd < 0 {
        return EBADF; // “EBADF  oldfd isn't an open file descriptor.”
    }
    let fd = fd as usize;

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let mut files_guard = cur_task.files.write();

    let file = files_guard.get(fd).ok_or(EBADF)?.0.clone();

    SyscallResult::success(files_guard.insert(FDTableEntry::no_flags(file))) // dup 不保留标志
}

/// 功能：复制文件描述符，并指定了新的文件描述符；
///
/// 输入：
///
/// - old：被复制的文件描述符。
/// - new：新的文件描述符。
///
/// 返回值：成功执行，返回新的文件描述符。失败，返回-1。
///
/// ```c
/// int old, int new;
/// int ret = syscall(SYS_dup3, old, new, 0);
/// ```
pub extern "C" fn dup3(old: isize, new: isize, flags: usize) -> SyscallResult {
    const FD_MAX: isize = 4096;

    if old == new {
        return EINVAL; // “If oldfd equals newfd, then dup3() fails with the error EINVAL.”
    }

    if new >= FD_MAX // 防止恶意用户占尽内核内存
     || old < 0 || new < 0
    {
        // “EBADF  newfd is out of the allowed range for file descriptors”
        // “EBADF  oldfd isn't an open file descriptor.”
        return EBADF;
    }

    let old = old as usize;
    let new = new as usize;

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let mut files_guard = cur_task.files.write();

    let file = files_guard.get(old).ok_or(EBADF)?.0.clone();

    let new_entry =
        if FileOpenFlags::from_bits_retain(flags as i32).contains(FileOpenFlags::O_CLOEXEC) {
            FDTableEntry::cloexec(file)
        } else {
            FDTableEntry::no_flags(file)
        };

    files_guard.set(new, new_entry);

    SyscallResult::success(new)
}

/// 功能：切换工作目录；
///
/// 输入：
///
/// - path：需要切换到的目录。
///
/// 返回值：成功执行，返回0。失败，返回-1。
///
/// ```c
/// const char *path;
/// int ret = syscall(SYS_chdir, path);
/// ```
pub extern "C" fn chdir(path: *const c_char) -> SyscallResult {
    let path = copy_str_from_user(path, PATH_LEN_HINT, PATH_LEN_MAX)?;

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let path_to_walk = PathToWalk::auto(path.as_ref(), || {
        cur_task.fs_data.read().current_work_dir.clone()
    })?;

    let new_cwd = walk_path(path_to_walk)?.ensure()?;

    cur_task.fs_data.write().current_work_dir = new_cwd;

    SyscallResult::success(0)
}

const AT_FDCWD: isize = -100;

/// 功能：打开或创建一个文件；
///
/// 输入：
///
/// - fd：文件所在目录的文件描述符。
///
/// - filename：要打开或创建的文件名。如为绝对路径，则忽略fd。如为相对路径，且fd是AT_FDCWD，则filename是相对于当前工作目录来说的。如为相对路径，且fd是一个文件描述符，则filename是相对于fd所指向的目录来说的。
/// - flags：必须包含如下访问模式的其中一种：O_RDONLY，O_WRONLY，O_RDWR。还可以包含文件创建标志和文件状态标志。
/// - mode：文件的所有权描述。详见`man 7 inode `。
///
/// 返回值：成功执行，返回新的文件描述符。失败，返回-1。
///
/// ```c
/// int fd, const char *filename, int flags, mode_t mode;
/// int ret = syscall(SYS_openat, fd, filename, flags, mode);
/// ```
pub extern "C" fn openat(
    fd: isize,
    filename: *const c_char,
    flags: isize,
    mode: usize,
) -> SyscallResult {
    let (file_open_flags, file_flags) = parse_file_open_flag(flags)?;

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let path = copy_str_from_user(filename, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let walk_path_from = parse_walk_path_from(fd, path.as_ref())?;

    log::trace!("openat fd: {fd}, path: `{path}`, flags: {flags:#x}, mode: {mode:03o}");

    let file = if file_open_flags.contains(FileOpenFlags::O_CREAT) {
        match walk_path(walk_path_from)? {
            crate::filesystem::vfs::WalkedPath::Positive(Path(dentry, mount)) => {
                if file_open_flags.contains(FileOpenFlags::O_EXCL) {
                    return EEXIST;
                }
                dentry.data.lock().ops.open(mount, file_flags)?
            }
            crate::filesystem::vfs::WalkedPath::Negative(neg) => {
                if !file_open_flags.contains(FileOpenFlags::O_CREAT) {
                    return ENOENT;
                }

                let mode = INodeMode::from_bits(mode as u16).ok_or(EINVAL)?;

                let new_inode = neg
                    .dir_inode()?
                    .data
                    .write()
                    .create(mode, neg.0.0.clone())?;
                neg.0.0.data.lock().ops.open(neg.0.1, file_flags)?
            }
        }
    } else {
        let path = walk_path(walk_path_from)?.ensure()?;

        path.0.data.lock().ops.open(path.1, file_flags)?
    };

    let fd_entry = if file_open_flags.contains(FileOpenFlags::O_CLOEXEC) {
        FDTableEntry::cloexec(file)
    } else {
        FDTableEntry::no_flags(file)
    };

    let fd = cur_task.files.write().insert(fd_entry);
    log::trace!("openat success, fd: {fd}");
    SyscallResult::success(fd)
}

bitflags! {
    struct FileOpenFlags: i32 {
        /// O_CREAT - Create the file if it does not exist
        const O_CREAT = 1 << 6;

        /// O_EXCL - Ensure that this call creates a new file
        const O_EXCL = 1 << 7;

        /// O_NOFOLLOW - Do not follow symbolic links
        const O_NOFOLLOW = 1 << 8;

        /// O_TRUNC - Truncate the file to zero length if it already exists
        const O_TRUNC = 1 << 9;

        /// O_APPEND - Append to the file, if possible
        const O_APPEND = 1 << 10;

        /// O_NONBLOCK - Open in non-blocking mode
        const O_NONBLOCK = 1 << 11;

        /// O_SYNC - Open with synchronous I/O
        const O_SYNC = 1 << 12;

        /// O_DSYNC - Open with data synchronization
        const O_DSYNC = 1 << 13;

        /// O_RSYNC - Open with read synchronization
        const O_RSYNC = 1 << 14;

        /// O_LARGEFILE - Allow large file operations (for 32-bit systems)
        const O_LARGEFILE = 1 << 15;

        const O_DIRECTORY = 1 << 16;

        /// Close-on-exec flag for the returned file descriptor
        const O_CLOEXEC = 1 << 19;

        /// O_NOATIME - Do not update file's access time
        const O_NOATIME = 1 << 20;

        /// O_TMPFILE - Create a temporary file (without a pathname)
        const O_TMPFILE = 1 << 21;
    }
}

fn parse_file_open_flag(flag: isize) -> VfsResult<(FileOpenFlags, FileFlags)> {
    let flag = flag as i32;

    let mut file_flags = match flag & 0b11 {
        0b00 => FileFlags::Read,
        0b01 => FileFlags::Write,
        0b10 => FileFlags::Read | FileFlags::Write,
        0b11 => return Err(VfsError::InvalidArguments("无效标志位")),
        _ => unreachable!(),
    };

    let open_flags = FileOpenFlags::from_bits_retain(flag & !0b11);
    if open_flags.contains_unknown_bits() {
        return Err(VfsError::NotYetImplemented);
    }

    if open_flags.contains(FileOpenFlags::O_APPEND) {
        file_flags |= FileFlags::Append;
    }

    if open_flags.contains(FileOpenFlags::O_EXCL) && !open_flags.contains(FileOpenFlags::O_CREAT) {
        return Err(VfsError::InvalidArguments("无效 flag 组合"));
    }

    if open_flags.contains(FileOpenFlags::O_DIRECTORY) && file_flags.contains(FileFlags::Write) {
        return Err(VfsError::InvalidArguments("无效 flag 组合"));
    }

    Ok((open_flags, file_flags))
}

fn parse_walk_path_from(fd: isize, path: &ByteStr) -> Result<PathToWalk<'_>, SyscallResult> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    let is_abs_path = path[0] == b'/';

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    Ok(if !is_abs_path {
        let dentry = match fd {
            AT_FDCWD => cur_task.fs_data.read().current_work_dir.clone(),
            ..0 => return Err(EBADF),
            0.. => cur_task
                .files
                .read()
                .get(fd.cast_unsigned())
                .ok_or(EBADF)?
                .0
                .data
                .lock()
                .path
                .clone(),
        };
        PathToWalk::Relative(ByteStr::new(path), dentry)
    } else {
        PathToWalk::Absolute(path)
    })
}

/// 功能：关闭一个文件描述符；
///
/// 输入：
///
/// - fd：要关闭的文件描述符。
///
/// 返回值：成功执行，返回0。失败，返回-1。
///
/// ```c
/// int fd;
/// int ret = syscall(SYS_close, fd);
/// ```
pub extern "C" fn close(fd: isize) -> SyscallResult {
    if fd < 0 {
        return EBADF;
    }

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    cur_task
        .files
        .write()
        .remove(fd.cast_unsigned())
        .ok_or(EBADF)?;
    SyscallResult::success(0)
}

/// ```c
/// struct dirent {
///     uint64 d_ino;	// 索引结点号
///     int64 d_off;	// 到下一个dirent的偏移
///     unsigned short d_reclen;	// 当前dirent的长度
///     unsigned char d_type;	// 文件类型
///     char d_name[];	//文件名
/// };
/// ```
#[repr(C)]
#[repr(align(8))]
pub struct Dirent {
    pub head: DirentHead,
    pub d_name: [c_char], // 文件名
}

#[repr(C)]
#[repr(packed)]
#[derive(Debug)]
pub struct DirentHead {
    pub d_ino: u64,         // 索引结点号
    pub d_off: i64,         // 到下一个dirent的偏移
    pub d_reclen: c_ushort, // 当前dirent的长度
    pub d_type: c_uchar,    // 文件类型
}

/// 功能：获取目录的条目;
///
/// 输入：
///
/// - fd：所要读取目录的文件描述符。
///
/// - buf：一个缓存区，用于保存所读取目录的信息。缓存区的结构如下：
///
///   ```c
///   struct dirent {
///       uint64 d_ino;	// 索引结点号
///       int64 d_off;	// 到下一个dirent的偏移
///       unsigned short d_reclen;	// 当前dirent的长度
///       unsigned char d_type;	// 文件类型
///       char d_name[];	//文件名
///   };
///   ```
///
/// - len：buf的大小。
///
/// 返回值：成功执行，返回读取的字节数。当到目录结尾，则返回0。失败，则返回-1。
///
/// ```c
/// int fd, struct dirent *buf, size_t len
/// int ret = syscall(SYS_getdents64, fd, buf, len);
/// ```
pub extern "C" fn getdents64(fd: isize, buf: *mut c_void, len: usize) -> SyscallResult {
    if len == 0 {
        return EINVAL;
    }

    let file = get_file_from_fd(fd)?;

    let mut k_buffer = Vec::<u8>::try_with_capacity(len + 7)?;

    let align_off = k_buffer.as_mut_ptr().align_offset(align_of::<DirentHead>());

    let buffer = &mut k_buffer.spare_capacity_mut()[align_off..len + align_off];
    let (data, _) = file.data.lock().iterate_dir(buffer)?;

    debug_assert!(data.len() <= len);
    copy_to_user(buf as *mut u8, data);

    SyscallResult::success(data.len())
}

/// 功能：从一个文件描述符中读取；
///
/// 输入：
///
/// - fd：要读取文件的文件描述符。
/// - buf：一个缓存区，用于存放读取的内容。
/// - count：要读取的字节数。
///
/// 返回值：成功执行，返回读取的字节数。如为0，表示文件结束。错误，则返回-1。
///
/// ```c
/// int fd, void *buf, size_t count;
/// ssize_t ret = syscall(SYS_read, fd, buf, count);
/// ```
pub extern "C" fn read(fd: isize, buf: *mut c_void, count: usize) -> SyscallResult {
    let file = get_file_from_fd(fd)?;

    let len = file.data.lock().read(buf as *mut u8, count as u64)?;

    SyscallResult::success(len as usize)
}

/// 功能：从一个文件描述符中写入；
///
/// 输入：
///
/// - fd：要写入文件的文件描述符。
/// - buf：一个缓存区，用于存放要写入的内容。
/// - count：要写入的字节数。
///
/// 返回值：成功执行，返回写入的字节数。错误，则返回-1。
///
/// ```c
/// int fd, const void *buf, size_t count;
/// ssize_t ret = syscall(SYS_write, fd, buf, count);
/// ```
pub extern "C" fn write(fd: isize, buf: *const c_void, count: usize) -> SyscallResult {
    let file = get_file_from_fd(fd)?;

    let len = file.data.lock().write(buf as *const u8, count as u64)?;

    SyscallResult::success(len as usize)
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoVec {
    iov_base: *mut c_void,
    iov_len: usize,
}

pub extern "C" fn writev(fd: isize, iov: *const IoVec, iovcnt: usize) -> SyscallResult {
    let file = get_file_from_fd(fd)?;

    let iov = copy_vec_from_user(ptr::slice_from_raw_parts(iov, iovcnt))?;

    let mut total_written = 0;

    // writev 保证原子性，因此需要大粒度锁
    {
        let mut file_lock_guard = file.data.lock();
        for IoVec { iov_base, iov_len } in iov {
            total_written += file_lock_guard.write(iov_base.cast(), iov_len as u64)?;
        }
    }

    SyscallResult::success(total_written as usize)
}

pub extern "C" fn readv(fd: isize, iov: *const IoVec, iovcnt: usize) -> SyscallResult {
    let file = get_file_from_fd(fd)?;

    let iov = copy_vec_from_user(ptr::slice_from_raw_parts(iov, iovcnt))?;

    let mut total_read = 0;

    // readv 保证原子性，因此需要大粒度锁
    {
        let mut file_lock_guard = file.data.lock();
        for IoVec { iov_base, iov_len } in iov {
            let new_read = file_lock_guard.read(iov_base.cast(), iov_len as u64)?;
            total_read += new_read;
            if new_read != iov_len as u64 {
                break;
            }
        }
    }

    SyscallResult::success(total_read as usize)
}

/// 功能：创建文件的链接；
///
/// 输入：
///
/// - olddirfd：原来的文件所在目录的文件描述符。
/// - oldpath：文件原来的名字。如果oldpath是相对路径，则它是相对于olddirfd目录而言的。如果oldpath是相对路径，且olddirfd的值为AT_FDCWD，则它是相对于当前路径而言的。如果oldpath是绝对路径，则olddirfd被忽略。
/// - newdirfd：新文件名所在的目录。
/// - newpath：文件的新名字。newpath的使用规则同oldpath。
/// - flags：在2.6.18内核之前，应置为0。其它的值详见`man 2 linkat`。
///
/// 返回值：成功执行，返回0。失败，返回-1。
///
/// ```c
/// int olddirfd, char *oldpath, int newdirfd, char *newpath, unsigned int flags
/// int ret = syscall(SYS_linkat, olddirfd, oldpath, newdirfd, newpath, flags);
/// ```
pub extern "C" fn linkat(
    olddirfd: isize,
    oldpath: *const c_char,
    newdirfd: isize,
    newpath: *const c_char,
    flags: usize,
) -> SyscallResult {
    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let oldpath = copy_str_from_user(oldpath, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let old_wpf = parse_walk_path_from(olddirfd, oldpath.as_ref())?;
    let old_dentry = walk_path(old_wpf)?.ensure()?.0;

    let newpath = copy_str_from_user(newpath, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let new_wpf = parse_walk_path_from(newdirfd, newpath.as_ref())?;
    let new_dentry = walk_path(new_wpf)?.ensure_neg()?;

    new_dentry.dir_inode()?.data.write().link(
        new_dentry.0.0,
        old_dentry.data.lock().inode.as_ref().unwrap(),
    )?;

    SyscallResult::success(0)
}

const AT_REMOVEDIR: usize = 0x200;

/// 功能：移除指定文件的链接(可用于删除文件)；
///
/// 输入：
///
/// - dirfd：要删除的链接所在的目录。
/// - path：要删除的链接的名字。如果path是相对路径，则它是相对于dirfd目录而言的。如果path是相对路径，且dirfd的值为AT_FDCWD，则它是相对于当前路径而言的。如果path是绝对路径，则dirfd被忽略。
/// - flags：可设置为0或AT_REMOVEDIR。
///
/// 返回值：成功执行，返回0。失败，返回-1。
///
/// ```c
/// int dirfd, char *path, unsigned int flags;
/// syscall(SYS_unlinkat, dirfd, path, flags);
/// ```
pub extern "C" fn unlinkat(dirfd: isize, path: *const c_char, flags: usize) -> SyscallResult {
    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let path = copy_str_from_user(path, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let walk_path_from = parse_walk_path_from(dirfd, path.as_ref())?;
    let dentry = walk_path(walk_path_from)?.ensure()?.0;
    let dir_inode = dentry
        .data
        .lock()
        .parent
        .as_ref()
        .ok_or(EBUSY)? // 根目录不允许被删除（无论是哪个文件系统的根目录，也无论是否为空）
        .data
        .lock()
        .inode
        .clone()
        .ok_or(ENOENT)?;

    if flags & AT_REMOVEDIR == 0 {
        dir_inode.data.write().unlink(&dentry)?;
        log::trace!("unlinked, fd={dirfd}, {path}");
    } else {
        dir_inode.data.write().rmdir(&dentry)?;
        log::trace!("rmdired, fd={dirfd}, {path}");
    }

    SyscallResult::success(0)
}

/// 功能：创建目录；
///
/// 输入：
///
/// - dirfd：要创建的目录所在的目录的文件描述符。
/// - path：要创建的目录的名称。如果path是相对路径，则它是相对于dirfd目录而言的。如果path是相对路径，且dirfd的值为AT_FDCWD，则它是相对于当前路径而言的。如果path是绝对路径，则dirfd被忽略。
/// - mode：文件的所有权描述。详见`man 7 inode `。
///
/// 返回值：成功执行，返回0。失败，返回-1。
///
/// ```c
/// int dirfd, const char *path, mode_t mode;
/// int ret = syscall(SYS_mkdirat, dirfd, path, mode);
/// ```
pub extern "C" fn mkdirat(dirfd: isize, path: *const c_char, mode: usize) -> SyscallResult {
    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let path = copy_str_from_user(path, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let walk_path_from = parse_walk_path_from(dirfd, path.as_ref())?;
    let dentry = walk_path(walk_path_from)?.ensure_neg()?;

    let mode = INodeMode::from_bits(mode as u16).ok_or(EINVAL)?;
    let dir_inode = dentry.dir_inode()?;
    dir_inode.data.write().mkdir(dentry.0.0, mode)?;
    SyscallResult::success(0)
}

/// * 功能：卸载文件系统；
/// * 输入：指定卸载目录，卸载参数；
/// * 返回值：成功返回0，失败返回-1；
/// ```c
/// const char *special, int flags;
/// int ret = syscall(SYS_umount2, special, flags);
/// ```
pub extern "C" fn umount2(special: *const c_char, flags: usize) -> SyscallResult {
    let _ = flags; // FIXME

    let target = copy_str_from_user(special, PATH_LEN_HINT, PATH_LEN_MAX)?;

    let target_wpf = parse_walk_path_from(AT_FDCWD, &target)?;
    let target = walk_path(target_wpf)?.ensure()?;

    // 注意 walk_path 会跟随挂载点，此时 target 是子文件系统的根目录
    if !target.1.mount_to.addr_eq(&target.0) {
        return EINVAL; // 说明并非挂载点
    }

    // TODO: busy check

    unsafe { target.1.mount_point.unmount(&target.1) };

    SyscallResult::success(0)
}

/// * 功能：挂载文件系统；
/// * 输入：
///     - special: 挂载设备；
///     - dir: 挂载点；
///     - fstype: 挂载的文件系统类型；
///     - flags: 挂载参数；
///     - data: 传递给文件系统的字符串参数，可为NULL；
/// * 返回值：成功返回0，失败返回-1；
/// ```c
/// const char *special, const char *dir, const char *fstype, unsigned long flags, const void *data;
/// int ret = syscall(SYS_mount, special, dir, fstype, flags, data);
/// ```
pub extern "C" fn mount(
    special: *const c_char,
    dir: *const c_char,
    fstype: *const c_char,
    flags: usize,
    data: *const c_void,
) -> SyscallResult {
    let device = copy_str_from_user(special, PATH_LEN_HINT, PATH_LEN_MAX)?;
    let target_dir = copy_str_from_user(dir, PATH_LEN_HINT, PATH_LEN_MAX)?;
    let fstype = copy_str_from_user(fstype, PATH_LEN_HINT, PATH_LEN_MAX)?;

    // let fs_type = get_filesystem_type(&fstype).ok_or(ENODEV)?; // FIXME?
    let _ = fstype;

    let target_wpf = parse_walk_path_from(AT_FDCWD, &target_dir)?;
    let target = walk_path(target_wpf)?.ensure()?;

    // FIXME: 目前这里直接使用了根目录所在 FS 对应块设备进行 mount
    // （功利点讲其实大概率不用 FIX 了，因为往届作品貌似都没实现或者随便实现的这部分 XP）
    let _ = device;
    let sub_fs = ROOT_PATH.1.sub_fs.clone();

    MountPoint::mount(target, sub_fs, ROOT_PATH.0.clone());

    // FIXME?
    let _ = flags;
    let _ = data;

    SyscallResult::success(0)
}

/// ```c
/// struct kstat {
/// 	dev_t st_dev;
/// 	ino_t st_ino;
/// 	mode_t st_mode;
/// 	nlink_t st_nlink;
/// 	uid_t st_uid;
/// 	gid_t st_gid;
/// 	dev_t st_rdev;
/// 	unsigned long __pad;
/// 	off_t st_size;
/// 	blksize_t st_blksize;
/// 	int __pad2;
/// 	blkcnt_t st_blocks;
/// 	long st_atime_sec;
/// 	long st_atime_nsec;
/// 	long st_mtime_sec;
/// 	long st_mtime_nsec;
/// 	long st_ctime_sec;
/// 	long st_ctime_nsec;
/// 	unsigned __unused[2];
/// };
/// ```
///
/// TODO：最好搞清楚 dev_t 这种类型的具体定义
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Kstat {
    /// 此文件所在设备
    pub st_dev: u64,
    /// inode id
    pub st_ino: u64,
    pub st_mode: u32,
    /// 硬链接数
    pub st_nlink: u32,
    pub inner: KstatInner,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct KstatInner {
    /// 拥有者 UID
    pub st_uid: u32,
    /// 拥有者 GID
    pub st_gid: u32,
    /// 此文件（inode）所代表的设备（如果有）
    pub st_rdev: u64,
    pub __pad: c_ulong,
    /// 以字节为单位的文件大小。对符号链接而言则是其包含的路径字符串长度（不含终止空字节）
    pub st_size: u64,
    /// 为高效文件系统 I/O 的首选块大小
    pub st_blksize: u32,
    pub __pad2: c_int,
    /// 分配给文件的块数，以 512 字节为单位
    ///
    /// 其值和 `st_size` 没有必然联系，因为文件允许存在空洞
    pub st_blocks: u64,
    pub st_atime: Time,
    pub st_mtime: Time,
    pub st_ctime: Time,
    pub __unused: [c_uint; 2],
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Time {
    pub sec: c_long,
    pub nsec: c_long,
}

impl Time {
    pub fn from_ns(ns: u64) -> Self {
        Self {
            sec: (ns / 1_000_000_000) as c_long,
            nsec: (ns % 1_000_000_000) as c_long,
        }
    }
    pub fn from_ms(ms: u64) -> Self {
        Self::from_ns(ms * 1_000_000)
    }
    pub fn from_sec(sec: u64) -> Self {
        Self {
            sec: sec as c_long,
            nsec: 0,
        }
    }
}

/// * 功能：获取文件状态；
/// * 输入：
///     - fd: 文件句柄；
///     - kst: 接收保存文件状态的指针；
/// ```c
/// struct kstat {
/// 	dev_t st_dev;
/// 	ino_t st_ino;
/// 	mode_t st_mode;
/// 	nlink_t st_nlink;
/// 	uid_t st_uid;
/// 	gid_t st_gid;
/// 	dev_t st_rdev;
/// 	unsigned long __pad;
/// 	off_t st_size;
/// 	blksize_t st_blksize;
/// 	int __pad2;
/// 	blkcnt_t st_blocks;
/// 	long st_atime_sec;
/// 	long st_atime_nsec;
/// 	long st_mtime_sec;
/// 	long st_mtime_nsec;
/// 	long st_ctime_sec;
/// 	long st_ctime_nsec;
/// 	unsigned __unused[2];
/// };
/// ```
/// * 返回值：成功返回0，失败返回-1；
/// ```c
/// int fd;
/// struct kstat kst;
/// int ret = syscall(SYS_fstat, fd, &kst);
/// ```
pub extern "C" fn fstat(fd: isize, kst: *mut Kstat) -> SyscallResult {
    if fd < 0 {
        return EBADF;
    }
    let fd = fd.cast_unsigned();

    let cur_task = unsafe { current_task().as_ref_unchecked() };
    let path = cur_task
        .files
        .read()
        .get(fd)
        .ok_or(EBADF)?
        .0
        .data
        .lock()
        .path
        .clone();

    fstat_common(path, kst)
}

pub extern "C" fn fstatat(
    dirfd: isize,
    pathname: *const c_char,
    buf: *mut Kstat,
    flags: usize,
) -> SyscallResult {
    let flags = FstatatFlags::from_bits(flags.try_into().map_err(|_| EINVAL)?).ok_or(EINVAL)?;

    let pathname = copy_str_from_user(pathname, FILENAME_LEN_HINT, PATH_LEN_MAX)?;

    log::trace!("fstatat dirfd: {dirfd}, pathname: `{pathname}`, flags: {flags:?}");

    let wpf = parse_walk_path_from(dirfd, pathname.as_ref())?;

    fstat_common(walk_path(wpf)?.whatever(), buf)
}

bitflags! {
    #[derive(Debug)]
    struct FstatatFlags: u16 {
        const AT_SYMLINK_NOFOLLOW = 1 << 8;
        const AT_NO_AUTOMOUNT = 1 << 11;
    }
}

fn fstat_common(path: Path, buf: *mut Kstat) -> SyscallResult {
    let Path(dentry, mount) = path;

    let inode = dentry.data.lock().inode.clone().ok_or(ENOENT)?;

    let kstat = Kstat {
        st_dev: mount.sub_fs.dev.to_u64(),
        st_ino: inode.id.into(),
        st_mode: inode.data.read().mode.bits().into(),
        st_nlink: inode.data.read().hard_links,
        inner: inode.data.read().get_kstat()?,
    };
    copy_one_to_user(buf, &kstat)?;

    debug_assert_eq!(kstat.inner.st_size, inode.data.read().file_size);

    SyscallResult::success(0)
}

pub extern "C" fn fcntl(fd: isize, op: isize, arg: usize) -> SyscallResult {
    if fd < 0 {
        return EBADF;
    }
    let fd = fd.cast_unsigned();

    let current_task = unsafe { current_task().as_ref_unchecked() };
    let fd_entry = current_task.files.read().get(fd).cloned().ok_or(EBADF)?;

    const F_DUPFD: isize = 0;
    const F_DUPFD_CLOEXEC: isize = 1030;
    const F_GETFD: isize = 1;
    const F_SETFD: isize = 2;
    // const F_GETFL: isize = 3;
    // const F_SETFL: isize = 4;

    let result = match op {
        F_DUPFD => current_task.files.write().insert_at_least(arg, fd_entry),
        F_DUPFD_CLOEXEC => current_task
            .files
            .write()
            .insert_at_least(arg, FDTableEntry::cloexec(fd_entry.0)),

        F_GETFD => fd_entry.1.into_raw(),
        F_SETFD => {
            let new_flags = FileDescriptorFlags::from_raw(arg);
            current_task.files.write().get_mut(fd).ok_or(EBADF)?.1 = new_flags; // 由于竞态，可能失败
            0
        }

        _ => {
            log::error!("fcntl: unsupported op {op}");
            return EINVAL; // EINVAL The value specified in op is not recognized by this kernel.
        }
    };

    SyscallResult::success(result)
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct StatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __reserved: i32,
}

impl From<Time> for StatxTimestamp {
    fn from(value: Time) -> Self {
        Self {
            tv_sec: value.sec,
            tv_nsec: value.nsec as u32,
            ..Default::default()
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Statx {
    stx_mask: u32,
    stx_blksize: u32,
    stx_attributes: u64,
    stx_nlink: u32,
    stx_uid: u32,
    stx_gid: u32,
    stx_mode: u16,
    __spare0: u16,
    stx_ino: u64,
    stx_size: u64,
    stx_blocks: u64,
    stx_attributes_mask: u64,
    stx_atime: StatxTimestamp,
    stx_btime: StatxTimestamp,
    stx_ctime: StatxTimestamp,
    stx_mtime: StatxTimestamp,
    stx_rdev_major: u32,
    stx_rdev_minor: u32,
    stx_dev_major: u32,
    stx_dev_minor: u32,
    stx_mnt_id: u64,
    __spare2: [u64; 13],
}

bitflags! {
    pub struct StatxMask: u16 {
        const STATX_TYPE = 1 << 0;
        const STATX_MODE = 1 << 1;
        const STATX_NLINK = 1 << 2;
        const STATX_UID = 1 << 3;
        const STATX_GID = 1 << 4;
        const STATX_ATIME = 1 << 5;
        const STATX_MTIME = 1 << 6;
        const STATX_CTIME = 1 << 7;
        const STATX_INO = 1 << 8;
        const STATX_SIZE = 1 << 9;
        const STATX_BLOCKS = 1 << 10;
        const STATX_BTIME = 1 << 11;
    }

    pub struct StatxFlags: u16 {
        const AT_SYMLINK_NOFOLLOW = 1 << 8;
        const AT_NO_AUTOMOUNT = 1 << 11;
        const AT_EMPTY_PATH = 1 << 12;
        const STATX_FORCE_SYNC = 1 << 13;
        const STATX_DONT_SYNC = 1 << 14;
    }
}

pub extern "C" fn statx(
    dirfd: isize,
    path: *const c_char,
    flags: usize,
    mask_raw: usize,
    statxbuf: *mut Statx,
) -> SyscallResult {
    if dirfd < 0 {
        return EBADF;
    }
    let dirfd = dirfd.cast_unsigned();

    let mask = StatxMask::from_bits_retain(mask_raw as _);
    let flags = StatxFlags::from_bits(flags.try_into().map_err(|_| EINVAL)?).ok_or(EINVAL)?;

    let Path(dentry, mount) = if flags.contains(StatxFlags::AT_EMPTY_PATH) {
        unsafe { current_task().as_ref_unchecked() }
            .files
            .read()
            .get(dirfd)
            .ok_or(EBADF)?
            .0
            .data
            .lock()
            .path
            .clone()
    } else {
        let path = copy_str_from_user(path, PATH_LEN_HINT, PATH_LEN_MAX)?;
        let wpf = parse_walk_path_from(dirfd.cast_signed(), &path)?;
        walk_path(wpf)?.whatever()
    };

    let inode = dentry.data.lock().inode.clone().ok_or(ENOENT)?;

    let kstat = inode.data.read().get_kstat()?;

    let DeviceId(rdev_major, rdev_minor) = DeviceId::from_u64(kstat.st_rdev);
    let DeviceId(dev_major, dev_minor) = mount.sub_fs.dev;

    let statx = Statx {
        stx_mask: mask_raw as u32,
        stx_blksize: kstat.st_blksize,
        stx_attributes: 0, // FIXME?
        stx_nlink: inode.data.read().hard_links,
        stx_uid: kstat.st_uid,
        stx_gid: kstat.st_gid,
        stx_mode: inode.data.read().mode.bits(),
        stx_ino: inode.id.into(),
        stx_size: kstat.st_size,
        stx_blocks: kstat.st_blocks,
        stx_attributes_mask: 0, // FIXME?
        stx_atime: kstat.st_atime.into(),
        stx_btime: kstat.st_ctime.into(), // FIXME
        stx_ctime: kstat.st_ctime.into(),
        stx_mtime: kstat.st_mtime.into(),
        stx_rdev_major: rdev_major.into(),
        stx_rdev_minor: rdev_minor,
        stx_dev_major: dev_major.into(),
        stx_dev_minor: dev_minor,
        stx_mnt_id: 0, // FIXME
        ..Default::default()
    };

    static_assertions::assert_not_impl_any!(Statx: Drop);
    copy_one_to_user(statxbuf, &statx)?;

    SyscallResult::success(0)
}

pub extern "C" fn splice(
    fd_in: isize,
    off_in: *mut u64,
    fd_out: isize,
    off_out: *mut u64,
    size: usize,
    flags: usize,
) -> SyscallResult {
    let end_in = get_splice_end(fd_in, off_in)?;
    let end_out = get_splice_end(fd_out, off_out)?;

    let len = size as u64;

    splice_inner(end_in, end_out, len, flags)
}

fn splice_inner(end_in: SpliceEnd, end_out: SpliceEnd, len: u64, flags: usize) -> SyscallResult {
    // 注意 splice 完全不保证原子性，因此不需要大锁
    use SpliceEnd::*;

    log::error!("TODO: 处理 flags: {flags:#x}");

    let length = match (end_in, end_out) {
        (Pipe(pipe_in), Pipe(pipe_out)) => {
            if addr_eq(pipe_in.as_ref().get_ref(), pipe_out.as_ref().get_ref()) {
                return EINVAL; // EINVAL fd_in and fd_out refer to the same pipe.
            }

            pipe_in.data.lock().kread(
                |mut data| {
                    let data_len = data.len() as u64;
                    pipe_out.data.lock().kwrite(
                        |buffer| {
                            let (first, rest) = data.split_at(buffer.len());
                            buffer.copy_from_slice(first.as_ref_uninit());
                            data = rest;
                            Ok(buffer.len() as u64)
                        },
                        data_len,
                    )
                },
                len,
            )?
        }
        (Pipe(pipe_in), File(file_out)) => pipe_in
            .data
            .lock()
            .kread(|data| file_out.write(data), len)?,
        (File(file_in), Pipe(pipe_out)) => {
            pipe_out.data.lock().kwrite(|buf| file_in.read(buf), len)?
        }

        (File(_), File(_)) => return EINVAL, // EINVAL Neither of the file descriptors refers to a pipe.
    };

    SyscallResult::success(length as usize)
}

pub extern "C" fn sendfile(
    out_fd: isize,
    in_fd: isize,
    offset: *mut u64,
    count: usize,
) -> SyscallResult {
    let end_in = get_splice_end(in_fd, offset)?;
    let end_out = get_splice_end(out_fd, core::ptr::null_mut())?; // sendfile 不支持带偏移的输出

    let len = count as u64;

    match end_out {
        pipe @ SpliceEnd::Pipe(_) => splice_inner(end_in, pipe, len, 0),
        SpliceEnd::File(file) => {
            let SpliceEndFile::File(file_out) = file else {
                unreachable!()
            };

            let SpliceEnd::File(file_in) = end_in else {
                return EINVAL; // The in_fd argument must correspond to a file which supports mmap(2)-like operations (i.e., it cannot be a socket).
            };

            let length = file_out.data.lock().kwrite(|buf| file_in.read(buf), len)?;

            SyscallResult::success(length as usize)
        }
    }
}

enum SpliceEnd {
    File(SpliceEndFile),
    Pipe(FileArc<PipeFile>),
}

enum SpliceEndFile {
    File(FileArc),
    FileWithOff(FileArc, NonNull<u64>),
}
impl SpliceEndFile {
    pub fn write(&self, mut data: &[u8]) -> VfsResult<u64> {
        let len = data.len() as u64;

        self.common(|file| {
            file.kwrite(
                |buf| {
                    let (first, rest) = data.split_at(buf.len());
                    buf.copy_from_slice(first.as_ref_uninit());
                    data = rest;
                    Ok(buf.len() as u64)
                },
                len,
            )
        })
    }

    pub fn read<'a>(&self, mut buf: &'a mut [MaybeUninit<u8>]) -> VfsResult<u64> {
        let len = buf.len() as u64;

        self.common(|file| {
            file.kread(
                |data| {
                    let slice = replace(&mut buf, &mut []); // 谢谢你，借用检查器
                    let (first, rest) = slice.split_at_mut(data.len());
                    first.copy_from_slice(data.as_ref_uninit());
                    buf = slice;
                    Ok(data.len() as u64)
                },
                len,
            )
        })
    }

    fn common(&self, op: impl FnOnce(&mut vfs::FileData) -> VfsResult<u64>) -> VfsResult<u64> {
        let (file, off_ptr) = match self {
            Self::File(file) => (file, None),
            Self::FileWithOff(file, off) => (file, Some(*off)),
        };

        if let Some(off_ptr) = off_ptr {
            let off_ptr = off_ptr.as_ptr();
            let off = copy_one_from_user(off_ptr)?;

            // TODO: 未来可以优化
            let tmp_file = {
                let file_guard = file.data.lock();
                file_guard
                    .path
                    .0
                    .data
                    .lock()
                    .ops
                    .open(file_guard.path.1.clone(), file_guard.flags)?
            };
            let mut tmp_file_guard = tmp_file.data.lock();
            tmp_file_guard.fseek(vfs::SeekFrom::Start(off))?;
            let writen = op(&mut tmp_file_guard)?;

            copy_one_to_user(off_ptr, &(off + writen))?;
            Ok(writen)
        } else {
            op(&mut file.data.lock())
        }
    }
}

fn get_splice_end(fd: isize, off: *mut u64) -> Result<SpliceEnd, SyscallResult> {
    let file = get_file_from_fd(fd)?;

    let off = NonNull::new(off);

    let is_pipe = file.get_inode().data.read().mode.contains(INodeMode::Pipe);

    match (is_pipe, off) {
        (true, None) => {
            let arc = unsafe { Pin::into_inner_unchecked(file) };
            let ptr = Arc::into_raw(arc);
            let pipe_ptr = ptr.cast();
            let pipe_arc = unsafe { Arc::from_raw(pipe_ptr) };
            let file = unsafe { Pin::new_unchecked(pipe_arc) };
            Ok(SpliceEnd::Pipe(file))
        }
        (true, Some(_)) => Err(ESPIPE),
        (false, None) => Ok(SpliceEnd::File(SpliceEndFile::File(file))),
        (false, Some(off)) => Ok(SpliceEnd::File(SpliceEndFile::FileWithOff(file, off))),
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

pub extern "C" fn ppoll(
    fds_ptr: *mut PollFd,
    nfds: usize,
    tmo_p: *const TimeSpec,
    sigmask: *const SignalMask,
) -> SyscallResult {
    let sigmask = if sigmask.is_null() {
        None
    } else {
        Some(copy_one_from_user(sigmask)?)
    };

    let timeout_us = if tmo_p.is_null() {
        None
    } else {
        let TimeSpec { tv_sec, tv_nsec } = copy_one_from_user(tmo_p)?;
        let us = tv_sec
            .checked_mul(1_000_000)
            .and_then(|s| s.checked_add(tv_nsec / 1000));
        if us.is_some_and(|value| value < 0) {
            return EINVAL;
        }
        us.map(|signed| signed.cast_unsigned())
    };

    if timeout_us == Some(0) {
        // Specifying a timeout of zero causes poll() to return immediately, even if no file descriptors are ready.
        // (A value of 0 indicates that the call timed out and no file descriptors were ready.)
        return SyscallResult::success(0);
    }

    let deadline_tick = timeout_us.map(|us| get_cpu_tick() + us_to_cpu_tick(us as usize));

    fn fd_shound_be_ignored(fd: &PollFd) -> bool {
        fd.fd < 0 || fd.events == 0
    }

    let mut fds = copy_vec_from_user(ptr::slice_from_raw_parts(fds_ptr, nfds))?;
    for fd in fds.iter_mut().filter(|fd| fd_shound_be_ignored(fd)) {
        fd.revents = 0;
    }

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    // 修改和随后恢复信号掩码
    // 内核态不会被信号打断，因此直接写即可
    let restore_sigmask_guard;
    if let Some(mask) = sigmask {
        let old_mask = replace(&mut cur_task.signals_wl.lock().mask, mask);
        restore_sigmask_guard = DropGuard::new(move || cur_task.signals_wl.lock().mask = old_mask);
    }

    let count = loop {
        let mut count = 0;

        for &mut PollFd {
            fd,
            events,
            ref mut revents,
        } in fds.iter_mut().filter(|fd| !fd_shound_be_ignored(fd))
        {
            debug_assert!(fd >= 0);
            let fd = fd as usize;

            // 注意这里需要每次轮询都查一次文件描述符表（Linux 也是这么做的）
            // 未来引入睡眠后效率其实并不会很差（本身查文件描述符表也就只是一次下标索引）
            let Some(fdt_entry) = cur_task.files.read().get(fd).cloned() else {
                count += 1; // 没错，这也是一个事件
                *revents = PollStatus::POLLNVAL.bits();
                continue;
            };

            let file = fdt_entry.0;

            let poll = file.data.lock().poll()?;
            let mask =
                PollStatus::from_bits_retain(events) | PollStatus::POLLERR | PollStatus::POLLHUP;
            let filtered = poll & mask;

            if !filtered.is_empty() {
                count += 1;
                *revents = filtered.bits();
            }
        }

        if count > 0 {
            break count;
        }

        crate::syscall::basic::yield_(); // 假装睡眠

        // 假装我们还设定了超时唤醒
        if deadline_tick.is_some_and(|ddl| ddl > get_cpu_tick()) {
            return SyscallResult::success(0); // 超时，此时不修改用户侧的 revents
        }

        // 假装我们还设定了信号唤醒
        if cur_task.signals_wl.lock().find_can_consume().is_some() {
            return EINTR;
        }
    };

    copy_slice_to_user(fds_ptr, &fds)?;
    SyscallResult::success(count)
}

#[derive(TryFromPrimitive)]
#[repr(usize)]
pub enum IoctlCommand {
    // vvv TTY vvv
    /// 获取当前串口设置
    TCGETS = 0x5401,
    /// 设置当前串口设置
    TCSETS = 0x5402,
    /// 等待输出缓冲区排空，然后设置当前串口设置
    TCSETSW = 0x5403,
    /// 等待输出缓冲区排空，丢弃所有待处理输入，然后设置当前串口设置
    TCSETSF = 0x5404,
    /// 和 [`Self::TCGETS`] 类似，但使用 termio 而非 termios
    TCGETA = 0x5405,
    /// 和 [`Self::TCSETS`] 类似，但使用 termio 而非 termios
    TCSETA = 0x5406,
    /// 和 [`Self::TCSETSW`] 类似，但使用 termio 而非 termios
    TCSETAW = 0x5407,
    /// 和 [`Self::TCSETSF`] 类似，但使用 termio 而非 termios
    TCSETAF = 0x5408,
    /// 如果终端使用异步串行数据传输，且 arg 为零，则发送一个中断信号（一串零位），持续时间为 0.25 至 0.5 秒。
    /// 如果终端未使用异步串行数据传输，则要么发送一个中断信号，要么函数不做任何操作直接返回。
    /// 当 arg 非零时，没有人知道会发生什么。
    ///
    /// （SVr4、UnixWare、Solaris 和 Linux 将非零参数的 tcsendbreak(fd, arg) 视为 tcdrain(fd)。
    /// SunOS 将 arg 视为一个倍数，发送的零位流长度为零参数时的 arg 倍。
    /// DG/UX 和 AIX 将非零的 arg 视为以毫秒计的时间间隔。HP-UX 则忽略 arg。）
    TCSBRK = 0x5409,
    /// 获取此终端上前台进程组的进程组 ID
    TIOCGPGRP = 0x540F,
    /// 设置此终端的前台进程组 ID
    TIOCSPGRP = 0x5410,
    /// 获取窗口大小
    TIOCGWINSZ = 0x5413,
    /// 设置窗口大小
    TIOCSWINSZ = 0x5414,
    // ^^^ TTY ^^^
}

pub extern "C" fn ioctl(fd: isize, op: usize, argp: *mut c_char) -> SyscallResult {
    let op = IoctlCommand::try_from(op).map_err(|_| EINVAL)?;

    let file = get_file_from_fd(fd)?;

    SyscallResult::success(file.data.lock().ioctl(op, argp.cast())?)
}
