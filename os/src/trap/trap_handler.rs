use core::{
    arch::{asm, naked_asm},
    mem::offset_of,
    ops::RangeInclusive,
};

use crate::trap::trap::{Exception, Trap};

pub fn init() {
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("csrw stvec, {0}", in(reg) trap_handler_raw) },
        target_arch = "loongarch64" => unsafe { asm!("csrwr {0}, 0xc",  in(reg) trap_handler_raw) },
    };
}

#[repr(C)]
struct TrapContext {
    pub registers: [usize; 31], // x1 ~ x31
    pub sstatus: usize,         // OR PRMD (LoongArch64)
    pub ret_pc: usize,          // 用户态 PC，即返回后执行的第一条指令
}

#[unsafe(no_mangle)]
pub extern "C" fn trap_handler(context: &mut TrapContext) -> usize {
    match Trap::new() {
        Trap::Exception(Exception::Syscall) => {
            use crate::syscall;

            const SYSCALL_ID_REG_INDEX: usize = cfg_select! {
                target_arch = "riscv64" => 16, // a7, r17
                target_arch = "loongarch64" => 10, // $a7, $r11
            };
            const SYSCALL_ARGS_REG_INDEX_RANGE: RangeInclusive<usize> = cfg_select! {
                target_arch = "riscv64" => 9..=14, // a0 ~ a5, r10 ~ r15
                target_arch = "loongarch64" => 3..=8, // $a0 ~ $a5, $r4 ~ $r9
            };

            context.ret_pc += 4; // 跳过 `ecall` / `break 0` 指令

            let id = context.registers[SYSCALL_ID_REG_INDEX];
            let args: syscall::Arguments = &context.registers[SYSCALL_ARGS_REG_INDEX_RANGE]
                .try_into()
                .unwrap();

            match syscall::TABLE.get(id) {
                Some(Some(func)) => func(args),
                _ => panic!("不支持的系统调用 ID: {id} ({id:#x})"),
            }
        }
        other => todo!("TRAP: {other:?}"),
    }
}

#[rustc_align(4)]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[cfg(target_arch = "riscv64")]
pub extern "C" fn trap_handler_raw() {
    static_assertions::const_assert_eq!(size_of::<TrapContext>(), 33 * 8);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, registers), 0);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, sstatus), 31 * 8);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, ret_pc), 32 * 8);

    naked_asm!(
        // 1. 切换到内核栈
        "csrrw sp, sscratch, sp",
        // 2. 为 TrapContext 留出栈空间
        "addi sp, sp, -34*8", // 注意：尽管我们只需要 33*8 字节的空间，RISC-V 64 要求栈按 16 字节对齐
        // 3. 保存寄存器（x1 ~ x31，不含 x2 (sp)）
        "sd  x1,  0 *8(sp)",
        //    (sp 在后面保存，它现在存放的是内核栈的地址)
        "sd  x3,  2 *8(sp)",
        "sd  x4,  3 *8(sp)",
        "sd  x5,  4 *8(sp)",
        "sd  x6,  5 *8(sp)",
        "sd  x7,  6 *8(sp)",
        "sd  x8,  7 *8(sp)",
        "sd  x9,  8 *8(sp)",
        "sd x10,  9 *8(sp)",
        "sd x11, 10 *8(sp)",
        "sd x12, 11 *8(sp)",
        "sd x13, 12 *8(sp)",
        "sd x14, 13 *8(sp)",
        "sd x15, 14 *8(sp)",
        "sd x16, 15 *8(sp)",
        "sd x17, 16 *8(sp)",
        "sd x18, 17 *8(sp)",
        "sd x19, 18 *8(sp)",
        "sd x20, 19 *8(sp)",
        "sd x21, 20 *8(sp)",
        "sd x22, 21 *8(sp)",
        "sd x23, 22 *8(sp)",
        "sd x24, 23 *8(sp)",
        "sd x25, 24 *8(sp)",
        "sd x26, 25 *8(sp)",
        "sd x27, 26 *8(sp)",
        "sd x28, 27 *8(sp)",
        "sd x29, 28 *8(sp)",
        "sd x30, 29 *8(sp)",
        "sd x31, 30 *8(sp)",
        // 4. 保存 sstatus 和 sepc
        "csrr a0, sstatus",
        "sd a0, 31*8(sp)",
        "csrr a1, sepc",
        "sd a1, 32*8(sp)",
        // 5. 保存用户栈的 sp
        "csrr a2, sscratch",
        "sd a2, 1*8(sp)",
        // 6. 调用 trap_handler
        "mv a0, sp", // 传参
        "call trap_handler",
        // 7. 恢复用户栈的 sp（到 sscratch 中）
        "ld a2, 1*8(sp)",
        "csrw sscratch, a2",
        // 8. 恢复 sepc 和 sstatus
        "ld a1, 32*8(sp)",
        "csrw sepc, a1",
        "ld a0, 31*8(sp)",
        "csrw sstatus, a0",
        // 9. 恢复先前保存的其它寄存器
        "ld x31, 30 *8(sp)",
        "ld x30, 29 *8(sp)",
        "ld x29, 28 *8(sp)",
        "ld x28, 27 *8(sp)",
        "ld x27, 26 *8(sp)",
        "ld x26, 25 *8(sp)",
        "ld x25, 24 *8(sp)",
        "ld x24, 23 *8(sp)",
        "ld x23, 22 *8(sp)",
        "ld x22, 21 *8(sp)",
        "ld x21, 20 *8(sp)",
        "ld x20, 19 *8(sp)",
        "ld x19, 18 *8(sp)",
        "ld x18, 17 *8(sp)",
        "ld x17, 16 *8(sp)",
        "ld x16, 15 *8(sp)",
        "ld x15, 14 *8(sp)",
        "ld x14, 13 *8(sp)",
        "ld x13, 12 *8(sp)",
        "ld x12, 11 *8(sp)",
        "ld x11, 10 *8(sp)",
        "ld x10,  9 *8(sp)",
        "ld  x9,  8 *8(sp)",
        "ld  x8,  7 *8(sp)",
        "ld  x7,  6 *8(sp)",
        "ld  x6,  5 *8(sp)",
        "ld  x5,  4 *8(sp)",
        "ld  x4,  3 *8(sp)",
        "ld  x3,  2 *8(sp)",
        "ld  x1,  0 *8(sp)",
        // 10. 释放栈空间
        "addi sp, sp, 34*8",
        // 11. 切换回用户栈
        "csrrw sp, sscratch, sp",
        // 12. 返回用户态（U 特权级）
        "sret",
    );
}

#[rustc_align(4096)]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[cfg(target_arch = "loongarch64")]
pub extern "C" fn trap_handler_raw() {
    static_assertions::const_assert_eq!(size_of::<TrapContext>(), 33 * 8);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, registers), 0);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, sstatus), 31 * 8);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, ret_pc), 32 * 8);

    naked_asm!(
        // 1. 切换到内核栈
        "csrwr $sp, {CSR_SAVE0}",
        // 2. 为 TrapContext 留出栈空间
        "addi.d $sp, $sp, -34*8", // 注意：尽管我们只需要 33*8 字节的空间，LoongArch 64 要求栈按 16 字节对齐
        // 3. 保存寄存器（$r1 ~ $r31，不含 $r3 ($sp)）
        "st.d  $r1, $sp,  0 *8",
        "st.d  $r2, $sp,  1 *8",
        //    ($sp 在后面保存，它现在存放的是内核栈的地址)
        "st.d  $r4, $sp,  3 *8",
        "st.d  $r5, $sp,  4 *8",
        "st.d  $r6, $sp,  5 *8",
        "st.d  $r7, $sp,  6 *8",
        "st.d  $r8, $sp,  7 *8",
        "st.d  $r9, $sp,  8 *8",
        "st.d $r10, $sp,  9 *8",
        "st.d $r11, $sp, 10 *8",
        "st.d $r12, $sp, 11 *8",
        "st.d $r13, $sp, 12 *8",
        "st.d $r14, $sp, 13 *8",
        "st.d $r15, $sp, 14 *8",
        "st.d $r16, $sp, 15 *8",
        "st.d $r17, $sp, 16 *8",
        "st.d $r18, $sp, 17 *8",
        "st.d $r19, $sp, 18 *8",
        "st.d $r20, $sp, 19 *8",
        "st.d $r21, $sp, 20 *8",
        "st.d $r22, $sp, 21 *8",
        "st.d $r23, $sp, 22 *8",
        "st.d $r24, $sp, 23 *8",
        "st.d $r25, $sp, 24 *8",
        "st.d $r26, $sp, 25 *8",
        "st.d $r27, $sp, 26 *8",
        "st.d $r28, $sp, 27 *8",
        "st.d $r29, $sp, 28 *8",
        "st.d $r30, $sp, 29 *8",
        "st.d $r31, $sp, 30 *8",
        // 4. 保存 PRMD 和 ERA
        "csrrd $t0, {CSR_PRMD}",
        "st.d $t0, $sp, 31*8",
        "csrrd $t1, {CSR_ERA}",
        "st.d $t1, $sp, 32*8",
        // 5. 保存用户栈的 $sp
        "csrrd $t2, {CSR_SAVE0}",
        "st.d $t2, $sp, 2*8",
        // 6. 调用 trap_handler
        "move $a0, $sp", // 传参
        "bl trap_handler",
        // 7. 恢复用户栈的 $sp（到 CSR_SAVE0 中）
        "ld.d $t2, $sp, 2*8",
        "csrwr $t2, {CSR_SAVE0}",
        // 8. 恢复 sepc 和 sstatus
        "ld.d $t1, $sp, 32*8",
        "csrwr $t1, {CSR_ERA}",
        "ld.d $t0, $sp, 31*8",
        "csrwr $t0, {CSR_PRMD}",
        // 9. 恢复先前保存的其它寄存器
        "ld.d $r31, $sp, 30 *8",
        "ld.d $r30, $sp, 29 *8",
        "ld.d $r29, $sp, 28 *8",
        "ld.d $r28, $sp, 27 *8",
        "ld.d $r27, $sp, 26 *8",
        "ld.d $r26, $sp, 25 *8",
        "ld.d $r25, $sp, 24 *8",
        "ld.d $r24, $sp, 23 *8",
        "ld.d $r23, $sp, 22 *8",
        "ld.d $r22, $sp, 21 *8",
        "ld.d $r21, $sp, 20 *8",
        "ld.d $r20, $sp, 19 *8",
        "ld.d $r19, $sp, 18 *8",
        "ld.d $r18, $sp, 17 *8",
        "ld.d $r17, $sp, 16 *8",
        "ld.d $r16, $sp, 15 *8",
        "ld.d $r15, $sp, 14 *8",
        "ld.d $r14, $sp, 13 *8",
        "ld.d $r13, $sp, 12 *8",
        "ld.d $r12, $sp, 11 *8",
        "ld.d $r11, $sp, 10 *8",
        "ld.d $r10, $sp,  9 *8",
        "ld.d  $r9, $sp,  8 *8",
        "ld.d  $r8, $sp,  7 *8",
        "ld.d  $r7, $sp,  6 *8",
        "ld.d  $r6, $sp,  5 *8",
        "ld.d  $r5, $sp,  4 *8",
        "ld.d  $r4, $sp,  3 *8",
        "ld.d  $r2, $sp,  1 *8",
        "ld.d  $r1, $sp,  0 *8",
        // 10. 释放栈空间
        "addi.d $sp, $sp, 34*8",
        // 11. 切换回用户栈
        "ld.d $sp, $sp, (2-34)*8",
        // 12. 返回用户态
        "ertn",
        CSR_SAVE0 = const 0x30,
        CSR_PRMD = const 0x1,
        CSR_ERA = const 0x6,
    );
}
