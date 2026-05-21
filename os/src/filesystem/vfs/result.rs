use crate::memory::uaccess;

pub type VfsResult<T> = core::result::Result<T, VfsError>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VfsError {
    /// 操作暂未实现
    NotYetImplemented,
    /// 文件已经存在
    FileAlreadyExists,
    /// 文件不存在
    FileNotFound,
    /// 试图在非目录对象上调用仅限目录对象的操作
    NotADirectory,
    /// 试图在目录对象上调用不支持目录对象的操作
    IsADirectory,
    /// 目录非空
    DirectoryNotEmpty,
    /// 文件系统不匹配，这几乎总是 VFS 端的逻辑错误
    FilesystemMismatch,
    /// 目标对象不支持该操作
    OperationNotSupported,
    /// 范围越界
    OutOfRange,
    /// 未找到对应设备
    DeviceNotFound,
    /// 用户态程序输入了错误的指针
    InvalidUserPointer(uaccess::CopyError),
    /// 目标对象支持该操作，但输入参数无效（如果其它更具体的类型能满足要求，请使用其他类型）
    InvalidArguments(&'static str),
    /// I/O 过程中发生异常
    IoError,
    Other(&'static str),
}

impl From<uaccess::CopyError> for VfsError {
    fn from(value: uaccess::CopyError) -> Self {
        VfsError::InvalidUserPointer(value)
    }
}

impl From<ext4_rs::Ext4Error> for VfsError {
    // 我恨孤儿规则
    fn from(value: ext4_rs::Ext4Error) -> Self {
        use VfsError::*;
        use ext4_rs::Errno::*;
        let msg = value.msg.unwrap_or("Unknown ext4 error");
        match value.error() {
            EPERM => todo!(),
            ENOENT => FileNotFound,
            EINTR => todo!(),
            EIO => IoError,
            ENXIO => DeviceNotFound,
            E2BIG => todo!(),
            EBADF => todo!(),
            EAGAIN => todo!(),
            ENOMEM => todo!(),
            EACCES => todo!(),
            EFAULT => InvalidUserPointer(uaccess::CopyError::BadAccess),
            ENOTBLK => todo!(),
            EBUSY => todo!(),
            EEXIST => FileAlreadyExists,
            EXDEV => todo!(),
            ENODEV => DeviceNotFound,
            ENOTDIR => NotADirectory,
            EISDIR => IsADirectory,
            EINVAL => InvalidArguments(msg),
            ENFILE => todo!(),
            EMFILE => todo!(),
            ENOTTY => todo!(),
            ETXTBSY => todo!(),
            EFBIG => todo!(),
            ENOSPC => todo!(),
            ESPIPE => todo!(),
            EROFS => todo!(),
            EMLINK => todo!(),
            EPIPE => todo!(),
            ENAMETOOLONG => todo!(),
            ENOTSUP => OperationNotSupported,
        }
    }
}

pub type Result<T> = VfsResult<T>;
