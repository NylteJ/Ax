use core::{ffi::CStr, pin::Pin};

use crate::{
    basic::{
        sbi::{ShutdownStatus, shutdown},
        shared_with_asm::SharedWithAsm,
    },
    multitask::{current_task, task_scheduler::TASK_SCHEDULER},
    print, println, run_program,
    syscall::Arguments,
};

// /// 退出应用程序并将返回值告知批处理系统。
// ///
// /// # 参数
// /// - `exit_code`：应用程序的返回值。
// pub fn exit(exit_code: i32) -> ! {
//     syscall!(93, exit_code);
//     unreachable!();
// }

pub fn exit(args: Arguments) -> ! {
    println!(
        "[Kernel] Application exited with code {0} ({0:#x}).",
        args[0]
    );

    let cur = current_task();
    unsafe {
        Pin::drop_asm(cur);
    };

    let Some(next_task) = TASK_SCHEDULER.write().pick_next() else {
        println!("所有用户程序运行完毕，关机");
        shutdown(ShutdownStatus::Normal);
    };

    run_program(next_task);
    unreachable!();
}

pub fn yield_(_args: Arguments) -> usize {
    println!("[Kernel] Application yielded.");

    let mut scheduler = TASK_SCHEDULER.write();
    let cur = current_task();
    scheduler.add_task(unsafe { Pin::from_asm(cur) });
    run_program(scheduler.pick_next().unwrap());

    0
}

// FIXME: 临时
pub fn print(args: Arguments) -> usize {
    let str = args[0] as *const _;
    let str = unsafe { CStr::from_ptr(str) };
    print!("{}", str.to_str().unwrap());
    0
}
