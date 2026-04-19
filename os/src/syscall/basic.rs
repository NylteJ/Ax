use crate::{println, run_next_program, syscall::Arguments};

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
    run_next_program();
}
