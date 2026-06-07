pub mod interfaces;

mod loongarch64;
mod riscv64;

#[cfg(target_arch = "loongarch64")]
pub use loongarch64::LoongArch64 as Native;
#[cfg(target_arch = "riscv64")]
pub use riscv64::RiscV64 as Native;

pub type NativeRegs = <Native as interfaces::Arch>::Regs;
pub type NativeBasic = <Native as interfaces::Arch>::Basic;
pub type NativeChrono = <Native as interfaces::Arch>::Chrono;
pub type NativeUAccessRaw = <Native as interfaces::Arch>::UAccessRaw;
pub type NativeTrap = <Native as interfaces::Arch>::Trap;
pub type NativeTrapContext = <NativeTrap as interfaces::Trap>::TrapContext;
pub type NativeMContext = <NativeTrapContext as interfaces::TrapContext>::MContext;
