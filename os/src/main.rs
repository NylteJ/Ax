#![no_std]
#![no_main]
//
#![allow(internal_features)]
#![feature(core_intrinsics)]
#![feature(rustc_attrs)]
#![feature(fn_align)]
#![feature(generic_const_exprs)]
#![feature(sync_unsafe_cell)]
#![feature(adt_const_params)]
#![feature(unsized_const_params)]

extern crate alloc;

use core::arch::asm;
use core::arch::global_asm;
use core::arch::naked_asm;
use core::intrinsics::volatile_set_memory;
use core::pin::Pin;

use alloc::sync::Arc;

use crate::basic::debugger::backtrace;
use crate::basic::global_allocator::init_heap;
use crate::basic::sbi::shutdown;
use crate::multitask::current_task;
use crate::multitask::task::Task;
use crate::multitask::task_context::TaskContext;
use crate::multitask::task_context::switch_context;
use crate::multitask::task_scheduler::TASK_SCHEDULER;

mod basic;
mod multitask;
mod syscall;
mod trap;

const BOOT_STACK_BYTES: usize = 8 * 1024;

global_asm!(
    ".section .bss.stack",
    ".globl boot_stack_bottom",
    "boot_stack_bottom:",
    "    .space {BOOT_STACK_BYTES}",
    ".globl boot_stack_top",
    "boot_stack_top:",
    BOOT_STACK_BYTES = const BOOT_STACK_BYTES,
);

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() {
    cfg_select! {
        target_arch = "riscv64" =>     naked_asm!("la  sp, boot_stack_top", "j main"),
        target_arch = "loongarch64" => naked_asm!("la $sp, boot_stack_top", "b main"),
    }
}

// main 必须不能返回（因为 _start 中用的是无条件跳转）
#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    clear_bss();
    trap::trap_handler::init_k();

    init_heap();

    println!("Hello, world!");
    println!("中文也可以！");
    println!();

    (|| {
        (|| {
            (|| {
                println!(
                    "下面是一条从很深的地方发出的 stacktrace（尽管 release 模式下会被优化掉）"
                );
                backtrace();
            })()
        })()
    })();

    println!("准备进入用户态...");

    seq_macro::seq!(N in 1..=3 {
        TASK_SCHEDULER.write().add_task(Task::new(
            multitask::SchedulePolicy::FIFO,
            40,
            user_start::<N, { concat!("HELLO FROM", N, "\n\0") }> as usize,
        ));
    });

    TASK_SCHEDULER.write().add_task(Task::new(
        multitask::SchedulePolicy::FIFO,
        50,
        (|| loop {}) as fn() as usize,
    ));
    TASK_SCHEDULER.write().add_task(Task::new(
        multitask::SchedulePolicy::FIFO,
        50,
        user_start::<5, "VME50\n\0"> as usize,
    ));

    let task = TASK_SCHEDULER.write().pick_next().unwrap();
    let task = unsafe { Arc::into_raw(Pin::into_inner_unchecked(task)).as_ref_unchecked() };
    let entry = task.entry.write().take().unwrap();

    // trap::set_global_interrupts_enable(true);
    trap::enable_timer_interrupt();
    basic::timer::start_timer_tick_tock();
    run_user_program(entry, task.ustack, task.kstack.get_addr());
}

fn clear_bss() {
    let sbss: *mut u8;
    let ebss: *mut u8;
    unsafe {
        asm!("la {0}, sbss", out(reg) sbss);
        asm!("la {0}, ebss", out(reg) ebss);
    }

    unsafe { volatile_set_memory(sbss, 0, ebss.offset_from_unsigned(sbss)) };
}

// FIXME: temp
fn run_next_program() {
    let Some(task) = TASK_SCHEDULER.write().pick_next() else {
        println!("所有用户程序运行完毕，关机");
        shutdown(basic::sbi::ShutdownStatus::Normal);
    };

    let task = unsafe { Arc::into_raw(Pin::into_inner_unchecked(task)).as_ref_unchecked() };

    if task as *const _ == current_task() {
        // 仍然切换到当前任务，直接返回即可。
        // 注意这不仅是优化，也是正确性所必需的：下面的 switch_context 分支无法处理这种情况。
        return;
    }

    let mut ent = task.entry.write();
    if let Some(entry) = *ent {
        *ent = None;
        drop(ent);
        cfg_select! {
            target_arch = "riscv64" => unsafe {
                use riscv::register::sstatus;
                sstatus::set_spp(sstatus::SPP::User);

                asm!(
                    "csrw stvec, {trap_handler_raw_from_user}",
                    trap_handler_raw_from_user = in(reg) trap::trap_handler::trap_handler_raw_from_user,
                );
            },
            target_arch = "loongarch64" => unsafe {
                use bit_field::BitField;

                let mut prmd: usize;
                asm!("csrrd {0}, 0x0", out(reg) prmd); // CRMD
                prmd.set_bits(0..2, 3);
                prmd.set_bit(2, true);
                asm!("csrwr {0}, 0x1", in(reg) prmd);
                
                asm!(
                    "csrwr {trap_handler_raw_from_user}, 0xc",
                    trap_handler_raw_from_user = in(reg) trap::trap_handler::trap_handler_raw_from_user,
                );
            },
        }
        save_current_and_run(entry, task.ustack, task.kstack.get_addr());
    } else {
        drop(ent);
        unsafe {
            switch_context((task.get_context_addr() as *const TaskContext).as_ref_unchecked())
        };
    }
}

/// 从内核态切换到用户态，执行给定代码。
///
/// 不会清理通用寄存器。
fn run_user_program(entry: usize, stack: usize, kstack: usize) -> ! {
    cfg_select! {
        target_arch = "riscv64" => unsafe {
            use riscv::register::sstatus;

            sstatus::set_spp(sstatus::SPP::User);

            asm!(
                "csrw stvec, {trap_handler_raw_from_user}",
                "csrw sscratch, {stack}",
                "csrw sepc, {entry}",
                "csrrw sp, sscratch, {kstack}",
                "sret",
                trap_handler_raw_from_user = in(reg) trap::trap_handler::trap_handler_raw_from_user,
                stack = in(reg) stack,
                entry = in(reg) entry,
                kstack = in(reg) kstack,
                options(noreturn),
            );
        },
        target_arch = "loongarch64" => unsafe {
            use bit_field::BitField;

            let mut prmd: usize;
            asm!("csrrd {0}, 0x0", out(reg) prmd); // CRMD
            prmd.set_bits(0..2, 3);
            prmd.set_bit(2, true);
            asm!("csrwr {0}, 0x1", in(reg) prmd);

            asm!(
                "csrwr {trap_handler_raw_from_user}, 0xc",
                "csrwr {entry}, 0x6",
                "csrwr {kstack}, 0x30",
                "move $sp, {stack}",
                "ertn",
                trap_handler_raw_from_user = in(reg) trap::trap_handler::trap_handler_raw_from_user,
                entry = in(reg) entry,
                stack = in(reg) stack,
                kstack = in(reg) kstack,
                options(noreturn),
            );
        },
    }
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
extern "C" fn save_current_and_run(entry: usize, stack: usize, kstack: usize) {
    use core::mem::offset_of;
    use seq_macro::seq;

    const TASK_CONTEXT_SIZE: usize = size_of::<TaskContext>();
    const RA_OFFSET: usize = offset_of!(TaskContext, return_address);
    const TASK_ADDR_OFFSET: usize = offset_of!(TaskContext, task_pointer);
    const SP_OFFSET: usize = offset_of!(TaskContext, stack_pointer);
    const REGS_OFFSET: usize = offset_of!(TaskContext, callee_saved_registers);

    const KSTACK_OFFSET_IN_TASK: usize = offset_of!(crate::multitask::task::Task, kstack);

    cfg_select! {
        target_arch = "riscv64" => seq!(N in 0..=11 { naked_asm!(
            // 1. 在当前内核栈上分配一块空间，用于存储当前上下文
            "addi sp, sp, -{TASK_CONTEXT_SIZE}",
            // 2. 保存当前上下文（ra、tp、sp、s0~s11）
            "sd ra, {RA_OFFSET}(sp)",
            "sd tp, {TASK_ADDR_OFFSET}(sp)",
            "sd sp, {SP_OFFSET}(sp)",
            #(
                concat!("sd s", N, ", ", N, "*8+{REGS_OFFSET}(sp)"),
            )*
            // 3. 更新当前 Task 结构体的内核栈顶指针
            "sd sp, {KSTACK_OFFSET_IN_TASK}(tp)",

            "csrw sscratch, a1",
            "csrw sepc, a0",
            "csrrw sp, sscratch, a2",
            "sret",
            TASK_CONTEXT_SIZE = const TASK_CONTEXT_SIZE,
            RA_OFFSET = const RA_OFFSET,
            SP_OFFSET = const SP_OFFSET,
            REGS_OFFSET = const REGS_OFFSET,
            TASK_ADDR_OFFSET = const TASK_ADDR_OFFSET,
            KSTACK_OFFSET_IN_TASK = const KSTACK_OFFSET_IN_TASK,
        )}),
        target_arch = "loongarch64" => seq!(N in 0..=8 { naked_asm!(
            // 1. 在当前内核栈上分配一块空间，用于存储当前上下文
            "addi.d $sp, $sp, -{TASK_CONTEXT_SIZE}",
            // 2. 保存当前上下文（$ra、$tp、$sp、$s0~$s8）
            "st.d $ra, $sp, {RA_OFFSET}",
            "st.d $tp, $sp, {TASK_ADDR_OFFSET}",
            "st.d $sp, $sp, {SP_OFFSET}",
            #(
                concat!("st.d $s", N, ", $sp, ", N, "*8+{REGS_OFFSET}"),
            )*
            // 3. 更新当前 Task 结构体的内核栈顶指针
            "st.d $sp, $tp, {KSTACK_OFFSET_IN_TASK}",

            "csrwr $a0, 0x6",
            "csrwr $a2, 0x30",
            "move $sp, $a1",
            "ertn",
            TASK_CONTEXT_SIZE = const TASK_CONTEXT_SIZE,
            RA_OFFSET = const RA_OFFSET,
            SP_OFFSET = const SP_OFFSET,
            REGS_OFFSET = const REGS_OFFSET,
            TASK_ADDR_OFFSET = const TASK_ADDR_OFFSET,
            KSTACK_OFFSET_IN_TASK = const KSTACK_OFFSET_IN_TASK,
        )}),
    }
}

fn user_start<const RET: usize, const STR: &'static str>() -> ! {
    fn user_main<const RET: usize, const STR: &'static str>() {
        for i in 0..RET {
            syscall!(233, (STR as *const str).addr());
            // syscall!(124);
        }
    }

    user_main::<RET, STR>();

    syscall!(93, RET);
    unreachable!();
}
