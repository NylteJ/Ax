pub unsafe trait UAccessRaw {
    /// 逐字节地从 `[from, from + len)` 复制到 `[to, to + len)`。
    ///
    /// 任意一方的指针无效时，中止复制并返回 1，否则返回 0。
    ///
    /// # Safety
    /// 它不会引起段错误，但拿指针在权限允许的范围内乱复制仍然会导致非常坏的结果。不幸的是，内核通常拥有非常大的“权限允许的范围”

    unsafe extern "C" fn __copy_uaccess(to: *mut u8, from: *const u8, len: usize) -> usize;

    /// 逐字节地从 `u_src` 复制到 `k_buf`，直到遇到第一个 `\0` 字节（不会复制该字节）或缓冲区耗尽为止。缓冲区大小由 `buf_length` 参数指定且至少为 1。
    ///
    /// 任意一方的指针无效时，终止复制并返回 [`usize::MAX`]，否则返回复制的字节数。
    ///
    /// # Safety
    /// `buf_length` 必须大于 0，同时源和目标不得重叠。除此以外，和 `__copy_uaccess` 类似，乱用此函数仍会导致各种问题。
    unsafe extern "C" fn __strcpy_uaccess(
        u_src: *const u8,
        k_buf: *mut u8,
        buf_length: usize,
    ) -> usize;

    /// 将 [target, target + len) 区域内的所有字节填充为 `val`。指针无效时，中止填充并返回 1，否则返回 0。
    ///
    /// # Safety
    /// 和 `__copy_uaccess` 类似。
    unsafe extern "C" fn __memset_uaccess(target: *mut u8, val: u8, len: usize) -> usize;
}
