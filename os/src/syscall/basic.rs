use core::slice;

use crate::{
    basic::{cleanup_and_shutdown, shared_with_asm::SharedWithAsm, timer::get_cpu_tick},
    memory::uaccess::{copy_from_user, copy_one_to_user, copy_to_user},
    multitask::{
        current_task,
        task::{INIT_PID, TaskArc},
        task_scheduler::TASK_SCHEDULER,
    },
    print, println,
    syscall::SyscallResult,
};

use crate::syscall::err_codes::*;

/// * 功能：触发进程终止，无返回值；
/// * 输入：终止状态值；
/// * 返回值：无返回值；
/// ```c
/// int ec;
/// syscall(SYS_exit, ec);
/// ```
pub extern "C" fn exit(ec: isize) -> ! {
    log::trace!(
        "Task {} exited with code {1} ({1:#x}).",
        unsafe { current_task().as_ref_unchecked() }.tid,
        ec
    );

    let cur = unsafe { TaskArc::from_asm(current_task()) };

    // FIXME: temp
    #[cfg(debug_assertions)]
    if cur.tid == INIT_PID {
        let mut raw = crate::trap::trap_handler::UNKNOWN_SYSCALLS
            .lock()
            .drain()
            .collect::<alloc::vec::Vec<_>>();

        raw.sort_unstable_by_key(|(_, count)| -count.cast_signed());

        let list = raw
            .into_iter()
            .map(|(id, count)| {
                alloc::format!(
                    "{} (id = {id}): {count} 次",
                    super::SYSCALL_ID_NAME_MAP.get(id).unwrap_or(&"???")
                )
            })
            .collect::<alloc::vec::Vec<_>>();

        log::info!("所有未知系统调用：\n{list:#?}")
    }
    debug_assert!(cur.tid != INIT_PID, "init 进程不应退出");

    log::error!("TODO: 处理退出 signal");
    cur.on_exit(0, ec as u8);

    let Some(next_task) = TASK_SCHEDULER.lock().pick_next() else {
        log::info!("所有用户程序运行完毕，关机");
        cleanup_and_shutdown();
    };

    // 注意只在不关机时 drop 当前任务，因为现在使用的页表还是当前任务的页表，drop 掉了其内存就可能被重用，
    // 导致清理任务出现各种问题。实际上连目前我们所在的内核栈都有可能被重用。
    // 不关机时 drop 因为下一步就是 switch_to，它不进行内存分配，所以没问题
    drop(cur);

    next_task.switch_to();
    unreachable!();
}

pub extern "C" fn yield_() -> isize {
    // log::trace!(
    //     "Task {} yielded.",
    //     unsafe { current_task().as_ref_unchecked() }.tid
    // );

    let mut scheduler = TASK_SCHEDULER.lock();
    let cur = current_task();
    scheduler.add_task(unsafe { TaskArc::from_asm(cur) });
    let next_task = scheduler.pick_next().unwrap();
    drop(scheduler);
    next_task.switch_to();

    0
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Tms {
    tms_utime: usize,
    tms_stime: usize,
    tms_cutime: usize,
    tms_cstime: usize,
}

/// * 功能：获取进程时间；
/// * 输入：tms结构体指针，用于获取保存当前进程的运行时间数据；
/// * 返回值：成功返回已经过去的滴答数，失败返回-1;
/// ```c
/// struct tms *tms;
/// clock_t ret = syscall(SYS_times, tms);
/// ```
pub extern "C" fn times(tms: *mut Tms) -> SyscallResult {
    if !tms.is_null() {
        let cur_task = unsafe { current_task().as_ref_unchecked() };

        let tms_data = {
            let time_stats = cur_task.time_stats.lock();
            Tms {
                tms_utime: time_stats.user_time,
                tms_stime: time_stats.kernel_time,
                tms_cutime: time_stats.children_user_time,
                tms_cstime: time_stats.children_kernel_time,
            }
        };

        copy_one_to_user(tms, &tms_data)?;
    }

    SyscallResult::success(get_cpu_tick())
}
