pub mod basic;

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

pub type Arguments<'a> = &'a [usize; 6];

pub static TABLE: [Option<fn(Arguments) -> usize>; 256] = {
    use crate::syscall::*;

    let mut table: [Option<fn(Arguments) -> usize>; 256] = [None; 256];

    table[93] = Some(|args| basic::exit(args));
    table[124] = Some(basic::yield_);
    table[233] = Some(basic::print);

    table
};
