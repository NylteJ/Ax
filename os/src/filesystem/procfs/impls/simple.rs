/// 普通 procfs 文件，只读，支持从不同位置读取。使用者只需提供用于产生文件内容的函数即可。
#[macro_export]
macro_rules! simple_file {
    (file: $file:ident, dentry: $dentry:ident; content: { $($content:tt)* }) => {
        pub struct $file;
        pub struct $dentry;

        $crate::impl_file!(for $file: kread: {
            fn kread(
                &mut self,
                visitor: &mut dyn FnMut(&[u8]) -> $crate::filesystem::vfs::VfsResult<u64>,
                len: u64,
            ) -> $crate::filesystem::vfs::VfsResult<u64> {
                let content: &[u8] = {
                    $($content)*
                }.as_ref();

                let content_len = content.len() as u64;

                let pos = &mut $crate::filesystem::vfs::File::from_dyn_mut(self).data.get_mut().pos;

                let upper = (*pos + len).min(content_len);
                let actual = visitor(&content[*pos as usize..upper as usize])?;
                *pos += actual as u64;

                Ok(actual)
            }
        }, kwrite: {
            fn kwrite(
                &mut self,
                _visitor: &mut dyn FnMut(&mut [core::mem::MaybeUninit<u8>]) -> $crate::filesystem::vfs::VfsResult<u64>,
                _len: u64,
            ) -> $crate::filesystem::vfs::VfsResult<u64> {
                Err($crate::filesystem::vfs::VfsError::OperationNotSupported)
            }
        });
    };
}
