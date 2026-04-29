use core::{arch::asm, ptr::NonNull};

use crate::basic::sync::Mutex;

pub mod basic;
pub mod process;

/// 执行系统调用。
///
/// 第一个参数是系统调用 ID，剩余参数将被转发给 ecall。
///
/// 允许 0 ~ 6 个参数，不在此范围内的参数数目会导致编译错误。
///
/// 返回值是 ecall 的返回值，类型为 isize。
#[macro_export]
macro_rules! syscall {
    ($nr:expr) => {{
        let ret: usize;
        unsafe {
            cfg_select! {
                target_arch = "riscv64" => core::arch::asm!(
                    "ecall",
                    in("a7") $nr,
                    lateout("a0") ret,
                ),
                target_arch = "loongarch64" => core::arch::asm!(
                    "syscall 0",
                    in("$a7") $nr,
                    lateout("$a0") ret,
                ),
            }
        }
        ret
    }};

    ($nr:expr, $a0:expr) => {{
        let ret: usize;
        unsafe {
            cfg_select! {
                target_arch = "riscv64" => core::arch::asm!(
                    "ecall",
                    in("a7") $nr,
                    in("a0") $a0,
                    lateout("a0") ret,
                ),
                target_arch = "loongarch64" => core::arch::asm!(
                    "syscall 0",
                    in("$a7") $nr,
                    in("$a0") $a0,
                    lateout("$a0") ret,
                ),
            }
        }
        ret
    }};

    ($nr:expr, $a0:expr, $a1:expr) => {{
        let ret: usize;
        unsafe {
            cfg_select! {
                target_arch = "riscv64" => core::arch::asm!(
                    "ecall",
                    in("a7") $nr,
                    in("a0") $a0,
                    in("a1") $a1,
                    lateout("a0") ret,
                ),
                target_arch = "loongarch64" => core::arch::asm!(
                    "syscall 0",
                    in("$a7") $nr,
                    in("$a0") $a0,
                    in("$a1") $a1,
                    lateout("$a0") ret,
                ),
            }
        }
        ret
    }}; // TODO
}

#[derive(Clone, Copy)]
pub struct SyscallFn {
    pub(self) ptr: NonNull<()>,
}

// SAFETY: 外部无法构造 SyscallFn，只能访问 TABLE，它是只读的，也没有内部可变性
unsafe impl Sync for SyscallFn {}

impl SyscallFn {
    pub fn call(&self, args: &[usize; 6]) -> isize {
        let func_addr = self.ptr.as_ptr() as usize;

        let ret: isize;
        cfg_select! {
            target_arch = "riscv64" => unsafe {
                asm!(
                    "jalr ra, {func}",
                    func = in(reg) func_addr,
                    in("a0") args[0],
                    in("a1") args[1],
                    in("a2") args[2],
                    in("a3") args[3],
                    in("a4") args[4],
                    in("a5") args[5],
                    lateout("a0") ret,
                    clobber_abi("C"),
                );
            },
            target_arch = "loongarch64" => unsafe {
                asm!(
                    "jirl $ra, {func}, 0",
                    func = in(reg) func_addr,
                    in("$a0") args[0],
                    in("$a1") args[1],
                    in("$a2") args[2],
                    in("$a3") args[3],
                    in("$a4") args[4],
                    in("$a5") args[5],
                    lateout("$a0") ret,
                    clobber_abi("C"),
                );
            },
        }

        ret
    }
}

pub static TABLE: [Option<SyscallFn>; 512] = const {
    use crate::syscall::*;

    let mut table: [Option<SyscallFn>; 512] = [None; 512];

    macro_rules! define {
        ($id:expr, $func:expr) => {
            table[$id] = Some(SyscallFn {
                ptr: NonNull::new($func as *mut ()).unwrap(),
            });
        };
    }

    define!(93, basic::exit);
    define!(124, basic::yield_);
    define!(173, process::getppid);
    define!(220, process::clone);
    define!(221, process::execve);
    define!(260, process::wait4);

    define!(233, basic::print);

    table
};
