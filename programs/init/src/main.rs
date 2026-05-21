#![no_std]
#![no_main]

use core::ffi::c_void;
use core::fmt::Write;

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
    }};

    ($nr:expr, $a0:expr, $a1:expr, $a2:expr) => {{
        let ret: usize;
        unsafe {
            cfg_select! {
                target_arch = "riscv64" => core::arch::asm!(
                    "ecall",
                    in("a7") $nr,
                    in("a0") $a0,
                    in("a1") $a1,
                    in("a2") $a2,
                    lateout("a0") ret,
                ),
                target_arch = "loongarch64" => core::arch::asm!(
                    "syscall 0",
                    in("$a7") $nr,
                    in("$a0") $a0,
                    in("$a1") $a1,
                    in("$a2") $a2,
                    lateout("$a0") ret,
                ),
            }
        }
        ret
    }};

    ($nr:expr, $a0:expr, $a1:expr, $a2:expr, $a3:expr) => {{
        let ret: usize;
        unsafe {
            cfg_select! {
                target_arch = "riscv64" => core::arch::asm!(
                    "ecall",
                    in("a7") $nr,
                    in("a0") $a0,
                    in("a1") $a1,
                    in("a2") $a2,
                    in("a3") $a3,
                    lateout("a0") ret,
                ),
                target_arch = "loongarch64" => core::arch::asm!(
                    "syscall 0",
                    in("$a7") $nr,
                    in("$a0") $a0,
                    in("$a1") $a1,
                    in("$a2") $a2,
                    in("$a3") $a3,
                    lateout("$a0") ret,
                ),
            }
        }
        ret
    }};
}

#[unsafe(no_mangle)]
fn _start() -> ! {
    syscall!(93, main());
    unreachable!();
}

fn main() -> isize {
    let stdin = openat(-100, b"/dev/tty\0".as_ptr(), 0b00, 0);
    let stdout = openat(-100, b"/dev/tty\0".as_ptr(), 0b01, 0);

    if stdin != 0 || stdout != 1 {
        return -1;
    }

    writeln!(ConsoleWriter, "Hello from init!").unwrap();

    write!(ConsoleWriter, "请输入文本（不会回显）：").unwrap();

    let mut buffer = [1u8; 1024];
    let read_bytes = read(stdin, buffer.as_mut_ptr().cast(), buffer.len());
    if read_bytes < 0 {
        return -2;
    }

    write!(ConsoleWriter, "\n你输入了：").unwrap();
    let wbytes = write(stdout, buffer.as_ptr().cast(), read_bytes as usize);
    if wbytes != read_bytes {
        return -3;
    }

    // loop {}
    0
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
