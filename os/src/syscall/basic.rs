use crate::{
    basic::{
        cleanup_and_shutdown,
        sbi::{ShutdownStatus, shutdown},
        shared_with_asm::SharedWithAsm,
    },
    memory::uaccess::copy_from_user,
    multitask::{
        current_task,
        task::{INIT_PID, TaskArc},
        task_scheduler::TASK_SCHEDULER,
    },
    print, println,
};

/// * 功能：触发进程终止，无返回值；
/// * 输入：终止状态值；
/// * 返回值：无返回值；
/// ```c
/// int ec;
/// syscall(SYS_exit, ec);
/// ```
pub extern "C" fn exit(ec: isize) -> ! {
    println!("[Kernel] Application exited with code {0} ({0:#x}).", ec);

    let cur = current_task();

    debug_assert!(
        unsafe { cur.as_ref_unchecked() }.pid != INIT_PID,
        "init 进程不应退出"
    );

    let Some(next_task) = TASK_SCHEDULER.lock().pick_next() else {
        println!("所有用户程序运行完毕，关机");
        cleanup_and_shutdown();
    };

    // 注意只在不关机时 drop 当前任务，因为现在使用的页表还是当前任务的页表，drop 掉了其内存就可能被重用，
    // 导致清理任务出现各种问题。实际上连目前我们所在的内核栈都有可能被重用。
    // 不关机时 drop 因为下一步就是 switch_to，它不进行内存分配，所以没问题
    unsafe { TaskArc::drop_asm(cur) };

    next_task.switch_to();
    unreachable!();
}

pub extern "C" fn yield_() -> isize {
    println!("[Kernel] Application yielded.");

    let mut scheduler = TASK_SCHEDULER.lock();
    let cur = current_task();
    scheduler.add_task(unsafe { TaskArc::from_asm(cur) });
    let next_task = scheduler.pick_next().unwrap();
    drop(scheduler);
    next_task.switch_to();

    0
}
