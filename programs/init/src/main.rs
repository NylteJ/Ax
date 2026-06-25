#![no_std]
#![no_main]
//
#![feature(integer_sign_cast)]
#![feature(concat_bytes)]

use core::ffi::c_void;
use core::fmt::Write;

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

fn main() -> isize {
    let stdin = openat(-100, b"/dev/tty\0".as_ptr(), 0b00, 0);
    let stdout = openat(-100, b"/dev/tty\0".as_ptr(), 0b01, 0);
    let stderr = openat(-100, b"/dev/tty\0".as_ptr(), 0b01, 0);

    if stdin != 0 || stdout != 1 || stderr != 2 {
        return -1;
    }

    if fork() == 0 {
        // 子进程，运行终端

        macro_rules! group {
            ($name:literal at $dir:literal shell $($script:literal),+) => {
                concat_bytes!(
                    b"echo \"#### OS COMP TEST GROUP START ",
                    $name,
                    b" ####\"; cd /sdcd/",
                    $dir,
                    $(
                        b"; /sdcd/musl/busybox sh /sdcd/",
                        $dir,
                        b"/",
                        $script,
                    )*
                    b"; echo \"#### OS COMP TEST GROUP END ",
                    $name,
                    b" ####\";",
                )
            };
            (nobanner at $dir:literal shell $($script:literal),+) => {
                concat_bytes!(
                    b"cd /sdcd/",
                    $dir,
                    $(
                        b"; /sdcd/musl/busybox sh /sdcd/",
                        $dir,
                        b"/",
                        $script,
                    )*
                    b";"
                )
            };
        }

        let argv: [*const u8; 5] = [
            b"/sdcd/musl/busybox\0".as_ptr(),
            b"sh\0".as_ptr(),
            b"-c\0".as_ptr(),
            concat_bytes!(
                b"/sdcd/musl/busybox --install -s /bin;",
                // ===================================================
                b"/sdcd/musl/busybox sh /ltp.sh;", // 不知道为什么，把 ltp 放在最后貌似会让 LoongArch 64 没分
                // ===================================================
                group!(b"basic-musl" at b"musl/basic" shell b"run-all.sh"),
                group!(b"basic-glibc" at b"glibc/basic" shell b"run-all.sh"),
                group!(nobanner at b"musl" shell b"busybox_testcode.sh"),
                group!(nobanner at b"glibc" shell b"busybox_testcode.sh"),
                group!(nobanner at b"musl" shell b"libcbench_testcode.sh"),
                group!(nobanner at b"glibc" shell b"libcbench_testcode.sh"),
                group!(b"libctest-musl" at b"musl" shell b"run-static.sh", b"run-dynamic.sh"),
                // group!(b"libctest-glibc" at b"glibc" shell b"run-static.sh", b"run-dynamic.sh"),
                // ===================================================
                b'\0',
            )
            .as_ptr(),
            core::ptr::null(),
        ];
        let envp: [*const u8; 2] = [b"PATH=/bin\0".as_ptr(), core::ptr::null()];
        let fail_code = syscall!(221, argv[0], argv.as_ptr(), envp.as_ptr()).cast_signed();
        println!("execve 失败，返回值：{fail_code}");
        return fail_code;
    }

    let mut status: usize = 0xBADC0DE;
    loop {
        let ret = syscall!(260, -1, &mut status, 0) as isize;
        if ret > 0 {
            // println!("init 进程回收了 {ret}, status: {:#04x}", status);
            if ret == 1 {
                return 0; // FIXME: temp
            }
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

fn fork() -> isize {
    syscall!(220, 0, 0).cast_signed()
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
