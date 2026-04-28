use crate::{
    basic::{
        sbi::{ShutdownStatus, shutdown},
        shared_with_asm::SharedWithAsm,
    },
    memory::uaccess::copy_from_user,
    multitask::{current_task, task::TaskArc, task_scheduler::TASK_SCHEDULER},
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
        TaskArc::drop_asm(cur);
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
    scheduler.add_task(unsafe { TaskArc::from_asm(cur) });
    let next_task = scheduler.pick_next().unwrap();
    drop(scheduler);
    run_program(next_task);

    0
}

// FIXME: 临时
pub fn print(args: Arguments) -> usize {
    let str_ptr = args[0] as *const u8;

    let mut length = args[1];

    const BUFFER_SIZE: usize = 64;

    let mut buffer = [0; BUFFER_SIZE];

    while length > BUFFER_SIZE {
        let Ok(()) = copy_from_user(&mut buffer, str_ptr) else {
            return 2;
        };

        let s = match str::from_utf8(&buffer) {
            Ok(str) => str,
            Err(err) => unsafe { str::from_utf8_unchecked(&buffer[0..err.valid_up_to()]) },
        };

        if s.is_empty() {
            return 1;
        }

        print!("{s}");

        length -= s.len();
    }

    let Ok(()) = copy_from_user(&mut buffer[0..length], str_ptr) else {
        return 2;
    };

    let Ok(s) = str::from_utf8(&buffer) else {
        return 1;
    };

    print!("{s}");

    0
}
