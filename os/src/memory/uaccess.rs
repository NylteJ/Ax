use core::{arch::naked_asm, ffi::c_char, mem::MaybeUninit, slice};

use alloc::{bstr::ByteString, vec::Vec};

use crate::memory::address::{VIRTUAL_ADDRESS_HIGH_BASE, VIRTUAL_ADDRESS_LOW_HIGHEST};

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
    let copy_ret = unsafe { __copy_uaccess(k_dest.as_mut_ptr().cast(), u_src, k_dest.len()) };
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
    let copy_ret = unsafe { __copy_uaccess(u_dest, k_src.as_ptr(), k_src.len()) };
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
        __strcpy_uaccess(
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
    let copy_ret = unsafe { __memset_uaccess(u_ptr, val, len) };
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

/// 逐字节地从 `[from, from + len)` 复制到 `[to, to + len)`。
///
/// 任意一方的指针无效时，中止复制并返回 1，否则返回 0。
///
/// # Safety
/// 它不会引起段错误，但拿指针在权限允许的范围内乱复制仍然会导致非常坏的结果。不幸的是，内核通常拥有非常大的“权限允许的范围”
#[unsafe(naked)]
#[unsafe(link_section = ".text.uaccess.copy")]
#[unsafe(no_mangle)] // no_mangle 是将 __copy_uaccess_fault 暴露出来所必需的
unsafe extern "C" fn __copy_uaccess(to: *mut u8, from: *const u8, len: usize) -> usize {
    cfg_select! {
        target_arch = "riscv64" => naked_asm!(
            // vvv 下列代码衍生自 Rust 工具链自带的 compiler_builtins::mem::memcpy 汇编，有略微改写 vvv
            "addi    sp, sp, -0x10",
            "li      a3, 0x10",
            "bltu    a2, a3, .CPY_L0",
            "neg     a3, a0",
            "andi    a6, a3, 0x7",
            "add     t3, a0, a6",
            "bgeu    a0, t3, .CPY_L1",
            "mv      a5, a6",
            "mv      a3, a0",
            "mv      a4, a1",
        ".CPY_L2:",
            "lbu     a7, 0x0(a4)",
            "addi    a5, a5, -0x1",
            "sb      a7, 0x0(a3)",
            "addi    a3, a3, 0x1",
            "addi    a4, a4, 0x1",
            "bnez    a5, .CPY_L2",
        ".CPY_L1:",
            "add     a1, a1, a6",
            "sub     t5, a2, a6",
            "andi    a5, t5, -0x8",
            "andi    a6, a1, 0x7",
            "add     a3, t3, a5",
            "bnez    a6, .CPY_L3",
            "bgeu    t3, a3, .CPY_L4",
            "mv      a4, a1",
        ".CPY_L5:",
            "ld      a2, 0x0(a4)",
            "sd      a2, 0x0(t3)",
            "addi    t3, t3, 0x8",
            "addi    a4, a4, 0x8",
            "bltu    t3, a3, .CPY_L5",
        ".CPY_L4:",
            "add     a1, a1, a5",
            "andi    a2, t5, 0x7",
            "add     a4, a3, a2",
            "bltu    a3, a4, .CPY_L6",
            "j       .CPY_L7",
        ".CPY_L0:",
            "mv      a3, a0",
            "add     a4, a0, a2",
            "bgeu    a0, a4, .CPY_L7",
        ".CPY_L6:",
            "lbu     a4, 0x0(a1)",
            "addi    a2, a2, -0x1",
            "sb      a4, 0x0(a3)",
            "addi    a3, a3, 0x1",
            "addi    a1, a1, 0x1",
            "bnez    a2, .CPY_L6",
        ".CPY_L7:",
            "addi    sp, sp, 0x10",
            "li      a0, 0x0",
            "ret",
        ".CPY_L3:",
            "li      a7, 0x0",
            "li      a4, 0x8",
            "sd      zero, 0x8(sp)",
            "sub     t1, a4, a6",
            "addi    t0, sp, 0x8",
            "andi    a4, t1, 0x1",
            "or      t0, t0, a6",
            "bnez    a4, .CPY_L8",
            "andi    a4, t1, 0x2",
            "bnez    a4, .CPY_L9",
        ".CPY_L13:",
            "andi    a4, t1, 0x4",
            "bnez    a4, .CPY_L10",
        ".CPY_L14:",
            "ld      t0, 0x8(sp)",
            "slli    a7, a6, 0x3",
            "addi    a4, t3, 0x8",
            "sub     t2, a1, a6",
            "bltu    a4, a3, .CPY_L11",
            "j       .CPY_L12",
        ".CPY_L8:",
            "lbu     a4, 0x0(a1)",
            "sb      a4, 0x0(t0)",
            "li      a7, 0x1",
            "andi    a4, t1, 0x2",
            "beqz    a4, .CPY_L13",
        ".CPY_L9:",
            "add     a4, a1, a7",
            "lh      t2, 0x0(a4)",
            "add     a4, t0, a7",
            "sh      t2, 0x0(a4)",
            "addiw   a7, a7, 0x2",
            "andi    a4, t1, 0x4",
            "beqz    a4, .CPY_L14",
        ".CPY_L10:",
            "add     a4, a1, a7",
            "lw      a4, 0x0(a4)",
            "add     a7, a7, t0",
            "sw      a4, 0x0(a7)",
            "ld      t0, 0x8(sp)",
            "slli    a7, a6, 0x3",
            "addi    a4, t3, 0x8",
            "sub     t2, a1, a6",
            "bgeu    a4, a3, .CPY_L12",
        ".CPY_L11:",
            "neg     a4, a7",
            "andi    t1, a4, 0x38",
        ".CPY_L15:",
            "mv      a4, t0",
            "ld      t0, 0x8(t2)",
            "srl     t4, a4, a7",
            "addi    t2, t2, 0x8",
            "sll     a4, t0, t1",
            "or      a4, a4, t4",
            "addi    t4, t3, 0x10",
            "sd      a4, 0x0(t3)",
            "addi    t3, t3, 0x8",
            "bltu    t4, a3, .CPY_L15",
        ".CPY_L12:",
            "li      t1, 0x0",
            "addi    t2, t2, 0x8",
            "li      a4, 0x4",
            "sd      zero, 0x8(sp)",
            "bgeu    a6, a4, .CPY_L16",
            "andi    a4, a1, 0x2",
            "bnez    a4, .CPY_L17",
        ".CPY_L19:",
            "andi    a4, a1, 0x1",
            "beqz    a4, .CPY_L18",
        ".CPY_L20:",
            "add     t2, t2, t1",
            "lbu     a6, 0x0(t2)",
            "addi    a4, sp, 0x8",
            "or      a4, a4, t1",
            "sb      a6, 0x0(a4)",
        ".CPY_L18:",
            "ld      t1, 0x8(sp)",
            "srl     a6, t0, a7",
            "neg     a4, a7",
            "andi    a4, a4, 0x38",
            "sll     a4, t1, a4",
            "or      a4, a4, a6",
            "sd      a4, 0x0(t3)",
            "add     a1, a1, a5",
            "andi    a2, t5, 0x7",
            "add     a4, a3, a2",
            "bltu    a3, a4, .CPY_L6",
            "j       .CPY_L7",
        ".CPY_L16:",
            "lw      a4, 0x0(t2)",
            "sw      a4, 0x8(sp)",
            "li      t1, 0x4",
            "andi    a4, a1, 0x2",
            "beqz    a4, .CPY_L19",
        ".CPY_L17:",
            "add     a4, t2, t1",
            "lh      a6, 0x0(a4)",
            "addi    a4, sp, 0x8",
            "or      a4, a4, t1",
            "sh      a6, 0x0(a4)",
            "addiw   t1, t1, 0x2",
            "andi    a4, a1, 0x1",
            "bnez    a4, .CPY_L20",
            "j       .CPY_L18",
            // ^^^ 以上代码衍生自 Rust 工具链自带的 compiler_builtins::mem::memcpy 汇编，有略微改写 ^^^
        ".globl  __copy_uaccess_fault",
        "__copy_uaccess_fault:",
            "addi    sp, sp, 0x10", // 改 sp 这一步是不会失败的（除非取指出问题，那就是内核 bug 了），因此失败了我们也要把 sp 改回去
            "li      a0, 0x1",
            "ret",
        ),
        target_arch = "loongarch64" => naked_asm!(
            // vvv 下列代码衍生自 Rust 工具链自带的 compiler_builtins::mem::memcpy 汇编，有略微改写 vvv
            "addi.d  $sp, $sp, -16",
            "ori     $a3, $zero, 16",
            "bltu    $a2, $a3, .CPY_L0",
            "sub.d   $a3, $zero, $a0",
            "andi    $a3, $a3, 7",
            "add.d   $a4, $a0, $a3",
            "bgeu    $a0, $a4, .CPY_L1",
            "move    $a5, $a3",
            "move    $a6, $a0",
            "move    $a7, $a1",
            "nop",
            "nop",
        ".CPY_L2:",
            "ld.b    $t0, $a7, 0",
            "st.b    $t0, $a6, 0",
            "addi.d  $a6, $a6, 1",
            "addi.d  $a5, $a5, -1",
            "addi.d  $a7, $a7, 1",
            "bnez    $a5, .CPY_L2",
        ".CPY_L1:",
            "sub.d   $a2, $a2, $a3",
            "add.d   $a1, $a1, $a3",
            "move    $a5, $a2",
            "bstrins.d       $a5, $zero, 2, 0",
            "andi    $a6, $a1, 7",
            "add.d   $a3, $a4, $a5",
            "bnez    $a6, .CPY_L3",
            "bgeu    $a4, $a3, .CPY_L4",
            "move    $a6, $a1",
            "nop",
        ".CPY_L5:",
            "ld.d    $a7, $a6, 0",
            "st.d    $a7, $a4, 0",
            "addi.d  $a4, $a4, 8",
            "addi.d  $a6, $a6, 8",
            "bltu    $a4, $a3, .CPY_L5",
        ".CPY_L4:",
            "add.d   $a1, $a1, $a5",
            "andi    $a2, $a2, 7",
            "add.d   $a4, $a3, $a2",
            "bltu    $a3, $a4, .CPY_L6",
            "b       .CPY_L7",
        ".CPY_L0:",
            "move    $a3, $a0",
            "add.d   $a4, $a3, $a2",
            "bgeu    $a3, $a4, .CPY_L7",
            "nop",
            "nop",
            "nop",
        ".CPY_L6:",
            "ld.b    $a4, $a1, 0",
            "st.b    $a4, $a3, 0",
            "addi.d  $a3, $a3, 1",
            "addi.d  $a2, $a2, -1",
            "addi.d  $a1, $a1, 1",
            "bnez    $a2, .CPY_L6",
        ".CPY_L7:",
            "addi.d  $sp, $sp, 16",
            "li.d    $a0, 0",
            "ret",
        ".CPY_L3:",
            "move    $a7, $zero",
            "ori     $t0, $zero, 8",
            "sub.d   $t1, $t0, $a6",
            "st.d    $zero, $sp, 8",
            "addi.d  $t0, $sp, 8",
            "andi    $t2, $t1, 1",
            "or      $t0, $t0, $a6",
            "bnez    $t2, .CPY_L8",
            "andi    $t2, $t1, 2",
            "bnez    $t2, .CPY_L19",
        ".CPY_L15:",
            "andi    $t1, $t1, 4",
            "beqz    $t1, .CPY_L9",
        ".CPY_L16:",
            "ldx.w   $t1, $a1, $a7",
            "stx.w   $t1, $t0, $a7",
        ".CPY_L9:",
            "ld.d    $t0, $sp, 8",
            "slli.d  $a7, $a6, 3",
            "addi.d  $t2, $a4, 8",
            "sub.d   $t1, $a1, $a6",
            "bgeu    $t2, $a3, .CPY_L10",
            "sub.d   $t2, $zero, $a7",
            "andi    $t2, $t2, 56",
        ".CPY_L11:",
            "move    $t3, $t0",
            "ld.d    $t0, $t1, 8",
            "srl.d   $t3, $t3, $a7",
            "addi.d  $t1, $t1, 8",
            "sll.d   $t4, $t0, $t2",
            "or      $t3, $t4, $t3",
            "st.d    $t3, $a4, 0",
            "addi.d  $t3, $a4, 16",
            "addi.d  $a4, $a4, 8",
            "bltu    $t3, $a3, .CPY_L11",
        ".CPY_L10:",
            "move    $t2, $zero",
            "addi.d  $t1, $t1, 8",
            "ori     $t3, $zero, 4",
            "st.d    $zero, $sp, 8",
            "bgeu    $a6, $t3, .CPY_L12",
            "andi    $a6, $a1, 2",
            "bnez    $a6, .CPY_L13",
        ".CPY_L17:",
            "andi    $a6, $a1, 1",
            "beqz    $a6, .CPY_L14",
        ".CPY_L18:",
            "ldx.b   $a6, $t1, $t2",
            "addi.d  $t1, $sp, 8",
            "or      $t1, $t1, $t2",
            "st.b    $a6, $t1, 0",
        ".CPY_L14:",
            "ld.d    $a6, $sp, 8",
            "srl.d   $t0, $t0, $a7",
            "sub.d   $a7, $zero, $a7",
            "andi    $a7, $a7, 56",
            "sll.d   $a6, $a6, $a7",
            "or      $a6, $a6, $t0",
            "st.d    $a6, $a4, 0",
            "b       .CPY_L4",
        ".CPY_L8:",
            "ld.b    $a7, $a1, 0",
            "st.b    $a7, $t0, 0",
            "ori     $a7, $zero, 1",
            "andi    $t2, $t1, 2",
            "beqz    $t2, .CPY_L15",
        ".CPY_L19:",
            "ldx.h   $t2, $a1, $a7",
            "stx.h   $t2, $t0, $a7",
            "addi.d  $a7, $a7, 2",
            "andi    $t1, $t1, 4",
            "bnez    $t1, .CPY_L16",
            "b       .CPY_L9",
        ".CPY_L12:",
            "ld.w    $a6, $t1, 0",
            "st.w    $a6, $sp, 8",
            "ori     $t2, $zero, 4",
            "andi    $a6, $a1, 2",
            "beqz    $a6, .CPY_L17",
        ".CPY_L13:",
            "ldx.h   $a6, $t1, $t2",
            "addi.d  $t3, $sp, 8",
            "or      $t3, $t3, $t2",
            "st.h    $a6, $t3, 0",
            "addi.d  $t2, $t2, 2",
            "andi    $a6, $a1, 1",
            "bnez    $a6, .CPY_L18",
            "b       .CPY_L14",
            // ^^^ 以上代码衍生自 Rust 工具链自带的 compiler_builtins::mem::memcpy 汇编，有略微改写 ^^^
        ".globl  __copy_uaccess_fault",
        "__copy_uaccess_fault:",
            "addi.d  $sp, $sp, 16", // 改 sp 这一步是不会失败的（除非取指出问题，那就是内核 bug 了），因此失败了我们也要把 sp 改回去
            "li.d    $a0, 1",
            "ret",
        ),
    }
}

/// 逐字节地从 `u_src` 复制到 `k_buf`，直到遇到第一个 `\0` 字节（不会复制该字节）或缓冲区耗尽为止。缓冲区大小由 `buf_length` 参数指定且至少为 1。
///
/// 任意一方的指针无效时，终止复制并返回 [`usize::MAX`]，否则返回复制的字节数。
///
/// # Safety
/// `buf_length` 必须大于 0，同时源和目标不得重叠。除此以外，和 `__copy_uaccess` 类似，乱用此函数仍会导致各种问题。
#[unsafe(naked)]
#[unsafe(link_section = ".text.uaccess.strcpy")]
#[unsafe(no_mangle)]
unsafe extern "C" fn __strcpy_uaccess(
    u_src: *const u8,
    k_buf: *mut u8,
    buf_length: usize,
) -> usize {
    cfg_select! {
        target_arch = "riscv64" => naked_asm!(
            "mv      a3, a0",
        ".LBB0_1:",
            "lbu     a5, 0(a3)",
            "beqz    a5, .LBB0_4",
            "sb      a5, 0(a1)",
            "addi    a3, a3, 1",
            "addi    a2, a2, -1",
            "addi    a1, a1, 1",
            "bnez    a2, .LBB0_1",
        ".LBB0_4:",
            "sub     a0, a3, a0",
            "ret",
        ".globl  __strcpy_uaccess_fault",
        "__strcpy_uaccess_fault:",
            "li      a0, {FAULT_RET}",
            "ret",
            FAULT_RET = const usize::MAX,
        ),
        target_arch = "loongarch64" => naked_asm!(
            "move    $a3, $a0",
            "move    $a0, $zero",
        ".LBB1_1:",
            "ldx.bu  $a4, $a3, $a0",
            "beqz    $a4, .LBB1_4",
            "stx.b   $a4, $a1, $a0",
            "addi.d  $a0, $a0, 1",
            "bne     $a2, $a0, .LBB1_1",
            "move    $a0, $a2",
        ".LBB1_4:",
            "ret",
        ".globl  __strcpy_uaccess_fault",
        "__strcpy_uaccess_fault:",
            "li.d    $a0, {FAULT_RET}",
            "ret",
            FAULT_RET = const usize::MAX,
        ),
    }
}

/// 将 [target, target + len) 区域内的所有字节填充为 `val`。指针无效时，中止填充并返回 1，否则返回 0。
///
/// # Safety
/// 和 `__copy_uaccess` 类似。
#[unsafe(naked)]
#[unsafe(link_section = ".text.uaccess.memset")]
#[unsafe(no_mangle)]
unsafe extern "C" fn __memset_uaccess(target: *mut u8, val: u8, len: usize) -> usize {
    cfg_select! {
        target_arch = "riscv64" => naked_asm!(
            ".option arch, +m",
            // vvv 下列代码衍生自 Rust 工具链自带的 memset 汇编，有略微改写 vvv
            "li      a3, 0x10",
            "bltu    a2, a3, .MST_L1",
            "neg     a3, a0",
            "andi    a6, a3, 0x7",
            "add     a4, a0, a6",
            "bgeu    a0, a4, .MST_L2",
            "mv      a5, a6",
            "mv      a3, a0",
        ".MST_L3:",
            "sb      a1, 0x0(a3)",
            "addi    a5, a5, -0x1",
            "addi    a3, a3, 0x1",
            "bnez    a5, .MST_L3",
        ".MST_L2:",
            "sub     a2, a2, a6",
            "andi    a3, a2, -0x8",
            "add     a3, a3, a4",
            "bgeu    a4, a3, .MST_L4",
            "slli    a6, a1, 0x38",
            "lui     a5, 0x10101",
            "slli    a5, a5, 0x4",
            "addi    a5, a5, 0x100",
            "mulhu   a6, a6, a5",
            "slli    a5, a6, 0x20",
            "or      a5, a5, a6",
        ".MST_L5:",
            "sd      a5, 0x0(a4)",
            "addi    a4, a4, 0x8",
            "bltu    a4, a3, .MST_L5",
        ".MST_L4:",
            "andi    a2, a2, 0x7",
            "add     a4, a3, a2",
            "bgeu    a3, a4, .MST_L6",
        ".MST_L7:",
            "sb      a1, 0x0(a3)",
            "addi    a2, a2, -0x1",
            "addi    a3, a3, 0x1",
            "bnez    a2, .MST_L7",
        ".MST_L6:",
            "li      a0, 0x0",
            "ret",
        ".MST_L1:",
            "mv      a3, a0",
            "add     a4, a0, a2",
            "bltu    a0, a4, .MST_L7",
            "j       .MST_L6",
            // ^^^ 以上代码衍生自 Rust 工具链自带的 memset 汇编，有略微改写 ^^^
        ".globl  __memset_uaccess_fault",
        "__memset_uaccess_fault:",
            "li      a0, 0x1",
            "ret",
        ),
        target_arch = "loongarch64" => naked_asm!(
            // vvv 下列代码衍生自 Rust 工具链自带的 memset 汇编，有略微改写 vvv
            "ori     $a3, $zero, 16",
            "bltu    $a2, $a3, .MST_L1",
            "sub.d   $a6, $zero, $a0",
            "andi    $a4, $a6, 7",
            "add.d   $a3, $a0, $a4",
            "bgeu    $a0, $a3, .MST_L2",
            "ori     $a7, $zero, 2",
            "move    $a5, $a0",
            "bltu    $a4, $a7, .MST_L3",
            "andi    $a6, $a6, 6",
            "add.d   $a5, $a0, $a6",
            "addi.d  $a7, $a0, 1",
            "move    $t0, $a6",
            "nop",
            "nop",
            "nop",
        ".MST_L4:",
            "st.b    $a1, $a7, -1",
            "st.b    $a1, $a7, 0",
            "addi.d  $t0, $t0, -2",
            "addi.d  $a7, $a7, 2",
            "bnez    $t0, .MST_L4",
            "beq     $a4, $a6, .MST_L2",
            "nop",
            "nop",
        ".MST_L3:",
            "st.b    $a1, $a5, 0",
            "addi.d  $a5, $a5, 1",
            "bne     $a5, $a3, .MST_L3",
        ".MST_L2:",
            "sub.d   $a2, $a2, $a4",
            "move    $a4, $a2",
            "bstrins.d       $a4, $zero, 2, 0",
            "add.d   $a4, $a3, $a4",
            "bgeu    $a3, $a4, .MST_L5",
            "andi    $a5, $a1, 255",
            "lu12i.w $a6, 4112",
            "ori     $a6, $a6, 257",
            "mul.d   $a5, $a5, $a6",
            "slli.d  $a6, $a5, 32",
            "or      $a5, $a6, $a5",
            "nop",
            "nop",
        ".MST_L6:",
            "st.d    $a5, $a3, 0",
            "addi.d  $a3, $a3, 8",
            "bltu    $a3, $a4, .MST_L6",
        ".MST_L5:",
            "andi    $a2, $a2, 7",
            "add.d   $a3, $a4, $a2",
            "bgeu    $a4, $a3, .MST_L7",
        ".MST_L11:",
            "ori     $a5, $zero, 2",
            "bgeu    $a2, $a5, .MST_L8",
            "move    $a5, $a4",
            "b       .MST_L9",
        ".MST_L8:",
            "andi    $a6, $a2, 14",
            "add.d   $a5, $a4, $a6",
            "addi.d  $a4, $a4, 1",
            "move    $a7, $a6",
            "nop",
            "nop",
        ".MST_L10:",
            "st.b    $a1, $a4, -1",
            "st.b    $a1, $a4, 0",
            "addi.d  $a7, $a7, -2",
            "addi.d  $a4, $a4, 2",
            "bnez    $a7, .MST_L10",
            "beq     $a2, $a6, .MST_L7",
            "nop",
            "nop",
        ".MST_L9:",
            "st.b    $a1, $a5, 0",
            "addi.d  $a5, $a5, 1",
            "bne     $a5, $a3, .MST_L9",
        ".MST_L7:",
            "li.d    $a0, 0",
            "ret",
        ".MST_L1:",
            "move    $a4, $a0",
            "add.d   $a3, $a4, $a2",
            "bltu    $a4, $a3, .MST_L11",
            "b       .MST_L7",
            // ^^^ 以上代码衍生自 Rust 工具链自带的 memset 汇编，有略微改写 ^^^
        ".globl  __memset_uaccess_fault",
        "__memset_uaccess_fault:",
            "li.d    $a0, 1",
            "ret",
        ),
    }
}
