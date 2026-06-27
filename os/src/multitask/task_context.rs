use core::arch::naked_asm;
use core::mem::offset_of;

use seq_macro::seq;

#[repr(C)]
#[repr(align(16))] // 上下文须压栈，而 RISC-V 64 和 LoongArch 64 都要求栈按 16 字节对齐
#[derive(Debug)]
pub struct TaskContext {
    pub return_address: usize,
    pub task_pointer: usize, // Task 结构体的地址
    pub stack_pointer: usize,
    pub page_table_register: usize,

    #[cfg(target_arch = "riscv64")]
    pub callee_saved_registers: [usize; 12], // s0 ~ s11
    #[cfg(target_arch = "loongarch64")]
    pub callee_saved_registers: [usize; 10], // $s0 ~ $s9

    #[cfg(target_arch = "loongarch64")]
    pub fregs: [usize; 32],
    #[cfg(target_arch = "loongarch64")]
    pub fcsr: u32,
    #[cfg(target_arch = "loongarch64")]
    pub fcc: u8,
    #[cfg(target_arch = "loongarch64")]
    pub euen: usize,
}

const TASK_CONTEXT_SIZE: usize = size_of::<TaskContext>();
const RA_OFFSET: usize = offset_of!(TaskContext, return_address);
const TASK_ADDR_OFFSET: usize = offset_of!(TaskContext, task_pointer);
const SP_OFFSET: usize = offset_of!(TaskContext, stack_pointer);
const PAGE_TABLE_OFFSET: usize = offset_of!(TaskContext, page_table_register);
const REGS_OFFSET: usize = offset_of!(TaskContext, callee_saved_registers);

const KSTACK_OFFSET_IN_TASK: usize = offset_of!(crate::multitask::task::Task, kstack);

/// 保存当前上下文，并切换到另一个上下文。
///
/// 注意此函数可能不会马上返回，请在调用前尽可能释放不必要的资源。
///
/// # SAFETY
/// 1. 另一个上下文必须已经使用 `switch_context` 保存并挂起且仍未恢复。\
///    特别地，这意味着 `to` **不能**指向自己。
/// 2. `tp` / `$tp` 寄存器指向当前上下文的 `Task` 结构体（通常内核态下均能满足）
#[naked]
pub unsafe extern "C" fn switch_context(to: &TaskContext) {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        seq!(N in 0..=11 {
            naked_asm!(
                // 1. 在当前内核栈上分配一块空间，用于存储当前上下文
                "addi sp, sp, -{TASK_CONTEXT_SIZE}",
                // 2. 保存当前上下文（ra、tp、sp、satp、s0~s11）
                "sd ra, {RA_OFFSET}(sp)",
                "sd tp, {TASK_ADDR_OFFSET}(sp)",
                "sd sp, {SP_OFFSET}(sp)",
                "csrr t0, satp",
                "sd t0, {PAGE_TABLE_OFFSET}(sp)",
                #(
                    concat!("sd s", N, ", ", N, "*8+{REGS_OFFSET}(sp)"),
                )*
                // 3. 更新当前 Task 结构体的内核栈顶指针
                "sd sp, {KSTACK_OFFSET_IN_TASK}(tp)",
                // 4. 恢复目标上下文（ra、tp、sp、satp、s0~s11）
                "ld ra, {RA_OFFSET}(a0)",
                "ld tp, {TASK_ADDR_OFFSET}(a0)",
                "ld sp, {SP_OFFSET}(a0)",
                "ld t0, {PAGE_TABLE_OFFSET}(a0)",
                "csrw satp, t0",
                "sfence.vma", // 切换页表后需要刷新 TLB
                #(
                    concat!("ld s", N, ", ", N, "*8+{REGS_OFFSET}(a0)"),
                )*
                // 5. 释放先前由此函数分配的内核栈空间
                //    注意此时已经处在目标函数的栈上了
                "addi sp, sp, {TASK_CONTEXT_SIZE}",
                // 6. 返回到目标上下文
                "ret",
                TASK_CONTEXT_SIZE = const TASK_CONTEXT_SIZE,
                RA_OFFSET = const RA_OFFSET,
                SP_OFFSET = const SP_OFFSET,
                PAGE_TABLE_OFFSET = const PAGE_TABLE_OFFSET,
                REGS_OFFSET = const REGS_OFFSET,
                TASK_ADDR_OFFSET = const TASK_ADDR_OFFSET,
                KSTACK_OFFSET_IN_TASK = const KSTACK_OFFSET_IN_TASK,
            )}
        );
    }
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        seq!(N in 0..=9 {
            naked_asm!(
                // 1. 在当前内核栈上分配一块空间，用于存储当前上下文
                "addi.d $sp, $sp, -{TASK_CONTEXT_SIZE}",
                // 2. 保存当前常规上下文（$ra、$tp、$sp、PGDL、EUEN、$s0~$s9）
                "st.d $ra, $sp, {RA_OFFSET}",
                "st.d $tp, $sp, {TASK_ADDR_OFFSET}",
                "st.d $sp, $sp, {SP_OFFSET}",
                "csrrd $t0, 0x19",
                "st.d $t0, $sp, {PAGE_TABLE_OFFSET}",
                "csrrd $t1, 0x2",
                "st.d $t1, $sp, {EUEN_OFFSET}",
                #(
                    concat!("st.d $s", N, ", $sp, ", N, "*8+{REGS_OFFSET}"),
                )*
                // 3. 如果 EUEN 中的浮点位为 1，则保存浮点上下文（$f0~$f31、FCSR、FCC）
                // 为提高热路径性能，这部分被提取到了其它地方
                "andi $t1, $t1, 1",
                "beqz $t1, 0f",
                "move $a1, $a0",
                "move $a0, $sp",
                "bl {store_fregs_to}", // 我们稍后会使用新 Task 的 ra 覆盖 ra，所以不用保存
                "move $a0, $a1",
                "0:",
                // 4. 更新当前 Task 结构体的内核栈顶指针
                "st.d $sp, $tp, {KSTACK_OFFSET_IN_TASK}",
                // 5. 恢复除 $ra 外的常规目标上下文（$tp、$sp、PGDL、EUEN、$s0~$s9）
                "ld.d $tp, $a0, {TASK_ADDR_OFFSET}",
                "ld.d $sp, $a0, {SP_OFFSET}",
                "ld.d $t0, $a0, {PAGE_TABLE_OFFSET}",
                "csrwr $t0, 0x19",
                "invtlb 0x0, $zero, $zero", // 切换页表后需要刷新 TLB
                "ld.d $t0, $a0, {EUEN_OFFSET}",
                "move $t1, $t0", // 将原 EUEN 保存到 $t1 中，因为 csrwr 会覆盖 $t0
                "csrwr $t0, 0x2", // 此外，注意必须先写入 EUEN 再恢复寄存器，否则可能触发浮点不可用异常
                #(
                    concat!("ld.d $s", N, ", $a0, ", N, "*8+{REGS_OFFSET}"),
                )*
                // 6. 如果 EUEN 中的浮点位为 1，则恢复浮点上下文（$f0~$f31、FCSR、FCC）
                // 为提高热路径性能，这部分被提取到了其它地方
                "andi $t1, $t1, 1",
                "beqz $t1, 1f",
                "bl {load_fregs}", // 同上，不用保存
                "1:",
                // 7. 恢复 $ra
                "ld.d $ra, $a0, {RA_OFFSET}",
                // 8. 释放先前由此函数分配的内核栈空间
                //    注意此时已经处在目标函数的栈上了
                "addi.d $sp, $sp, {TASK_CONTEXT_SIZE}",
                // 9. 返回到目标上下文
                "ret",
                TASK_CONTEXT_SIZE = const TASK_CONTEXT_SIZE,
                RA_OFFSET = const RA_OFFSET,
                SP_OFFSET = const SP_OFFSET,
                PAGE_TABLE_OFFSET = const PAGE_TABLE_OFFSET,
                EUEN_OFFSET = const offset_of!(TaskContext, euen),
                REGS_OFFSET = const REGS_OFFSET,
                TASK_ADDR_OFFSET = const TASK_ADDR_OFFSET,
                KSTACK_OFFSET_IN_TASK = const KSTACK_OFFSET_IN_TASK,
                store_fregs_to = sym store_fregs_to,
                load_fregs = sym load_fregs,
            )}
        );
    }
}

#[cfg(target_arch = "loongarch64")]
const FREGS_OFFSET: usize = offset_of!(TaskContext, fregs);
#[cfg(target_arch = "loongarch64")]
const FCSR_OFFSET: usize = offset_of!(TaskContext, fcsr);
#[cfg(target_arch = "loongarch64")]
const FCC_OFFSET: usize = offset_of!(TaskContext, fcc);

#[cfg(target_arch = "loongarch64")]
#[naked]
pub unsafe extern "C" fn store_fregs_to(context: *mut TaskContext) {
    unsafe {
        seq!(N in 0..=31 {
            naked_asm!(
                // 额外注意这里不是真正的函数调用，不能随意使用调用者保存寄存器
                // $f0 ~ $f31
                #(
                    concat!("fst.d $f", N, ", $a0, ", N, "*8+{FREGS_OFFSET}"),
                )*
                // FCSR
                "movfcsr2gr $t2, $fcsr0",
                "st.w $t2, $a0, {FCSR_OFFSET}",
                // FCC
                "movcf2gr $t3, $fcc7",
                "slli.w $t3, $t3, 1",
                "movcf2gr $t3, $fcc6",
                "slli.w $t3, $t3, 1",
                "movcf2gr $t3, $fcc5",
                "slli.w $t3, $t3, 1",
                "movcf2gr $t3, $fcc4",
                "slli.w $t3, $t3, 1",
                "movcf2gr $t3, $fcc3",
                "slli.w $t3, $t3, 1",
                "movcf2gr $t3, $fcc2",
                "slli.w $t3, $t3, 1",
                "movcf2gr $t3, $fcc1",
                "slli.w $t3, $t3, 1",
                "movcf2gr $t3, $fcc0",
                "st.b $t3, $a0, {FCC_OFFSET}",
                // 返回
                "ret",
                FREGS_OFFSET = const FREGS_OFFSET,
                FCSR_OFFSET = const FCSR_OFFSET,
                FCC_OFFSET = const FCC_OFFSET,
            )}
        );
    }
}

#[cfg(target_arch = "loongarch64")]
#[naked]
extern "C" fn load_fregs() {
    unsafe {
        seq!(N in 0..=31 {
            naked_asm!(
                // 额外注意这里不是真正的函数调用，不能随意使用调用者保存寄存器
                // $f0 ~ $f31
                #(
                    concat!("fld.d $f", N, ", $a0, ", N, "*8+{FREGS_OFFSET}"),
                )*
                // FCSR
                "ld.w $t2, $a0, {FCSR_OFFSET}",
                "movgr2fcsr $fcsr0, $t2",
                // FCC
                "ld.b $t3, $a0, {FCC_OFFSET}",
                "movgr2cf $fcc0, $t3",
                "srli.w $t3, $t3, 1",
                "movgr2cf $fcc1, $t3",
                "srli.w $t3, $t3, 1",
                "movgr2cf $fcc2, $t3",
                "srli.w $t3, $t3, 1",
                "movgr2cf $fcc3, $t3",
                "srli.w $t3, $t3, 1",
                "movgr2cf $fcc4, $t3",
                "srli.w $t3, $t3, 1",
                "movgr2cf $fcc5, $t3",
                "srli.w $t3, $t3, 1",
                "movgr2cf $fcc6, $t3",
                "srli.w $t3, $t3, 1",
                "movgr2cf $fcc7, $t3",
                // 返回
                "ret",
                FREGS_OFFSET = const FREGS_OFFSET,
                FCSR_OFFSET = const FCSR_OFFSET,
                FCC_OFFSET = const FCC_OFFSET,
            )}
        );
    }
}
