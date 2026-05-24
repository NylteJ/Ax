#![no_std]
#![no_main]
//
#![feature(integer_sign_cast)]

use core::ffi::c_void;
use core::fmt::Write;
use core::mem::MaybeUninit;
use core::sync::atomic::AtomicU32;

#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
compile_error!("Unsupported architecture");

macro_rules! syscall {
    ($nr:expr) => {{
        let ret: usize;
        unsafe {
            #[cfg(target_arch = "riscv64")]
            core::arch::asm!(
                "ecall",
                in("a7") $nr,
                lateout("a0") ret,
                options(nostack, nomem, preserves_flags),
            );
            #[cfg(target_arch = "loongarch64")]
            core::arch::asm!(
                "syscall 0",
                in("$a7") $nr,
                lateout("$a0") ret,
                options(nostack, nomem, preserves_flags),
            );
        }
        ret
    }};

    ($nr:expr, $a0:expr) => {{
        let ret: usize;
        unsafe {
            #[cfg(target_arch = "riscv64")]
            core::arch::asm!(
                "ecall",
                in("a7") $nr,
                in("a0") $a0,
                lateout("a0") ret,
            );
            #[cfg(target_arch = "loongarch64")]
            core::arch::asm!(
                "syscall 0",
                in("$a7") $nr,
                in("$a0") $a0,
                lateout("$a0") ret,
            );
        }
        ret
    }};

    ($nr:expr, $a0:expr, $a1:expr) => {{
        let ret: usize;
        unsafe {
            #[cfg(target_arch = "riscv64")]
            core::arch::asm!(
                "ecall",
                in("a7") $nr,
                in("a0") $a0,
                in("a1") $a1,
                lateout("a0") ret,
            );
            #[cfg(target_arch = "loongarch64")]
            core::arch::asm!(
                "syscall 0",
                in("$a7") $nr,
                in("$a0") $a0,
                in("$a1") $a1,
                lateout("$a0") ret,
            );
        }
        ret
    }};

    ($nr:expr, $a0:expr, $a1:expr, $a2:expr) => {{
        let ret: usize;
        unsafe {
            #[cfg(target_arch = "riscv64")]
            core::arch::asm!(
                "ecall",
                in("a7") $nr,
                in("a0") $a0,
                in("a1") $a1,
                in("a2") $a2,
                lateout("a0") ret,
            );
            #[cfg(target_arch = "loongarch64")]
            core::arch::asm!(
                "syscall 0",
                in("$a7") $nr,
                in("$a0") $a0,
                in("$a1") $a1,
                in("$a2") $a2,
                lateout("$a0") ret,
            );
        }
        ret
    }};

    ($nr:expr, $a0:expr, $a1:expr, $a2:expr, $a3:expr) => {{
        let ret: usize;
        unsafe {
            #[cfg(target_arch = "riscv64")]
            core::arch::asm!(
                "ecall",
                in("a7") $nr,
                in("a0") $a0,
                in("a1") $a1,
                in("a2") $a2,
                in("a3") $a3,
                lateout("a0") ret,
            );
            #[cfg(target_arch = "loongarch64")]
            core::arch::asm!(
                "syscall 0",
                in("$a7") $nr,
                in("$a0") $a0,
                in("$a1") $a1,
                in("$a2") $a2,
                in("$a3") $a3,
                lateout("$a0") ret,
            );
        }
        ret
    }};
}

#[macro_export]
macro_rules! print {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        write!(ConsoleWriter, $fmt $(, $($arg)+)?).unwrap();
    };
}

#[macro_export]
macro_rules! println {
    () => {
        writeln!(ConsoleWriter).unwrap();
    };
    ($fmt: literal $(, $($arg: tt)+)?) => {
        writeln!(ConsoleWriter, $fmt $(, $($arg)+)?).unwrap();
    };
}

#[unsafe(no_mangle)]
fn _start() -> ! {
    syscall!(93, main());
    unreachable!();
}

#[repr(align(16))]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct Stack([MaybeUninit<u8>; 4096]);

static mut STACKS: [Stack; 10] = [Stack([MaybeUninit::uninit(); 4096]); 10];
static SUM: AtomicU32 = AtomicU32::new(0);

fn main() -> isize {
    let stdin = openat(-100, b"/dev/tty\0".as_ptr(), 0b00, 0);
    let stdout = openat(-100, b"/dev/tty\0".as_ptr(), 0b01, 0);

    if stdin != 0 || stdout != 1 {
        return -1;
    }

    if fork() == 0 {
        // 子进程，运行终端
        let fail_code = execve(b"/sdcard/musl/busybox\0");
        println!("execve 失败，返回值：{fail_code}");
        return fail_code;
    }

    let mut status: usize = 0xBADC0DE;
    loop {
        let ret = syscall!(260, -1, &mut status, 0) as isize;
        if ret > 0 {
            println!("init 进程回收了 {ret}, status: {:#04x}", status);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscall!(93, -233);

    loop {}
}

fn openat(fd: isize, filename: *const u8, flags: isize, mode: usize) -> isize {
    syscall!(56, fd, filename, flags, mode).cast_signed()
}

fn write(fd: isize, buf: *const c_void, count: usize) -> isize {
    syscall!(64, fd, buf, count).cast_signed()
}

fn read(fd: isize, buf: *mut c_void, count: usize) -> isize {
    syscall!(63, fd, buf, count).cast_signed()
}

fn fork() -> isize {
    syscall!(220, 0, 0).cast_signed()
}

fn execve(path: &[u8]) -> isize {
    let argv: [*const u8; 2] = [path.as_ptr(), core::ptr::null()];
    let envp: [*const u8; 1] = [core::ptr::null()];
    syscall!(221, path.as_ptr(), argv.as_ptr(), envp.as_ptr()).cast_signed()
}

struct ConsoleWriter;

impl Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let ret = write(1, s.as_ptr().cast(), s.len());
        if ret != s.len() as isize {
            return Err(core::fmt::Error);
        }
        Ok(())
    }
}
