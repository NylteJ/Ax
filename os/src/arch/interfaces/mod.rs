use core::bstr::ByteStr;

mod basic;
mod chrono;
mod regs;
mod trap;
mod uaccess_raw;

pub use basic::Basic;
pub use chrono::Chrono;
pub use regs::Regs;
pub use trap::{Trap, TrapContext};
pub use uaccess_raw::UAccessRaw;

pub unsafe trait Arch {
    /// init 进程对应可执行文件的绝对路径
    const INIT_EXEC_PATH: &ByteStr;

    type Regs: Regs;
    type Basic: Basic;
    type Chrono: Chrono;
    type UAccessRaw: UAccessRaw;
    type Trap: Trap;

    /// 具体架构相关的入口函数，其负责设置栈指针、建立某种线性内存映射、让内核运行在这块映射上，
    /// 填充 main 函数的参数，并跳转执行 main。
    extern "C" fn _start();
}
