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
    /// 用户态程序输入了错误的指针
    InvalidUserPointer(uaccess::CopyError),
    /// 目标对象支持该操作，但输入参数无效（如果其它更具体的类型能满足要求，请使用其他类型）
    InvalidArguments(&'static str),
    Other(&'static str),
}

impl From<uaccess::CopyError> for VfsError {
    fn from(value: uaccess::CopyError) -> Self {
        VfsError::InvalidUserPointer(value)
    }
}

pub type Result<T> = VfsResult<T>;
