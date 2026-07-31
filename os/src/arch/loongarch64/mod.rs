#![cfg(target_arch = "loongarch64")]

use core::arch::naked_asm;

use crate::{
    arch::interfaces::Arch, basic::bstr::ByteStr, main, memory::address::LINEAR_MAPPING_OFFSET,
};

mod basic;
mod chrono;
mod regs;
mod smp;
mod trap;
mod uaccess_raw;

pub struct LoongArch64;

unsafe impl Arch for LoongArch64 {
    const INIT_EXEC_PATH: &ByteStr = ByteStr::from_bytes(b"/la-init");

    type Regs = regs::Regs;
    type Basic = basic::Basic;
    type Chrono = chrono::Chrono;
    type Smp = smp::SmpImpl;
    type UAccessRaw = uaccess_raw::UAccessRaw;
    type Trap = trap::Trap;

    #[unsafe(no_mangle)]
    #[naked]
    #[unsafe(link_section = ".text.entry")]
    extern "C" fn _start() {
        unsafe {
            naked_asm!(
                // 启用 DMW
                "li.d $t0, {LINEAR_MAPPING_OFFSET} | 0x11", // 一致可缓存，PLV 0
                "csrwr $t0, 0x180",
                "li.d $t0, 0x10", // DA = 0, PG = 1
                "csrwr $t0, 0x0",
                // 和 RISC-V 64 的情况不一样，这里 PC 初始就位于高地址，所以接下来不用动了
                // （由于 LoongArch 64 物理地址空间最高不超过 60 位，不启用 DMW 程序貌似也是能正常跑的，多余高位会直接被忽略）
                "la $sp, boot_stack_top",
                // LoongArch 64 支持设备树，但不会像 RISC-V 64 那样自动通过寄存器传参，而是始终为定值
                // 这里我们手动传给 main
                "li.d $a1, 0x100000",
                "b {main}",
                LINEAR_MAPPING_OFFSET = const LINEAR_MAPPING_OFFSET,
                main = sym main,
            )
        }
    }
}
