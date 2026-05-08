#![no_std]
#![no_main]
//
#![allow(internal_features)]
#![allow(incomplete_features)]
#![feature(core_intrinsics)]
#![feature(rustc_attrs)]
#![feature(fn_align)]
#![feature(generic_const_exprs)]
#![feature(sync_unsafe_cell)]
#![feature(adt_const_params)]
#![feature(unsized_const_params)]
#![feature(exact_div)]
#![feature(never_type)]
#![feature(generic_atomic)]
#![feature(ptr_metadata)]
#![feature(bstr)]
#![feature(unsize)]
#![feature(coerce_unsized)]
#![feature(deref_pure_trait)]
#![feature(debug_closure_helpers)]
#![feature(try_trait_v2)]
#![feature(deque_extend_front)]
#![feature(slice_split_once)]
#![feature(unique_rc_arc)]
//
#![deny(unused_must_use)]

extern crate alloc;

use core::arch::asm;
use core::arch::global_asm;
use core::arch::naked_asm;
use core::bstr::ByteStr;
use core::intrinsics::volatile_set_memory;

use crate::basic::debugger::backtrace;
use crate::basic::global_allocator::init_heap;
use crate::basic::logger;
use crate::basic::shared_with_asm::SharedWithAsm;
use crate::filesystem::vfs;
use crate::filesystem::vfs::Path;
use crate::filesystem::vfs::ROOT_PATH;
use crate::filesystem::vfs::walk_path;
use crate::memory::address::LINEAR_MAPPING_OFFSET;
use crate::memory::frame_allocator;
use crate::multitask::current_task;
use crate::multitask::task::Task;
use crate::multitask::task::TaskArc;
use crate::multitask::task_context::TaskContext;
use crate::multitask::task_context::switch_context;
use crate::multitask::task_scheduler::TASK_SCHEDULER;

mod basic;
mod filesystem;
mod memory;
mod multitask;
mod syscall;
mod trap;

const BOOT_STACK_BYTES: usize = 128 * 1024; // 128KB，设小了会带来不幸（欸我操这 page table 怎么这么大.jpg）

global_asm!(
    ".section .bss.stack",
    ".globl boot_stack_bottom",
    "boot_stack_bottom:",
    "    .space {BOOT_STACK_BYTES}",
    ".globl boot_stack_top",
    "boot_stack_top:",
    BOOT_STACK_BYTES = const BOOT_STACK_BYTES,
);

#[cfg(target_arch = "riscv64")]
#[repr(align(4096))]
#[allow(dead_code)] // 尽管 Rust 侧不会读取其成员，汇编侧会访问它，因此并非“死代码”
struct BootPageTable([usize; 512]);

#[cfg(target_arch = "riscv64")]
static BOOT_PAGE_TABLE: BootPageTable = const {
    use crate::memory::address::PhysPageNum;

    use crate::memory::page::PTEFlagsNative;

    let mut entries = [0; 512];

    const VX: usize = PTEFlagsNative::Valid.bits() | PTEFlagsNative::Execute.bits();
    const VRWX: usize = VX | PTEFlagsNative::Read.bits() | PTEFlagsNative::Write.bits();

    // 将低地址的 0x80000000~... 和高地址的都映射到同一个位置
    // 这是因为 Rust 编译器认为自己位于高地址，因此各种全局变量都会从高地址拿数据
    // 但程序实际跑在低地址，特别是刚启动时，直接使用的就是物理地址。为了 pc 还能用，必须暂时映射低地址。
    entries[2] = PhysPageNum::<2>(2).mask() | VX;
    entries[256 + 2] = PhysPageNum::<2>(2).mask() | VRWX;

    BootPageTable(entries)
};

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() {
    cfg_select! {
        target_arch = "riscv64" => naked_asm!(
            // 此时，尽管程序认为自己位于高 256 GB 处，实际它还是被加载到了正常入口点位置，因此必须先启用虚拟内存
            // === 注意 ===
            // 这里按理来说是要手动偏移的（因为 BOOT_PAGE_TABLE 位于一个很高的虚拟地址），但是 Rust 编译器会在所
            // 有优化等级下将其编译为依据 pc 间接寻址，而 Rust 编译器认为的 pc 恰好也是虚拟地址，二者的相对偏移量相同，
            // 因此 t1 可以直接加载到 BOOT_PAGE_TABLE 的实际物理地址。
            // 它只是恰好能工作！
            // ============
            // "li t0, {LINEAR_MAPPING_OFFSET}",
            "la t1, {BOOT_PAGE_TABLE}",
            // "sub t1, t1, t0",
            "li t2, 8 << 60",
            "srli t1, t1, 12",
            "or t1, t1, t2",
            "csrw satp, t1",
            "sfence.vma",
            // 现在虚拟地址已经启用，不过 PC 还在低地址，这会让一些相对寻址失效。
            // 目前不会出问题，因为我们也映射了低地址，但长远考虑这里还是要挪 PC。
            // 注意即使是内核也没办法直接改 PC，我们只能用跳转指令操作。
            "li t0, {LINEAR_MAPPING_OFFSET} + 4 * 2", // 注意这个偏移量和下面的汇编代码长度强相关！（是的这代码很烂）
            "auipc t1, 0",
            "add t1, t1, t0",
            "jr t1",
            // OK，现在一切正常了！
            "la sp, boot_stack_top",
            "j {main}",
            BOOT_PAGE_TABLE = sym BOOT_PAGE_TABLE,
            LINEAR_MAPPING_OFFSET = const LINEAR_MAPPING_OFFSET,
            main = sym main,
        ),
        target_arch = "loongarch64" => naked_asm!(
            // 启用 DMW
            "li.d $t0, {LINEAR_MAPPING_OFFSET} | 0x11", // 一致可缓存，PLV 0
            "csrwr $t0, 0x180",
            "li.d $t0, 0x10", // DA = 0, PG = 1
            "csrwr $t0, 0x0",
            // 和 RISC-V 64 的情况不一样，这里 PC 初始就位于高地址，所以接下来不用动了
            // （由于 LoongArch 64 物理地址空间最高不超过 60 位，不启用 DMW 程序貌似也是能正常跑的，多余高位会直接被忽略）
            "la $sp, boot_stack_top",
            "b {main}",
            LINEAR_MAPPING_OFFSET = const LINEAR_MAPPING_OFFSET,
            main = sym main,
        ),
    }
}

// main 必须不能返回（因为 _start 中用的是无条件跳转）
pub extern "C" fn main() -> ! {
    clear_bss();
    trap::trap_handler::init_k();

    #[cfg(target_arch = "loongarch64")]
    memory::tlb_refill_handler::init();

    init_heap();

    frame_allocator::init();

    logger::init();

    // 设置 LoongArch 64 下的页表格式
    #[cfg(target_arch = "loongarch64")]
    {
        use memory::PAGE_SHIFTS;

        unsafe {
            asm!(
                "csrwr {page_shift}, {STLBPS}",
                page_shift = in(reg) PAGE_SHIFTS[0],
                STLBPS = const 0x1e,
            );
        }

        unsafe {
            asm!(
                "csrwr {page_shift}, {TLBREHI}",
                page_shift = in(reg) PAGE_SHIFTS[0],
                TLBREHI = const 0x8e,
            );
        }

        // LoongArch 64 支持五级页表，格式也可以自定义，不过我们这里配成 Sv39 就够了
        {
            use bit_field::BitField;

            let mut pwcl = 0;
            pwcl.set_bits(0..=4, PAGE_SHIFTS[0]); // PTbase
            pwcl.set_bits(5..=9, PAGE_SHIFTS[1] - PAGE_SHIFTS[0]); // PTwidth
            pwcl.set_bits(10..=14, PAGE_SHIFTS[1]); // Dir1_base
            pwcl.set_bits(15..=19, PAGE_SHIFTS[2] - PAGE_SHIFTS[1]); // Dir1_width
            pwcl.set_bits(20..=24, PAGE_SHIFTS[2]); // Dir2_base
            pwcl.set_bits(25..=29, 9); // Dir2_width
            pwcl.set_bits(30..=31, 0); // PTEWidth

            let pwch = 0;

            unsafe {
                asm!(
                    "csrwr {pwcl}, {PWCL}",
                    "csrwr {pwch}, {PWCH}",
                    pwcl = in(reg) pwcl,
                    pwch = in(reg) pwch,
                    PWCL = const 0x1c,
                    PWCH = const 0x1d,
                );
            }
        }

        // 清空整个 TLB
        unsafe { asm!("invtlb 0x0, $zero, $zero") };
    }

    filesystem::init_all();

    println!("Hello, world!");
    println!("中文也可以！");
    println!();

    (|| {
        (|| {
            (|| {
                println!(
                    "下面是一条从很深的地方发出的 stacktrace（尽管 release 模式下会被优化掉）"
                );
                backtrace();
            })()
        })()
    })();

    println!("准备进入用户态...");

    let root = {
        let tmpfs_type = filesystem::filesystems::get_filesystem_type("tmpfs").unwrap();

        let tmpfs = tmpfs_type.ops.new_fs();

        let root = tmpfs.init_root_dentry.clone();

        // 对于根目录所在的文件系统，其根目录直接挂载到自身
        let root_circle_mount = unsafe { vfs::MountPoint::mount_root(tmpfs.clone()) };
        ROOT_PATH.init(Path(root.clone(), root_circle_mount.clone()));

        debug_assert!(root.data.lock().mount.is_some());

        let tmp_1 = walk_path(vfs::PathToWalk::Absolute(ByteStr::new("/tmp")))
            .unwrap()
            .ensure_neg()
            .unwrap();
        tmpfs
            .root_inode
            .data
            .write()
            .dynamic_data
            .mkdir(tmp_1.0.0.clone(), vfs::INodeMode::all())
            .unwrap();

        let tmpfs_tmp = tmpfs_type.ops.new_fs();
        let tmp_2 = tmpfs_tmp.init_root_dentry.clone();
        vfs::MountPoint::mount(tmp_1.0, tmpfs_tmp, tmp_2);
        Path(root, root_circle_mount)
    };

    TASK_SCHEDULER.write().add_task(Task::new(
        multitask::SchedulePolicy::RoundRobin,
        40,
        user_start::<16, "Hello, world!\n"> as usize,
        root.clone(),
    ));
    seq_macro::seq!(N in 1..=3 {
        TASK_SCHEDULER.write().add_task(Task::new(
            multitask::SchedulePolicy::RoundRobin,
            40,
            user_start::<{10 + N}, { concat!("Hello, world ", N, "!\n") }> as usize,
            root.clone(),
        ));
    });

    // TASK_SCHEDULER.write().add_task(Task::new(
    //     multitask::SchedulePolicy::RoundRobin,
    //     50,
    //     (|| loop {}) as fn() as usize,
    // ));
    TASK_SCHEDULER.write().add_task(Task::new(
        multitask::SchedulePolicy::RoundRobin,
        50,
        user_start::<5, "VME50\n"> as usize,
        root.clone(),
    ));
    TASK_SCHEDULER.write().add_task(Task::new(
        multitask::SchedulePolicy::RoundRobin,
        60,
        user_start::<1, "BOOOOM!\n"> as usize,
        root,
    ));

    // TASK_SCHEDULER.write().add_task(Task::new(multitask::SchedulePolicy::FIFO, 0, just_for_test as usize));

    let task = TASK_SCHEDULER.write().pick_next().unwrap();
    let task = unsafe { task.into_asm().as_ref_unchecked() };
    let entry = task.entry.write().take().unwrap();

    // trap::set_global_interrupts_enable(true);
    trap::enable_timer_interrupt();
    basic::timer::start_timer_tick_tock();

    // 注意这个不能写进下面的函数调用，否则神必的 Rust 编译器不会析构 ref
    let page_table_reg = task
        .memory_space
        .read()
        .get_page_table()
        .to_native_register();

    run_user_program(
        entry - OFFSET,
        task.ustack,
        task.kstack.get_addr(),
        page_table_reg,
    );
}

fn clear_bss() {
    let sbss: *mut u8;
    let ebss: *mut u8;
    unsafe {
        asm!("la {0}, sbss", out(reg) sbss);
        asm!("la {0}, ebss", out(reg) ebss);
    }

    unsafe { volatile_set_memory(sbss, 0, ebss.offset_from_unsigned(sbss)) };
}

// FIXME：temp
const OFFSET: usize = cfg_select! {
    target_arch = "riscv64" =>     0xffffffc000000000,
    target_arch = "loongarch64" => 0x1000000000000000,
};

// FIXME: temp
fn run_program(task: TaskArc) {
    let task = unsafe { task.into_asm().as_ref_unchecked() };

    if core::ptr::eq(task, current_task()) {
        // 仍然切换到当前任务，直接返回即可。
        // 注意这不仅是优化，也是正确性所必需的：下面的 switch_context 分支无法处理这种情况。
        return;
    }

    let mut ent = task.entry.write();
    if let Some(entry) = *ent {
        *ent = None;
        drop(ent);
        cfg_select! {
            target_arch = "riscv64" => unsafe {
                use riscv::register::sstatus;
                sstatus::set_spp(sstatus::SPP::User);

                asm!(
                    "csrw stvec, {trap_handler_raw_from_user}",
                    trap_handler_raw_from_user = in(reg) trap::trap_handler::trap_handler_raw_from_user,
                );
            },
            target_arch = "loongarch64" => unsafe {
                use bit_field::BitField;

                let mut prmd: usize;
                asm!("csrrd {0}, 0x0", out(reg) prmd); // CRMD
                prmd.set_bits(0..2, 3);
                prmd.set_bit(2, true);
                asm!("csrwr {0}, 0x1", in(reg) prmd);

                asm!(
                    "csrwr {trap_handler_raw_from_user}, 0xc",
                    trap_handler_raw_from_user = in(reg) trap::trap_handler::trap_handler_raw_from_user,
                );
            },
        }

        // 注意这个不能写进下面的函数调用，否则神必的 Rust 编译器不会析构 ref
        let page_table_reg = task
            .memory_space
            .read()
            .get_page_table()
            .to_native_register();

        save_current_and_run(
            entry - OFFSET,
            task.ustack,
            task.kstack.get_addr(),
            page_table_reg,
        );
    } else {
        drop(ent);
        unsafe { switch_context(task.get_context().as_ref_unchecked()) };
    }
}

/// 从内核态切换到用户态，执行给定代码。
///
/// 不会清理通用寄存器。
fn run_user_program(entry: usize, stack: usize, kstack: usize, page_table_reg: usize) -> ! {
    cfg_select! {
        target_arch = "riscv64" => unsafe {
            use riscv::register::sstatus;

            sstatus::set_spp(sstatus::SPP::User);

            asm!(
                "csrw stvec, {trap_handler_raw_from_user}",
                "csrw satp, {satp}",
                "csrw sscratch, {stack}",
                "csrw sepc, {entry}",
                "csrrw sp, sscratch, {kstack}",
                "sret",
                trap_handler_raw_from_user = in(reg) trap::trap_handler::trap_handler_raw_from_user,
                satp = in(reg) page_table_reg,
                stack = in(reg) stack,
                entry = in(reg) entry,
                kstack = in(reg) kstack,
                options(noreturn),
            );
        },
        target_arch = "loongarch64" => unsafe {
            use bit_field::BitField;

            let mut prmd: usize;
            asm!("csrrd {0}, 0x0", out(reg) prmd); // CRMD
            prmd.set_bits(0..2, 3);
            prmd.set_bit(2, true);
            asm!("csrwr {0}, 0x1", in(reg) prmd);

            asm!(
                "csrwr {trap_handler_raw_from_user}, 0xc",
                "csrwr {pgdl}, 0x19",
                "csrwr {entry}, 0x6",
                "csrwr {kstack}, 0x30",
                "move $sp, {stack}",
                "ertn",
                trap_handler_raw_from_user = in(reg) trap::trap_handler::trap_handler_raw_from_user,
                pgdl = in(reg) page_table_reg,
                entry = in(reg) entry,
                stack = in(reg) stack,
                kstack = in(reg) kstack,
                options(noreturn),
            );
        },
    }
}

#[unsafe(naked)]
extern "C" fn save_current_and_run(
    entry: usize,
    stack: usize,
    kstack: usize,
    page_table_reg: usize,
) {
    use core::mem::offset_of;
    use seq_macro::seq;

    const TASK_CONTEXT_SIZE: usize = size_of::<TaskContext>();
    const RA_OFFSET: usize = offset_of!(TaskContext, return_address);
    const TASK_ADDR_OFFSET: usize = offset_of!(TaskContext, task_pointer);
    const SP_OFFSET: usize = offset_of!(TaskContext, stack_pointer);
    const PAGE_TABLE_OFFSET: usize = offset_of!(TaskContext, page_table_register);
    const REGS_OFFSET: usize = offset_of!(TaskContext, callee_saved_registers);

    const KSTACK_OFFSET_IN_TASK: usize = offset_of!(crate::multitask::task::Task, kstack);

    cfg_select! {
        target_arch = "riscv64" => seq!(N in 0..=11 { naked_asm!(
            // 1. 在当前内核栈上分配一块空间，用于存储当前上下文
            "addi sp, sp, -{TASK_CONTEXT_SIZE}",
            // 2. 保存当前上下文（ra、tp、sp、satp、s0~s11）
            "sd ra, {RA_OFFSET}(sp)",
            "sd tp, {TASK_ADDR_OFFSET}(sp)",
            "sd sp, {SP_OFFSET}(sp)",
            "csrr t0, satp",
            "sd t0, {PAGE_TABLE_OFFSET}(sp)",
            #(
                concat!("sd s", N, ", ", N, "*8+{REGS_OFFSET}(sp)"),
            )*
            // 3. 更新当前 Task 结构体的内核栈顶指针
            "sd sp, {KSTACK_OFFSET_IN_TASK}(tp)",

            "csrw sscratch, a1",
            "csrw sepc, a0",
            "csrrw sp, sscratch, a2",
            "csrw satp, a3",
            "sfence.vma",
            "sret",
            TASK_CONTEXT_SIZE = const TASK_CONTEXT_SIZE,
            RA_OFFSET = const RA_OFFSET,
            SP_OFFSET = const SP_OFFSET,
            PAGE_TABLE_OFFSET = const PAGE_TABLE_OFFSET,
            REGS_OFFSET = const REGS_OFFSET,
            TASK_ADDR_OFFSET = const TASK_ADDR_OFFSET,
            KSTACK_OFFSET_IN_TASK = const KSTACK_OFFSET_IN_TASK,
        )}),
        target_arch = "loongarch64" => seq!(N in 0..=8 { naked_asm!(
            // 1. 在当前内核栈上分配一块空间，用于存储当前上下文
            "addi.d $sp, $sp, -{TASK_CONTEXT_SIZE}",
            // 2. 保存当前上下文（$ra、$tp、$sp、PGDL、$s0~$s8）
            "st.d $ra, $sp, {RA_OFFSET}",
            "st.d $tp, $sp, {TASK_ADDR_OFFSET}",
            "st.d $sp, $sp, {SP_OFFSET}",
            "csrrd $t0, 0x19",
            "st.d $t0, $sp, {PAGE_TABLE_OFFSET}",
            #(
                concat!("st.d $s", N, ", $sp, ", N, "*8+{REGS_OFFSET}"),
            )*
            // 3. 更新当前 Task 结构体的内核栈顶指针
            "st.d $sp, $tp, {KSTACK_OFFSET_IN_TASK}",

            "csrwr $a0, 0x6",
            "csrwr $a2, 0x30",
            "csrwr $a3, 0x19",
            "invtlb 0x0, $zero, $zero",
            "move $sp, $a1",
            "ertn",
            TASK_CONTEXT_SIZE = const TASK_CONTEXT_SIZE,
            RA_OFFSET = const RA_OFFSET,
            SP_OFFSET = const SP_OFFSET,
            PAGE_TABLE_OFFSET = const PAGE_TABLE_OFFSET,
            REGS_OFFSET = const REGS_OFFSET,
            TASK_ADDR_OFFSET = const TASK_ADDR_OFFSET,
            KSTACK_OFFSET_IN_TASK = const KSTACK_OFFSET_IN_TASK,
        )}),
    }
}

fn user_start<const RET: usize, const STR: &'static str>() -> ! {
    fn user_main<const RET: usize, const STR: &'static str>() {
        // for _ in 0..RET {
        //     syscall!(233, (STR as *const str).addr(), STR.len());
        //     // syscall!(124);
        // }
        let mut buf = [0u8; 100];
        let buf2 = syscall!(17, buf.as_mut_ptr(), buf.len());
        if buf2 != buf.as_ptr().addr() {
            syscall!(93, 111111);
        }
        let Ok(str) = core::ffi::CStr::from_bytes_until_nul(&buf) else {
            syscall!(93, 111112);
            unreachable!();
        };
        syscall!(233, "CWD:".as_ptr(), "CWD:".len());
        syscall!(233, str.as_ptr(), str.count_bytes());
        syscall!(233, "\n".as_ptr(), 1);

        if RET != 1 {
            let ret = syscall!(
                56,
                -100,
                "MY_FILE\0".as_ptr(),
                0b10 | (1 << 6) | (1 << 10),
                0o777
            ) as i32;
            if ret < 0 {
                syscall!(93, 20000 - (ret as isize));
            }
            let len = syscall!(64, ret, STR.as_ptr(), STR.len());
            if len != STR.len() {
                syscall!(93, 20999);
            }
            syscall!(233, "WROTE:".as_ptr(), "WROTE:".len());
            syscall!(233, STR.as_ptr(), STR.len());
            syscall!(233, "\n".as_ptr(), 1);

            let ret = syscall!(56, -8, "/MY_FILE\0".as_ptr(), 0b10) as i32;
            if ret < 0 {
                syscall!(93, 24000 - (ret as isize));
            }
            let mut buffer = [100u8; 100];
            let bytes = syscall!(63, ret, buffer.as_mut_ptr(), buffer.len());
            syscall!(233, "DATA:".as_ptr(), "DATA:".len());
            syscall!(233, buffer.as_ptr(), bytes);
            syscall!(233, "\n\n".as_ptr(), "\n\n".len());
        } else {
            let ret = syscall!(56, -3, "/MY_FILE\0".as_ptr(), 0b10) as i32; // openat
            if ret < 0 {
                syscall!(93, 21000 - (ret as isize));
            }
            let mut buffer = [100u8; 100];
            let bytes = syscall!(63, ret, buffer.as_mut_ptr(), buffer.len()); // read
            syscall!(233, buffer.as_ptr(), bytes);
            syscall!(233, "\n========\n".as_ptr(), "\n========\n".len());
            let ret = syscall!(34, -100, "MY_DIR\0".as_ptr(), 0o777) as i32; // mkdirat
            if ret < 0 {
                syscall!(93, 22000 - (ret as isize));
            }
            let ret = syscall!(34, -100, "./MY_DIR/MY_SUBDIR\0".as_ptr(), 0o777) as i32; // mkdirat
            if ret < 0 {
                syscall!(93, 23000 - (ret as isize));
            }
            let ret = syscall!(56, -100, "MY_DIR/MY_SUBDIR\0".as_ptr(), 1 << 16, 0o777) as i32; // openat
            if ret < 0 {
                syscall!(93, 23500 - (ret as isize));
            }
            let ret = syscall!(
                56,
                ret,
                "FFFILE\0".as_ptr(),
                0b10 | (1 << 6) | (1 << 7),
                0o777
            ) as i32; // openat
            if ret < 0 {
                syscall!(93, 24000 - (ret as isize));
            }
            let fffile = ret;
            let ret = syscall!(49, "/MY_DIR/MY_SUBDIR/../MY_SUBDIR\0".as_ptr()); // chdir
            if ret != 0 {
                syscall!(93, 25000 - ret as isize);
            }
            let ret = syscall!(37, -100, "FFFILE\0".as_ptr(), -9, "/S\0".as_ptr()) as isize; // linkat
            if ret < 0 {
                syscall!(93, 26000 - (ret as isize));
            }
            let ret =
                syscall!(64, fffile, "1233211234567".as_ptr(), "1233211234567".len()) as isize; // write
            if ret < 0 {
                syscall!(93, 27000 - (ret as isize));
            }
            let ret = syscall!(35, -100, "FFFILE\0".as_ptr(), 0) as isize; // unlinkat
            if ret < 0 {
                syscall!(93, 28000 - (ret as isize));
            }
            {
                let _ = syscall!(17, buf.as_mut_ptr(), buf.len());
                let Ok(str) = core::ffi::CStr::from_bytes_until_nul(&buf) else {
                    syscall!(93, 111112);
                    unreachable!();
                };
                syscall!(233, "CWD:".as_ptr(), "CWD:".len());
                syscall!(233, str.as_ptr(), str.count_bytes());
                syscall!(233, "\n".as_ptr(), 1);
            }
            let ret = syscall!(56, -100, "../../S\0".as_ptr(), 0b10, 0o777) as i32; // openat
            if ret < 0 {
                syscall!(93, 29000 - (ret as isize));
            }
            let s = ret;
            syscall!(
                233,
                "====== data ======\n".as_ptr(),
                "====== data ======\n".len()
            );
            let bytes = syscall!(63, s, buffer.as_mut_ptr(), buffer.len()); // read
            syscall!(233, buffer.as_ptr(), bytes);
            syscall!(233, "\n\n".as_ptr(), 2);
            let duped_s = syscall!(23, s); // dup
            if duped_s < 0 {
                syscall!(93, 30000 - duped_s as isize);
            }
            let dup3_s = syscall!(24, duped_s, 1145); // dup3
            if dup3_s != 1145 {
                syscall!(93, 31000 - dup3_s as isize);
            }
            let ret = syscall!(56, 1145, "../tmp/tt.txt\0".as_ptr(), 0b10 | (1 << 6), 0o777) as i32; // openat
            if ret < 0 {
                syscall!(93, 32000 - (ret as isize));
            }
            let tt = ret;
            let ret = syscall!(56, tt, "../../tmp/../tmp\0".as_ptr(), 0b10, 0o777) as isize; // linkat
            if ret < 0 {
                syscall!(93, 33000 - (ret as isize));
            }
        }
    }

    user_main::<RET, STR>();

    if RET == 1 {
        if syscall!(233, 0, 2333) != 2 {
            syscall!(93, 999999);
        }
    }

    syscall!(93, RET);
    unreachable!();
}
