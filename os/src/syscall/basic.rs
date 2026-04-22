use core::{ffi::CStr, pin::Pin};

use alloc::sync::Arc;

use crate::{
    multitask::{current_task, task_scheduler::TASK_SCHEDULER},
    print, println, run_next_program,
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
    unsafe { Arc::decrement_strong_count(cur) };

    run_next_program();
    unreachable!();
}

pub fn yield_(args: Arguments) -> usize {
    println!("[Kernel] Application yielded.");

    let cur = current_task();
    TASK_SCHEDULER
        .write()
        .add_task(unsafe { Pin::new_unchecked(Arc::from_raw(cur)) });
    run_next_program();

    0
}

// FIXME: 临时
pub fn print(args: Arguments) -> usize {
    let str = args[0] as *const _;
    let str = unsafe { CStr::from_ptr(str) };
    print!("{}", str.to_str().unwrap());
    0
}
