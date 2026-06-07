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
    pub callee_saved_registers: [usize; 9], // $s0 ~ $s8
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
        seq!(N in 0..=8 {
            naked_asm!(
                // 1. 在当前内核栈上分配一块空间，用于存储当前上下文
                "addi.d $sp, $sp, -{TASK_CONTEXT_SIZE}",
                // 2. 保存当前上下文（$ra、$tp、$sp、PGDL、$s0~$s8）
                "st.d $ra, $sp, {RA_OFFSET}",
                "st.d $tp, $sp, {TASK_ADDR_OFFSET}",
                "st.d $sp, $sp, {SP_OFFSET}",
                "csrrd $t0, 0x19",
                "st.d $t0, $sp, {PAGE_TABLE_OFFSET}",
                #(
                    concat!("st.d $s", N, ", $sp, ", N, "*8+{REGS_OFFSET}"),
                )*
                // 3. 更新当前 Task 结构体的内核栈顶指针
                "st.d $sp, $tp, {KSTACK_OFFSET_IN_TASK}",
                // 4. 恢复目标上下文（$ra、$tp、$sp、PGDL、$s0~$s8）
                "ld.d $ra, $a0, {RA_OFFSET}",
                "ld.d $tp, $a0, {TASK_ADDR_OFFSET}",
                "ld.d $sp, $a0, {SP_OFFSET}",
                "ld.d $t0, $a0, {PAGE_TABLE_OFFSET}",
                "csrwr $t0, 0x19",
                "invtlb 0x0, $zero, $zero", // 切换页表后需要刷新 TLB
                #(
                    concat!("ld.d $s", N, ", $a0, ", N, "*8+{REGS_OFFSET}"),
                )*
                // 5. 释放先前由此函数分配的内核栈空间
                //    注意此时已经处在目标函数的栈上了
                "addi.d $sp, $sp, {TASK_CONTEXT_SIZE}",
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
}
