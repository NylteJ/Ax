use core::{arch::naked_asm, num::NonZero};

use crate::memory::address::{VIRTUAL_ADDRESS_HIGH_BASE, VIRTUAL_ADDRESS_LOW_HIGHEST};

pub enum CopyError {
    UserPointerNotInUserSpace,
    BadAccess(NonZero<usize>), // 额外返回未拷贝的字节数
}

/// 从用户态地址空间复制数据到内核态，总是填满 k_dest
///
/// # Safety
/// 在我们（操作系统）看来，此函数完全安全：如果用户传入了错误的指针，我们可以检测到并返回 Err，所有行为都是完全确定的。
/// （Rust 编译器可能不会这么想，但这不重要！）
pub fn copy_from_user(k_dest: &mut [u8], u_src: *const u8) -> Result<(), CopyError> {
    if u_src as usize >= VIRTUAL_ADDRESS_LOW_HIGHEST {
        // 用户传入指针必须指向用户态地址空间
        return Err(CopyError::UserPointerNotInUserSpace);
    }

    debug_assert!(k_dest.as_ptr() as usize >= VIRTUAL_ADDRESS_HIGH_BASE);

    // 暂时允许内核态访问用户态地址空间
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sstatus::set_sum();
    }

    let remaining_bytes = unsafe { __copy_uaccess(u_src, k_dest.as_mut_ptr(), k_dest.len()) };

    // 重新禁止内核态访问用户态地址空间
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sstatus::clear_sum();
    }

    NonZero::new(remaining_bytes).map_or(Ok(()), |bytes| Err(CopyError::BadAccess(bytes)))
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

    // 暂时允许内核态访问用户态地址空间
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sstatus::set_sum();
    }

    let remaining_bytes = unsafe { __copy_uaccess(k_src.as_ptr(), u_dest, k_src.len()) };

    // 重新禁止内核态访问用户态地址空间
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sstatus::clear_sum();
    }

    NonZero::new(remaining_bytes).map_or(Ok(()), |bytes| Err(CopyError::BadAccess(bytes)))
}

/// 逐字节地从 `[from, from + len)` 复制到 `[to, to + len)`。
///
/// 任意一方的指针无效时，中止复制并返回未复制的字节数，否则返回 0。
///
/// # Safety
/// 它不会引起段错误，但拿指针在权限允许的范围内乱复制仍然会导致非常坏的结果。不幸的是，内核通常拥有非常大的“权限允许的范围”
#[unsafe(naked)]
#[unsafe(link_section = ".text.uaccess")]
#[unsafe(no_mangle)]
unsafe extern "C" fn __copy_uaccess(from: *const u8, to: *mut u8, len: usize) -> usize {
    // TODO: 性能优化
    cfg_select! {
        target_arch = "riscv64" => naked_asm!(
            "beqz    a2, .LBB0_3",
            "add     a2, a2, a1",
        ".LBB0_2:",
            "lbu     a3, 0(a0)",
            "sb      a3, 0(a1)",
            "addi    a1, a1, 1",
            "addi    a0, a0, 1",
            "bne     a1, a2, .LBB0_2",
        ".LBB0_3:",
            "li      a0, 0",
            "ret",
        ".globl  __copy_uaccess_fault",
        "__copy_uaccess_fault:",
            "sub     a0, a2, a1",
            "ret",
        ),
        target_arch = "loongarch64" => naked_asm!(
            "beqz    $a2, .LBB0_2",
        ".LBB0_1:",
            "ld.b    $a3, $a0, 0",
            "st.b    $a3, $a1, 0",
            "addi.d  $a0, $a0, 1",
            "addi.d  $a2, $a2, -1",
            "addi.d  $a1, $a1, 1",
            "bnez    $a2, .LBB0_1",
        ".LBB0_2:",
            "move    $a0, $zero",
            "ret",
        ".globl  __copy_uaccess_fault",
        "__copy_uaccess_fault:",
            "move    $a0, $a2",
            "ret",
        ),
    }
}
