use core::{arch::naked_asm, num::NonZero};

use crate::memory::address::{VIRTUAL_ADDRESS_HIGH_BASE, VIRTUAL_ADDRESS_LOW_HIGHEST};

pub enum CopyError {
    UserPointerNotInUserSpace,
    BadAccess,
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

    let copy_ret = unsafe { __copy_uaccess(k_dest.as_mut_ptr(), u_src, k_dest.len()) };

    // 重新禁止内核态访问用户态地址空间
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sstatus::clear_sum();
    }

    if copy_ret == 0 {
        Ok(())
    } else {
        Err(CopyError::BadAccess)
    }
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

    let copy_ret = unsafe { __copy_uaccess(u_dest, k_src.as_ptr(), k_src.len()) };

    // 重新禁止内核态访问用户态地址空间
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sstatus::clear_sum();
    }

    if copy_ret == 0 {
        Ok(())
    } else {
        Err(CopyError::BadAccess)
    }
}

/// 逐字节地从 `[from, from + len)` 复制到 `[to, to + len)`。
///
/// 任意一方的指针无效时，中止复制并返回 1，否则返回 0。
///
/// # Safety
/// 它不会引起段错误，但拿指针在权限允许的范围内乱复制仍然会导致非常坏的结果。不幸的是，内核通常拥有非常大的“权限允许的范围”
#[unsafe(naked)]
#[unsafe(link_section = ".text.uaccess")]
#[unsafe(no_mangle)]
unsafe extern "C" fn __copy_uaccess(to: *mut u8, from: *const u8, len: usize) -> usize {
    cfg_select! {
        target_arch = "riscv64" => naked_asm!(
            // vvv 下列代码衍生自 Rust 工具链自带的 compiler_builtins::mem::memcpy 汇编，有略微改写 vvv
            "addi    sp, sp, -0x10",
            "li      a3, 0x10",
            "bltu    a2, a3, .LABEL0",
            "neg     a3, a0",
            "andi    a6, a3, 0x7",
            "add     t3, a0, a6",
            "bgeu    a0, t3, .LABEL1",
            "mv      a5, a6",
            "mv      a3, a0",
            "mv      a4, a1",
        ".LABEL2:",
            "lbu     a7, 0x0(a4)",
            "addi    a5, a5, -0x1",
            "sb      a7, 0x0(a3)",
            "addi    a3, a3, 0x1",
            "addi    a4, a4, 0x1",
            "bnez    a5, .LABEL2",
        ".LABEL1:",
            "add     a1, a1, a6",
            "sub     t5, a2, a6",
            "andi    a5, t5, -0x8",
            "andi    a6, a1, 0x7",
            "add     a3, t3, a5",
            "bnez    a6, .LABEL3",
            "bgeu    t3, a3, .LABEL4",
            "mv      a4, a1",
        ".LABEL5:",
            "ld      a2, 0x0(a4)",
            "sd      a2, 0x0(t3)",
            "addi    t3, t3, 0x8",
            "addi    a4, a4, 0x8",
            "bltu    t3, a3, .LABEL5",
        ".LABEL4:",
            "add     a1, a1, a5",
            "andi    a2, t5, 0x7",
            "add     a4, a3, a2",
            "bltu    a3, a4, .LABEL6",
            "j       .LABEL7",
        ".LABEL0:",
            "mv      a3, a0",
            "add     a4, a0, a2",
            "bgeu    a0, a4, .LABEL7",
        ".LABEL6:",
            "lbu     a4, 0x0(a1)",
            "addi    a2, a2, -0x1",
            "sb      a4, 0x0(a3)",
            "addi    a3, a3, 0x1",
            "addi    a1, a1, 0x1",
            "bnez    a2, .LABEL6",
        ".LABEL7:",
            "addi    sp, sp, 0x10",
            "li      a0, 0x0",
            "ret",
        ".LABEL3:",
            "li      a7, 0x0",
            "li      a4, 0x8",
            "sd      zero, 0x8(sp)",
            "sub     t1, a4, a6",
            "addi    t0, sp, 0x8",
            "andi    a4, t1, 0x1",
            "or      t0, t0, a6",
            "bnez    a4, .LABEL8",
            "andi    a4, t1, 0x2",
            "bnez    a4, .LABEL9",
        ".LABEL13:",
            "andi    a4, t1, 0x4",
            "bnez    a4, .LABEL10",
        ".LABEL14:",
            "ld      t0, 0x8(sp)",
            "slli    a7, a6, 0x3",
            "addi    a4, t3, 0x8",
            "sub     t2, a1, a6",
            "bltu    a4, a3, .LABEL11",
            "j       .LABEL12",
        ".LABEL8:",
            "lbu     a4, 0x0(a1)",
            "sb      a4, 0x0(t0)",
            "li      a7, 0x1",
            "andi    a4, t1, 0x2",
            "beqz    a4, .LABEL13",
        ".LABEL9:",
            "add     a4, a1, a7",
            "lh      t2, 0x0(a4)",
            "add     a4, t0, a7",
            "sh      t2, 0x0(a4)",
            "addiw   a7, a7, 0x2",
            "andi    a4, t1, 0x4",
            "beqz    a4, .LABEL14",
        ".LABEL10:",
            "add     a4, a1, a7",
            "lw      a4, 0x0(a4)",
            "add     a7, a7, t0",
            "sw      a4, 0x0(a7)",
            "ld      t0, 0x8(sp)",
            "slli    a7, a6, 0x3",
            "addi    a4, t3, 0x8",
            "sub     t2, a1, a6",
            "bgeu    a4, a3, .LABEL12",
        ".LABEL11:",
            "neg     a4, a7",
            "andi    t1, a4, 0x38",
        ".LABEL15:",
            "mv      a4, t0",
            "ld      t0, 0x8(t2)",
            "srl     t4, a4, a7",
            "addi    t2, t2, 0x8",
            "sll     a4, t0, t1",
            "or      a4, a4, t4",
            "addi    t4, t3, 0x10",
            "sd      a4, 0x0(t3)",
            "addi    t3, t3, 0x8",
            "bltu    t4, a3, .LABEL15",
        ".LABEL12:",
            "li      t1, 0x0",
            "addi    t2, t2, 0x8",
            "li      a4, 0x4",
            "sd      zero, 0x8(sp)",
            "bgeu    a6, a4, .LABEL16",
            "andi    a4, a1, 0x2",
            "bnez    a4, .LABEL17",
        ".LABEL19:",
            "andi    a4, a1, 0x1",
            "beqz    a4, .LABEL18",
        ".LABEL20:",
            "add     t2, t2, t1",
            "lbu     a6, 0x0(t2)",
            "addi    a4, sp, 0x8",
            "or      a4, a4, t1",
            "sb      a6, 0x0(a4)",
        ".LABEL18:",
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
            "bltu    a3, a4, .LABEL6",
            "j       .LABEL7",
        ".LABEL16:",
            "lw      a4, 0x0(t2)",
            "sw      a4, 0x8(sp)",
            "li      t1, 0x4",
            "andi    a4, a1, 0x2",
            "beqz    a4, .LABEL19",
        ".LABEL17:",
            "add     a4, t2, t1",
            "lh      a6, 0x0(a4)",
            "addi    a4, sp, 0x8",
            "or      a4, a4, t1",
            "sh      a6, 0x0(a4)",
            "addiw   t1, t1, 0x2",
            "andi    a4, a1, 0x1",
            "bnez    a4, .LABEL20",
            "j       .LABEL18",
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
            "bltu    $a2, $a3, .LABEL0",
            "sub.d   $a3, $zero, $a0",
            "andi    $a3, $a3, 7",
            "add.d   $a4, $a0, $a3",
            "bgeu    $a0, $a4, .LABEL1",
            "move    $a5, $a3",
            "move    $a6, $a0",
            "move    $a7, $a1",
            "nop",
            "nop",
        ".LABEL2:",
            "ld.b    $t0, $a7, 0",
            "st.b    $t0, $a6, 0",
            "addi.d  $a6, $a6, 1",
            "addi.d  $a5, $a5, -1",
            "addi.d  $a7, $a7, 1",
            "bnez    $a5, .LABEL2",
        ".LABEL1:",
            "sub.d   $a2, $a2, $a3",
            "add.d   $a1, $a1, $a3",
            "move    $a5, $a2",
            "bstrins.d       $a5, $zero, 2, 0",
            "andi    $a6, $a1, 7",
            "add.d   $a3, $a4, $a5",
            "bnez    $a6, .LABEL3",
            "bgeu    $a4, $a3, .LABEL4",
            "move    $a6, $a1",
            "nop",
        ".LABEL5:",
            "ld.d    $a7, $a6, 0",
            "st.d    $a7, $a4, 0",
            "addi.d  $a4, $a4, 8",
            "addi.d  $a6, $a6, 8",
            "bltu    $a4, $a3, .LABEL5",
        ".LABEL4:",
            "add.d   $a1, $a1, $a5",
            "andi    $a2, $a2, 7",
            "add.d   $a4, $a3, $a2",
            "bltu    $a3, $a4, .LABEL6",
            "b       .LABEL7",
        ".LABEL0:",
            "move    $a3, $a0",
            "add.d   $a4, $a3, $a2",
            "bgeu    $a3, $a4, .LABEL7",
            "nop",
            "nop",
            "nop",
        ".LABEL6:",
            "ld.b    $a4, $a1, 0",
            "st.b    $a4, $a3, 0",
            "addi.d  $a3, $a3, 1",
            "addi.d  $a2, $a2, -1",
            "addi.d  $a1, $a1, 1",
            "bnez    $a2, .LABEL6",
        ".LABEL7:",
            "addi.d  $sp, $sp, 16",
            "li.d    $a0, 0",
            "ret",
        ".LABEL3:",
            "move    $a7, $zero",
            "ori     $t0, $zero, 8",
            "sub.d   $t1, $t0, $a6",
            "st.d    $zero, $sp, 8",
            "addi.d  $t0, $sp, 8",
            "andi    $t2, $t1, 1",
            "or      $t0, $t0, $a6",
            "bnez    $t2, .LABEL8",
            "andi    $t2, $t1, 2",
            "bnez    $t2, .LABEL19",
        ".LABEL15:",
            "andi    $t1, $t1, 4",
            "beqz    $t1, .LABEL9",
        ".LABEL16:",
            "ldx.w   $t1, $a1, $a7",
            "stx.w   $t1, $t0, $a7",
        ".LABEL9:",
            "ld.d    $t0, $sp, 8",
            "slli.d  $a7, $a6, 3",
            "addi.d  $t2, $a4, 8",
            "sub.d   $t1, $a1, $a6",
            "bgeu    $t2, $a3, .LABEL10",
            "sub.d   $t2, $zero, $a7",
            "andi    $t2, $t2, 56",
        ".LABEL11:",
            "move    $t3, $t0",
            "ld.d    $t0, $t1, 8",
            "srl.d   $t3, $t3, $a7",
            "addi.d  $t1, $t1, 8",
            "sll.d   $t4, $t0, $t2",
            "or      $t3, $t4, $t3",
            "st.d    $t3, $a4, 0",
            "addi.d  $t3, $a4, 16",
            "addi.d  $a4, $a4, 8",
            "bltu    $t3, $a3, .LABEL11",
        ".LABEL10:",
            "move    $t2, $zero",
            "addi.d  $t1, $t1, 8",
            "ori     $t3, $zero, 4",
            "st.d    $zero, $sp, 8",
            "bgeu    $a6, $t3, .LABEL12",
            "andi    $a6, $a1, 2",
            "bnez    $a6, .LABEL13",
        ".LABEL17:",
            "andi    $a6, $a1, 1",
            "beqz    $a6, .LABEL14",
        ".LABEL18:",
            "ldx.b   $a6, $t1, $t2",
            "addi.d  $t1, $sp, 8",
            "or      $t1, $t1, $t2",
            "st.b    $a6, $t1, 0",
        ".LABEL14:",
            "ld.d    $a6, $sp, 8",
            "srl.d   $t0, $t0, $a7",
            "sub.d   $a7, $zero, $a7",
            "andi    $a7, $a7, 56",
            "sll.d   $a6, $a6, $a7",
            "or      $a6, $a6, $t0",
            "st.d    $a6, $a4, 0",
            "b       .LABEL4",
        ".LABEL8:",
            "ld.b    $a7, $a1, 0",
            "st.b    $a7, $t0, 0",
            "ori     $a7, $zero, 1",
            "andi    $t2, $t1, 2",
            "beqz    $t2, .LABEL15",
        ".LABEL19:",
            "ldx.h   $t2, $a1, $a7",
            "stx.h   $t2, $t0, $a7",
            "addi.d  $a7, $a7, 2",
            "andi    $t1, $t1, 4",
            "bnez    $t1, .LABEL16",
            "b       .LABEL9",
        ".LABEL12:",
            "ld.w    $a6, $t1, 0",
            "st.w    $a6, $sp, 8",
            "ori     $t2, $zero, 4",
            "andi    $a6, $a1, 2",
            "beqz    $a6, .LABEL17",
        ".LABEL13:",
            "ldx.h   $a6, $t1, $t2",
            "addi.d  $t3, $sp, 8",
            "or      $t3, $t3, $t2",
            "st.h    $a6, $t3, 0",
            "addi.d  $t2, $t2, 2",
            "andi    $a6, $a1, 1",
            "bnez    $a6, .LABEL18",
            "b       .LABEL14",
            // ^^^ 以上代码衍生自 Rust 工具链自带的 compiler_builtins::mem::memcpy 汇编，有略微改写 ^^^
        ".globl  __copy_uaccess_fault",
        "__copy_uaccess_fault:",
            "addi.d  $sp, $sp, 16", // 改 sp 这一步是不会失败的（除非取指出问题，那就是内核 bug 了），因此失败了我们也要把 sp 改回去
            "li.d    $a0, 1",
            "ret",
        ),
    }
}
