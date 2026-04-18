use core::arch::asm;

pub fn debugbreak() {
    cfg_select! {
        target_arch = "riscv64" => unsafe { asm!("ebreak") },
        target_arch = "loongarch64" => unsafe { asm!("break 0") },
    };
}
