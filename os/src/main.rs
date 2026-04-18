#![no_std]
#![no_main]
//
#![allow(internal_features)]
#![feature(core_intrinsics)]
#![feature(rustc_attrs)]

use core::arch::asm;
use core::arch::global_asm;
use core::arch::naked_asm;
use core::intrinsics::volatile_set_memory;

use crate::basic::sbi::shutdown;

mod basic;

const KERNEL_STACK_BYTES: usize = 4096 * 16;

global_asm!(
    ".section .bss.stack",
    ".globl boot_stack_bottom",
    "boot_stack_bottom:",
    "    .space {KERNEL_STACK_BYTES}",
    ".globl boot_stack_top",
    "boot_stack_top:",
    KERNEL_STACK_BYTES = const KERNEL_STACK_BYTES,
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

    println!("Hello, world!");
    println!("中文也可以！");
    println!();

    panic!("这是一个 panic！");

    shutdown(basic::sbi::ShutdownStatus::Normal);
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
