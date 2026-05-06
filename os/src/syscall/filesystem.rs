use core::{
    bstr::ByteStr,
    ffi::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_ushort, c_void},
    mem::MaybeUninit,
    ops::FromResidual,
    ptr::{from_raw_parts, from_raw_parts_mut, null_mut},
    slice,
};

use alloc::{borrow::ToOwned, bstr::ByteString, collections::vec_deque::VecDeque, str, vec::Vec};
use bitflags::{Flags, bitflags};

use crate::{
    filesystem::vfs::{
        DEntryArc, FileFlags, FileSystem, INodeArc, INodeMode, IterateDirData, VfsError, VfsResult,
    },
    memory::uaccess::{copy_from_user_uninit, copy_str_from_user, copy_to_user},
    multitask::current_task,
    syscall::SyscallResult,
};

use crate::syscall::err_codes::*;

const PATH_LEN_HINT: usize = 64;
const FILENAME_LEN_HINT: usize = 32;
const PATH_LEN_MAX: usize = 32768;

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
    loop {
        let cwd_data = cwd.data.lock();

        let Some(parent) = cwd_data.parent.clone() else {
            break;
        };
        str.prepend(cwd_data.name.0.clone().into_iter());
        drop(cwd_data);

        str.push_front(b'/');
        cwd = parent;
    }

    debug_assert_eq!(cwd.data.lock().name, b"/");
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
    ENOSYS
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

    let Some(file) = files_guard.get(fd).cloned() else {
        return EBADF; // “EBADF  oldfd isn't an open file descriptor.”
    };

    SyscallResult::success(files_guard.insert(file))
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

    let Some(file) = files_guard.get(old).cloned() else {
        return EBADF; // “EBADF  oldfd isn't an open file descriptor.”
    };

    files_guard.set(new, file);

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

    let cwd = cur_task.fs_data.read().current_work_dir.clone();

    let new_cwd = unsafe { cwd.filesystem.as_ref() }.walk_path(ByteStr::new(&path), Some(cwd))?;

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

    let cwd = cur_task.fs_data.read().current_work_dir.clone();
    let fs = cwd.get_fs();

    let path = copy_str_from_user(filename, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let walk_path_from = parse_walk_path_from(fd, path.as_ref())?;

    let file = if file_open_flags.contains(FileOpenFlags::O_CREAT) {
        let (dentry, dir_inode) = walk_path_mayneg(fs, path.as_ref(), walk_path_from.clone())?;

        let mut dentry_data_guard = dentry.data.lock();

        if let Some(old_inode) = &dentry_data_guard.inode {
            if file_open_flags.contains(FileOpenFlags::O_EXCL) {
                return EEXIST;
            }

            dentry_data_guard.dynamic_data.open(file_flags)?
        } else {
            drop(dentry_data_guard);

            if !file_open_flags.contains(FileOpenFlags::O_CREAT) {
                return ENOENT;
            }

            let mode = INodeMode::from_bits(mode as u16).ok_or(EINVAL)?;

            let new_inode = dir_inode
                .data
                .write()
                .dynamic_data
                .create(mode, dentry.clone())?;
            dentry.data.lock().dynamic_data.open(file_flags)?
        }
    } else {
        fs.walk_path(path.as_ref(), walk_path_from)?
            .data
            .lock()
            .dynamic_data
            .open(file_flags)?
    };

    let fd = cur_task.files.write().insert(file);
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
        const FSOPEN_CLOEXEC = 1 << 19;

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

fn parse_walk_path_from(fd: isize, path: &ByteStr) -> Result<Option<DEntryArc>, SyscallResult> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    let is_abs_path = path[0] == b'/';

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    Ok(if !is_abs_path {
        match fd {
            AT_FDCWD => Some(cur_task.fs_data.read().current_work_dir.clone()),
            ..0 => return Err(EBADF),
            0.. => cur_task
                .files
                .read()
                .get(fd.cast_unsigned())
                .map(|file| file.data.lock().path.clone()),
        }
    } else {
        None
    })
}

/// 返回值：找到的项的 DEntry 和它所在目录的 INode。
fn walk_path_mayneg(
    fs: &FileSystem,
    path: &ByteStr,
    walk_path_from: Option<DEntryArc>,
) -> VfsResult<(DEntryArc, INodeArc)> {
    let (dir, filename) = match path.rsplit_once(|&x| x == b'/') {
        Some((rest, filename)) => {
            let dir = if rest.is_empty() {
                // `/xxx`（或 `/`，但这种情况会在后面报错）
                fs.init_root_dentry.clone()
            } else {
                fs.walk_path(ByteStr::new(rest), walk_path_from)?
            };

            (dir, ByteStr::new(filename))
        }
        None => {
            // 相对路径且只有文件名
            debug_assert_ne!(path[0], b'/');

            (walk_path_from.unwrap(), ByteStr::new(&path))
        }
    };

    let dir_inode = dir.data.lock().inode.as_ref().cloned().unwrap();
    Ok((
        if let Some(dcache) = fs.find_dentry(dir.ptr(), filename) {
            dcache
        } else {
            dir_inode.data.read().dynamic_data.lookup(filename, dir)?
        },
        dir_inode,
    ))
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
    if fd < 0 {
        return EBADF;
    }
    let fd = fd.cast_unsigned();

    if len == 0 {
        return EINVAL;
    }

    let cur_task = unsafe { current_task().as_ref_unchecked() };
    let files_guard = cur_task.files.read();
    let file = files_guard.get(fd).cloned().ok_or(EBADF)?;

    let mut k_buffer = Vec::<u8>::with_capacity(len + 7); // TODO: 提防恶意用户导致的 OOM

    let k_buffer_uninit = k_buffer.spare_capacity_mut();

    let (data, _) = file.data.lock().iterate_dir(k_buffer_uninit)?;

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
    if fd < 0 {
        return EBADF;
    }
    let fd = fd.cast_unsigned();

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let file = cur_task.files.read().get(fd).ok_or(EBADF)?.clone();

    let len = file
        .data
        .lock()
        .dynamic_data
        .read(buf as *mut u8, count as u64)?;

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
    if fd < 0 {
        return EBADF;
    }
    let fd = fd.cast_unsigned();

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let file = cur_task.files.read().get(fd).ok_or(EBADF)?.clone();

    let len = file
        .data
        .lock()
        .dynamic_data
        .write(buf as *const u8, count as u64)?;

    SyscallResult::success(len as usize)
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

    let cwd = cur_task.fs_data.read().current_work_dir.clone();
    let fs = cwd.get_fs();

    let oldpath = copy_str_from_user(oldpath, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let old_wpf = parse_walk_path_from(olddirfd, oldpath.as_ref())?;
    let old_dentry = fs.walk_path(oldpath.as_ref(), old_wpf)?;

    let newpath = copy_str_from_user(newpath, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let new_wpf = parse_walk_path_from(newdirfd, newpath.as_ref())?;
    let (new_dentry, dir_inode) = walk_path_mayneg(fs, newpath.as_ref(), new_wpf)?;

    if new_dentry.data.lock().inode.is_some() {
        return EEXIST;
    }

    dir_inode
        .data
        .write()
        .dynamic_data
        .link(new_dentry, old_dentry.data.lock().inode.as_ref().unwrap())?;

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

    let cwd = cur_task.fs_data.read().current_work_dir.clone();
    let fs = cwd.get_fs();

    let path = copy_str_from_user(path, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let walk_path_from = parse_walk_path_from(dirfd, path.as_ref())?;
    let (dentry, dir_inode) = walk_path_mayneg(fs, path.as_ref(), walk_path_from)?;

    if flags & AT_REMOVEDIR == 0 {
        dir_inode.data.write().dynamic_data.unlink(&dentry)?;
    } else {
        dir_inode.data.write().dynamic_data.rmdir(&dentry)?;
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

    let cwd = cur_task.fs_data.read().current_work_dir.clone();
    let fs = cwd.get_fs();

    let path = copy_str_from_user(path, FILENAME_LEN_HINT, PATH_LEN_MAX)?;
    let walk_path_from = parse_walk_path_from(dirfd, path.as_ref())?;
    let (dentry, dir_inode) = walk_path_mayneg(fs, path.as_ref(), walk_path_from)?;

    if dentry.data.lock().inode.is_some() {
        return EEXIST;
    }

    let mode = INodeMode::from_bits(mode as u16).ok_or(EINVAL)?;
    dir_inode.data.write().dynamic_data.mkdir(dentry, mode)?;
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
    ENOSYS
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
    ENOSYS
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
pub(super) struct Kstat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub __pad: c_ulong,
    pub st_size: u64,
    pub st_blksize: u32,
    pub __pad2: c_int,
    pub st_blocks: u64,
    pub st_atime_sec: c_long,
    pub st_atime_nsec: c_long,
    pub st_mtime_sec: c_long,
    pub st_mtime_nsec: c_long,
    pub st_ctime_sec: c_long,
    pub st_ctime_nsec: c_long,
    pub __unused: [c_uint; 2],
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
    ENOSYS
}
