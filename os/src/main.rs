#![no_std]
#![no_main]
//
#![allow(internal_features)]
#![feature(core_intrinsics)]
#![feature(rustc_attrs)]
#![feature(fn_align)]
#![feature(never_type)]

extern crate alloc;

use core::arch::asm;
use core::arch::global_asm;
use core::arch::naked_asm;
use core::intrinsics::volatile_set_memory;

use crate::basic::debugger::backtrace;
use crate::basic::global_allocator::init_heap;
use crate::basic::sbi::shutdown;

mod basic;
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
    init_heap();

    let layout = core::alloc::Layout::from_size_align(256, 64).unwrap();
    for _ in 0..100 {
        let memory = unsafe { alloc::alloc::alloc(layout) };
        let addr = memory as usize;
        println!("动态分配得到了位于 {addr:#x} 的内存");
        assert!(addr % layout.align() == 0);
    }

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

    trap::trap_handler::init();

    println!("准备进入用户态...");

    unsafe { (CURRENT_USER_PROGRAM_INDEX_ADDR as *mut usize).write(0) }

    run_next_program();
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

// TODO: 这几个都是临时的（不过吐槽一句内核是真的可以为所欲为，想写哪里自己填）
const USER_STACK_ADDRESS: usize = 0x81000000;
static USER_PROGRAMS: [fn() -> !; 3] = [user_start::<0>, user_start::<1>, user_start::<2>];
static CURRENT_USER_PROGRAM_INDEX_ADDR: usize = 0x81001000;
fn run_next_program() -> ! {
    let idx = unsafe { (CURRENT_USER_PROGRAM_INDEX_ADDR as *const usize).read() };
    if idx == USER_PROGRAMS.len() {
        println!("所有用户程序已执行完毕！");
        shutdown(basic::sbi::ShutdownStatus::Normal);
    }
    println!("准备运行用户程序 {idx}...");
    unsafe { (CURRENT_USER_PROGRAM_INDEX_ADDR as *mut usize).write(idx + 1) }
    run_user_program(USER_PROGRAMS[idx] as usize, USER_STACK_ADDRESS);
}

/// 从内核态切换到用户态，执行给定代码。
///
/// 不会清理通用寄存器。
fn run_user_program(entry: usize, stack: usize) -> ! {
    cfg_select! {
        target_arch = "riscv64" => unsafe {
            use riscv::register::sstatus;

            sstatus::set_spp(sstatus::SPP::User);

            asm!(
                "csrw sscratch, {stack}",
                "csrw sepc, {entry}",
                "csrrw sp, sscratch, sp",
                "sret",
                stack = in(reg) stack,
                entry = in(reg) entry,
                options(noreturn),
            );
        },
        target_arch = "loongarch64" => unsafe {
            use bit_field::BitField;

            let mut prmd: usize;
            asm!("csrrd {0}, 0x1", out(reg) prmd);
            prmd.set_bits(0..2, 3);
            asm!("csrwr {0}, 0x1", in(reg) prmd);

            asm!(
                "csrwr {entry}, 0x6",
                "csrwr $sp, 0x30",
                "move $sp, {stack}",
                "ertn",
                entry = in(reg) entry,
                stack = in(reg) stack,
                options(noreturn),
            );
        },
    }
}

fn user_start<const RET: usize>() -> ! {
    fn user_main() {}

    user_main();

    syscall!(93, RET);
    unreachable!();
}
