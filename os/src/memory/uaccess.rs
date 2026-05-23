use core::{ffi::c_char, mem::MaybeUninit, slice};

use alloc::{bstr::ByteString, vec::Vec};

use crate::{
    arch::{self, interfaces::UAccessRaw},
    memory::address::{VIRTUAL_ADDRESS_HIGH_BASE, VIRTUAL_ADDRESS_LOW_HIGHEST},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyError {
    UserPointerNotInUserSpace,
    BadAccess,
    TooLong,
}

/// 从用户态地址空间复制数据到内核态，总是填满 k_dest
///
/// # Safety
/// 在我们（操作系统）看来，此函数完全安全：如果用户传入了错误的指针，我们可以检测到并返回 Err，所有行为都是完全确定的。
/// （Rust 编译器可能不会这么想，但这不重要！）
pub fn copy_from_user_uninit(
    k_dest: &mut [MaybeUninit<u8>],
    u_src: *const u8,
) -> Result<&mut [u8], CopyError> {
    if u_src as usize >= VIRTUAL_ADDRESS_LOW_HIGHEST {
        // 用户传入指针必须指向用户态地址空间
        return Err(CopyError::UserPointerNotInUserSpace);
    }

    debug_assert!(k_dest.as_ptr() as usize >= VIRTUAL_ADDRESS_HIGH_BASE);

    allow_uaccess();
    let copy_ret = unsafe {
        arch::NativeUAccessRaw::__copy_uaccess(k_dest.as_mut_ptr().cast(), u_src, k_dest.len())
    };
    disallow_uaccess();

    if copy_ret == 0 {
        Ok(unsafe { k_dest.assume_init_mut() })
    } else {
        Err(CopyError::BadAccess)
    }
}

/// 从用户态地址空间复制数据到内核态，总是填满 k_dest
/// 更多信息参见 `copy_from_user_uninit`
pub fn copy_from_user(k_dest: &mut [u8], u_src: *const u8) -> Result<(), CopyError> {
    copy_from_user_uninit(
        unsafe {
            slice::from_raw_parts_mut(k_dest.as_mut_ptr() as *mut MaybeUninit<u8>, k_dest.len())
        },
        u_src,
    )?;
    Ok(())
}

/// 从内核态地址空间复制数据到用户态，总是复制整个 k_src
///
/// # Safety
/// 在我们（操作系统）看来，此函数完全安全：如果用户传入了错误的指针，我们可以检测到并返回 Err，所有行为都是完全确定的。
/// （Rust 编译器可能不会这么想，但这不重要！）
pub fn copy_to_user(u_dest: *mut u8, k_src: &[u8]) -> Result<(), CopyError> {
    if u_dest as usize >= VIRTUAL_ADDRESS_LOW_HIGHEST {
        // 用户传入指针必须指向用户态地址空间
        return Err(CopyError::UserPointerNotInUserSpace);
    }

    debug_assert!(k_src.as_ptr() as usize >= VIRTUAL_ADDRESS_HIGH_BASE);

    allow_uaccess();
    let copy_ret =
        unsafe { arch::NativeUAccessRaw::__copy_uaccess(u_dest, k_src.as_ptr(), k_src.len()) };
    disallow_uaccess();

    if copy_ret == 0 {
        Ok(())
    } else {
        Err(CopyError::BadAccess)
    }
}

/// 从用户态地址空间复制空终止字符串到 k_buf，最多复制 k_buf.len() 字节，成功时返回复制完成部分的缓冲区。复制和返回值都不包含空字符。
///
/// # Safety
/// 在我们（操作系统）看来，此函数完全安全：如果用户传入了错误的指针，我们可以检测到并返回 Err，所有行为都是完全确定的。
/// （Rust 编译器可能不会这么想，但这不重要！）
pub fn copy_str_from_user_raw_uninit(
    u_src: *const c_char,
    k_buf: &mut [MaybeUninit<u8>],
) -> Result<&mut [u8], CopyError> {
    if k_buf.is_empty() {
        return Ok(&mut []);
    }

    if u_src as usize >= VIRTUAL_ADDRESS_LOW_HIGHEST {
        // 用户传入指针必须指向用户态地址空间
        return Err(CopyError::UserPointerNotInUserSpace);
    }

    debug_assert!(k_buf.as_ptr() as usize >= VIRTUAL_ADDRESS_HIGH_BASE);

    allow_uaccess();

    let strcpy_ret = unsafe {
        arch::NativeUAccessRaw::__strcpy_uaccess(
            u_src as *const u8,
            k_buf.as_mut_ptr() as *mut u8,
            k_buf.len(),
        )
    };

    disallow_uaccess();

    if strcpy_ret == usize::MAX {
        Err(CopyError::BadAccess)
    } else {
        Ok(unsafe { slice::from_raw_parts_mut(k_buf.as_mut_ptr() as *mut u8, strcpy_ret) })
    }
}

/// 从用户态地址空间复制空终止字符串到 k_buf，最多复制 k_buf.len() 字节，成功时返回实际复制的字节数。复制和返回值都不包含空字符。
/// 更多信息参见 `copy_str_from_user_raw_uninit`
pub fn copy_str_from_user_raw(u_src: *const c_char, k_buf: &mut [u8]) -> Result<usize, CopyError> {
    Ok(copy_str_from_user_raw_uninit(u_src, unsafe {
        slice::from_raw_parts_mut(k_buf.as_mut_ptr() as *mut MaybeUninit<u8>, k_buf.len())
    })?
    .len())
}

/// 从用户态地址空间复制空终止字符串，返回值不包括空字符。
///
/// 可以使用 `length_hint` 提示字符串长度，并使用 `max_capacity` 限制最大容量。更多信息参见 `copy_str_from_user_raw_uninit`。
pub fn copy_str_from_user(
    mut u_src: *const c_char,
    length_hint: usize,
    max_capacity: usize,
) -> Result<ByteString, CopyError> {
    let length_hint = length_hint.max(1);
    let mut buffer = Vec::<u8>::new();

    while buffer.capacity() == buffer.len() {
        if buffer.capacity() > max_capacity {
            return Err(CopyError::TooLong);
        }

        buffer.reserve(length_hint);
        let len = copy_str_from_user_raw_uninit(u_src, buffer.spare_capacity_mut())?.len();

        unsafe { buffer.set_len(len) };
        u_src = unsafe { u_src.add(len) };
    }

    Ok(ByteString(buffer))
}

pub fn fill_user_memory(u_ptr: *mut u8, len: usize, val: u8) -> Result<(), CopyError> {
    if u_ptr as usize >= VIRTUAL_ADDRESS_LOW_HIGHEST {
        // 用户传入指针必须指向用户态地址空间
        return Err(CopyError::UserPointerNotInUserSpace);
    }

    allow_uaccess();
    let copy_ret = unsafe { arch::NativeUAccessRaw::__memset_uaccess(u_ptr, val, len) };
    disallow_uaccess();

    if copy_ret == 0 {
        Ok(())
    } else {
        Err(CopyError::BadAccess)
    }
}

/// 暂时允许内核态访问用户态地址空间
#[inline(always)]
fn allow_uaccess() {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sstatus::set_sum()
    };
}
/// 重新禁止内核态访问用户态地址空间
#[inline(always)]
fn disallow_uaccess() {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sstatus::clear_sum()
    };
}
