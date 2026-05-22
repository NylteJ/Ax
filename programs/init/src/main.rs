#![no_std]
#![no_main]

use core::ffi::c_void;
use core::fmt::Write;
use core::mem::MaybeUninit;
use core::sync::atomic::AtomicU32;

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

    println!("Hello from init!");

    print!("请输入文本（不会回显）：");

    let mut buffer = [1u8; 1024];
    let read_bytes = read(stdin, buffer.as_mut_ptr().cast(), buffer.len());
    if read_bytes < 0 {
        return -2;
    }

    print!("\n你输入了：");
    let wbytes = write(stdout, buffer.as_ptr().cast(), read_bytes as usize);
    if wbytes != read_bytes {
        return -3;
    }

    let ret = syscall!(220, 0, 0) as isize;
    if ret < 0 {
        return -4;
    }
    println!("clone 返回值：{ret}");

    if ret == 0 {
        println!("子进程！父进程 PID 是 {}", syscall!(173));
        let ret = syscall!(220, 0, 0) as isize;
        if ret < 0 {
            return -5;
        }
        println!("我的父进程 PID 是 {}, ret = {ret}", syscall!(173));

        if ret != 0 {
            let mut v: i32 = -1919810;
            let status = syscall!(260, ret, &mut v, 0) as isize;
            if status < 0 {
                return -6;
            }
            println!("子进程 {ret} 退出了，状态码是 {:#04x}", v);
        } else {
            let ret = syscall!(220, 0, 0) as isize;
            if ret < 0 {
                return -6;
            }

            if ret == 0 {
                println!("我是子进程，我会创建很多个线程做加法");

                for i in 0..10 {
                    let ret = syscall!(220, (1 << 8) | (1 << 16), (&raw mut STACKS[0]).add(i + 1))
                        as isize;
                    if ret < 0 {
                        return -70;
                    }
                    if ret == 0 {
                        println!("来自子线程：ret 地址 {:p}", &raw const ret);
                        break;
                    } else {
                        println!("来自主线程：PID {ret}, ret 地址 {:p}", &raw const ret);
                    }
                }

                for _ in 0..10000000 {
                    SUM.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                }

                println!("sum = {}", SUM.load(core::sync::atomic::Ordering::Relaxed));
            } else {
                println!("我是父进程，我应该先死");
            }

            syscall!(93, 0x64);
        }

        syscall!(93, 100 + ret);
    }

    let mut status: i32 = -114514;
    while let rt @ ..=0 = syscall!(260, ret, &mut status, 1) as isize {
        if rt < 0 {
            return -10;
        }
        println!("WNOHANG 失败一次");
    }
    println!("子进程 {ret} 退出了，状态码是 {:#04x}", status);

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
