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
                // QEMU 的 LoongArch 虚拟机上所有核会同时从入口地址启动，且 CSR_TID 无法可靠区分核，
                // 因此通过原子加竞争槽位：拿到槽位 0 的核为启动核，其余核按到达顺序获得 1、2、3...
                "la $t0, {boot_claim}",
                "li.d $t1, 1",
                "amadd.w $t2, $t1, $t0", // $t2 = 旧值（槽位），内存值 += 1
                "bnez $t2, 1f",
                // 启动核（槽位 0）
                "la $sp, boot_stack_top",
                // LoongArch 64 支持设备树，但不会像 RISC-V 64 那样自动通过寄存器传参，而是始终为定值
                // 这里我们手动传给 main
                "li.d $a1, 0x100000",
                "b {main}",
                // 次要核：使用自己的启动栈（第 (槽位+1) 号栈）
                "1:",
                "la $sp, boot_stacks_base",
                "li.d $t0, {BOOT_STACK_BYTES}",
                "mul.d $t0, $t0, $t2",
                "add.d $sp, $sp, $t0",
                "add.d $sp, $sp, $t0",
                // 等待启动核完成初始化并释放我们
                "2:",
                "la $t0, {boot_flag}",
                "ld.b $t3, $t0, 0",
                "beqz $t3, 2b",
                "dbar 0",
                // 进入次要核入口（$a0 = 核 ID）
                "move $a0, $t2",
                "b {secondary_entry}",
                LINEAR_MAPPING_OFFSET = const LINEAR_MAPPING_OFFSET,
                BOOT_STACK_BYTES = const crate::BOOT_STACK_BYTES,
                boot_claim = sym crate::basic::per_cpu::BOOT_CLAIM,
                boot_flag = sym crate::basic::per_cpu::SECONDARY_BOOT_FLAG,
                main = sym main,
                secondary_entry = sym crate::basic::per_cpu::secondary_entry,
            )
        }
    }
}
