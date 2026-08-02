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
#![feature(ptr_metadata)]
#![feature(deref_pure_trait)]
#![feature(debug_closure_helpers)]
#![feature(try_trait_v2)]
#![feature(slice_split_once)]
#![feature(unique_rc_arc)]
#![feature(get_mut_unchecked)]
#![feature(const_type_name)]
#![feature(offset_of_enum)]
#![feature(forget_unsized)]
#![feature(const_trait_impl)]
#![feature(never_type)]
#![feature(ptr_as_uninit)]
#![feature(associated_type_defaults)]
#![feature(iter_advance_by)]
#![feature(naked_functions)]
#![feature(let_chains)]
#![feature(non_null_from_ref)]
#![feature(ptr_as_ref_unchecked)]
#![feature(const_vec_string_slice)]
#![feature(integer_sign_cast)]
#![feature(new_zeroed_alloc)]
#![feature(trait_upcasting)]
#![feature(array_windows)]
#![feature(try_with_capacity)]
#![feature(try_reserve_kind)]
#![feature(pointer_is_aligned_to)]
#![feature(concat_bytes)]
#![feature(maybe_uninit_as_bytes)]
#![feature(int_roundings)]
#![feature(slice_ptr_get)]
#![feature(const_copy_from_slice)]
#![feature(extract_if)]
#![feature(slice_as_array)]
#![feature(btree_cursors)]
#![feature(allocator_api)]
#![feature(macro_metavar_expr)]
#![feature(vec_push_within_capacity)]
#![feature(layout_for_ptr)]
#![feature(ip_from)]
#![feature(unwrap_infallible)]
//
#![deny(unused_must_use)]

extern crate alloc;

use core::arch::asm;
use core::arch::global_asm;
use core::intrinsics::volatile_set_memory;

use flat_device_tree::Fdt;

use crate::arch::interfaces::{Arch, Regs, Trap};
use crate::basic::bstr::ByteStr;
use crate::basic::debugger::backtrace;
use crate::basic::global_allocator::init_heap;
use crate::basic::logger;
use crate::basic::shared_with_asm::SharedWithAsm;
use crate::devices::DEVICES;
use crate::devices::pci::scan_pci_devices;
use crate::devices::virtio::scan_virtio_devices;
use crate::filesystem::vfs;
use crate::filesystem::vfs::ROOT_PATH;
use crate::filesystem::vfs::walk_path;
use crate::memory::ZERO_PAGE;
use crate::memory::address::PhysAddr;
use crate::memory::address::VirtAddr;
use crate::memory::frame_allocator;
use crate::memory::page_meta::PAGE_METADATAS;
use crate::multitask::task::Task;

mod arch;
mod basic;
mod devices;
mod filesystem;
mod memory;
mod multitask;
mod network;
mod syscall;
mod trap;

const BOOT_STACK_BYTES: usize = 128 * 1024; // 128KB，设小了会带来不幸（欸我操这 page table 怎么这么大.jpg）

// 每个核都需要自己的启动栈（次要核在进入各自的空闲任务前会短暂使用它）。
// 布局：第 0 号栈为启动核（0 号核）的启动栈（其栈顶即 `boot_stack_top`），
// 第 N 号栈（1 <= N <= MAX_CPUS）为 N 号核的启动栈（栈顶为 boot_stacks_base + N * BOOT_STACK_BYTES）。
// 注意 MAX_CPUS 号栈实际上是多余的（核的编号从 0 开始），但多预留一个无妨。
global_asm!(
    ".section .bss.stack",
    ".globl boot_stacks_base",
    "boot_stacks_base:",
    "    .space {BOOT_STACK_BYTES}",
    ".globl boot_stack_top",
    "boot_stack_top:",
    "    .space {BOOT_STACK_BYTES} * {MAX_CPUS}",
    BOOT_STACK_BYTES = const BOOT_STACK_BYTES,
    MAX_CPUS = const basic::per_cpu::MAX_CPUS,
);

// main 必须不能返回（因为 _start 中用的是无条件跳转）
pub extern "C" fn main(_: usize, dtb: PhysAddr) -> ! {
    clear_bss();
    arch::NativeRegs::set_trap_handler(arch::NativeTrap::trap_handler_raw_from_kernel);

    arch::NativeRegs::set_tp(0);

    // 完成当前 CPU 上 TLB 相关初始化（TLB 相关的 CSR 是每核独立的）
    #[cfg(target_arch = "loongarch64")]
    memory::tlb_refill_handler::init_cpu();

    logger::init();

    init_heap();

    frame_allocator::init();

    basic::timer::init_clock_freq();

    filesystem::init();

    basic::timer::init();

    trap::trap_handler::init_fixups();

    #[cfg(feature = "has-debug-info")]
    basic::debugger::DEBUG_CONTEXT.init();

    log::debug!("设备树物理地址：{:#x}", dtb.0);
    let fdt = unsafe { Fdt::from_ptr(dtb.to_virtual_address().0 as *const u8) }.unwrap();

    let mut devices = devices::Devices::new();

    scan_virtio_devices(&fdt, &mut devices);
    scan_pci_devices(&fdt, &mut devices);

    unsafe { basic::UnsafeInited::init_unsafe(&DEVICES, devices) };

    log::info!("设备树扫描完毕！");
    for device in &DEVICES.block {
        log::info!(
            "识别到块设备: {name}, 块大小 {block_size} 字节, 总大小 {block_size} * {block_count} 字节",
            name = match &device.device_id() {
                Ok(x) if x.is_empty() => "<无名>",
                Ok(other) => other,
                Err(_) => "<读取过程中发生错误>",
            },
            block_size = device.block_size(),
            block_count = device.block_count(),
        );
    }

    filesystem::devfs::drivers::init();
    network::namespace::init();

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

                // println!("测试崩溃时的调试信息：");
                // unsafe { (0 as *const u8).read_volatile() };
            })()
        })()
    })();

    // 初始化文件系统
    {
        use filesystem::filesystems::get_filesystem_type;

        let ext4_type = get_filesystem_type(ByteStr::new("ext4")).unwrap();
        let tmpfs_type = get_filesystem_type(ByteStr::new("tmpfs")).unwrap();
        let devfs_type = get_filesystem_type(ByteStr::new("devfs")).unwrap();
        let procfs_type = get_filesystem_type(ByteStr::new("procfs")).unwrap();

        // FIXME
        let block_device_1 = &*DEVICES.block[0];
        let block_device_2 = &*DEVICES.block[1];

        // RISC-V disk is 5 MiB (10240 blocks), LoongArch disk is 25 MiB (51200 blocks)
        #[cfg(target_arch = "riscv64")]
        const ROOT_BLOCK_COUNT: u64 = 10240; // 5 MiB
        #[cfg(target_arch = "loongarch64")]
        const ROOT_BLOCK_COUNT: u64 = 51200; // 25 MiB

        let (root_device, sdcard_device) = if block_device_1.block_count() == ROOT_BLOCK_COUNT {
            log::info!("已选定块设备 1 作为根文件系统");
            (block_device_1, block_device_2)
        } else {
            assert!(block_device_2.block_count() == ROOT_BLOCK_COUNT);
            log::info!("已选定块设备 2 作为根文件系统");
            (block_device_2, block_device_1)
        };

        filesystem::init_root(ext4_type, root_device);

        let root_fs = ROOT_PATH.1.sub_fs.clone();

        fn get_or_create_path(root_fs: &vfs::FileSystem, path: &str) -> vfs::Path {
            match walk_path(vfs::PathToWalk::Absolute(ByteStr::new(path))).unwrap() {
                vfs::WalkedPath::Positive(path) => path,
                vfs::WalkedPath::Negative(npath) => {
                    root_fs
                        .root_inode
                        .data
                        .write()
                        .mkdir(npath.0.0.clone(), vfs::INodeMode::all_permitted())
                        .unwrap();
                    npath.0
                }
            }
        }

        let tmp_1 = get_or_create_path(&root_fs, "/tmp");
        let tmp_fs = tmpfs_type.new_fs(None);
        let tmp_2 = tmp_fs.root_dentry.clone();
        vfs::MountPoint::mount(tmp_1, tmp_fs, tmp_2);
        log::info!("已将 tmpfs 挂载到 /tmp");

        let dev_1 = get_or_create_path(&root_fs, "/dev");
        let dev_fs = devfs_type.new_fs(None);
        let dev_2 = dev_fs.root_dentry.clone();
        vfs::MountPoint::mount(dev_1, dev_fs, dev_2);
        log::info!("已将 devfs 挂载到 /dev");
        init_devs();

        let dev_shm = get_or_create_path(&root_fs, "/dev/shm");
        let dev_shm_fs = tmpfs_type.new_fs(None);
        let dev_shm_2 = dev_shm_fs.root_dentry.clone();
        vfs::MountPoint::mount(dev_shm, dev_shm_fs, dev_shm_2);
        log::info!("已将 tmpfs 挂载到 /dev/shm");

        let sdcard_1 = get_or_create_path(&root_fs, "/sdcd");
        let sdcard_fs = ext4_type.new_fs(Some(sdcard_device));
        let sdcard_2 = sdcard_fs.root_dentry.clone();
        vfs::MountPoint::mount(sdcard_1, sdcard_fs, sdcard_2);
        log::info!("已将 sdcard 挂载到 /sdcd");

        let proc_1 = get_or_create_path(&root_fs, "/proc");
        let proc_fs = procfs_type.new_fs(None);
        let proc_2 = proc_fs.root_dentry.clone();
        vfs::MountPoint::mount(proc_1, proc_fs, proc_2);
        log::info!("已将 procfs 挂载到 /proc");
        init_proc();

        let _ = get_or_create_path(&root_fs, "/etc");

        let _ = get_or_create_path(&root_fs, "/usr");

        let _ = get_or_create_path(&root_fs, "/var");

        let _ = get_or_create_path(&root_fs, "/bin"); // 对其内容的填充在 init 中完成

        let lib = get_or_create_path(&root_fs, "/lib");
        init_libs(lib.clone());
        let lib64 = get_or_create_path(&root_fs, "/lib64");
        vfs::MountPoint::mount(lib64, root_fs.clone(), lib.0.clone());
        let usr_lib64 = get_or_create_path(&root_fs, "/usr/lib64");
        vfs::MountPoint::mount(usr_lib64, root_fs, lib.0);
    }

    let zero_page_num = unsafe {
        PAGE_METADATAS
            .mark_new_page(
                VirtAddr::from_ptr(&ZERO_PAGE).to_physical_address().into(),
                memory::page_meta::PageType::Phantom,
            )
            .into_asm()
    };
    log::info!("零页 {:#x} 初始化完毕", zero_page_num.0);

    multitask::futex::init();

    // 创建各核的空闲任务并启动其它核（SMP）
    basic::per_cpu::init();
    basic::per_cpu::start_secondary_cpus();

    // FIXME

    println!("准备进入用户态...");


    // trap::set_global_interrupts_enable(true);
    trap::enable_timer_interrupt();
    basic::timer::start_timer_tick_tock();

    let rv = vfs::walk_path(vfs::PathToWalk::Absolute(arch::Native::INIT_EXEC_PATH))
        .unwrap()
        .ensure()
        .unwrap();

    let file =
        rv.0.data
            .lock()
            .ops
            .open(rv.1, vfs::FileFlags::Read)
            .unwrap();

    Task::execute_init(
        multitask::SchedulePolicy::RoundRobin {
            rest_time_slice: core::sync::atomic::AtomicU8::new(u8::MAX),
        },
        50,
        file,
        ROOT_PATH.clone(),
    );
}

fn clear_bss() {
    let sbss: *mut u8;
    let ebss: *mut u8;
    unsafe {
        asm!("la {0}, sbss", out(reg) sbss);
        asm!("la {0}, ebss", out(reg) ebss);
    }

    unsafe { volatile_set_memory(sbss, 0, ebss.offset_from(sbss).cast_unsigned()) };
}

fn init_devs() {
    let dev = vfs::walk_path(vfs::PathToWalk::Absolute(ByteStr::new("/dev")))
        .unwrap()
        .ensure()
        .expect("根目录下应该有 dev 目录");

    let dev_inode = dev.0.data.lock().inode.clone().unwrap();

    let mode = vfs::INodeMode::all_permitted();

    macro_rules! define {
        ($name:literal, $kind:expr) => {{
            let neg = vfs::walk_path(vfs::PathToWalk::Relative(ByteStr::new($name), dev.clone()))
                .unwrap()
                .ensure_neg()
                .expect(concat!("/dev/", $name, " 应当初始不存在"));

            dev_inode
                .data
                .write()
                .make_node($kind, mode, neg.0.0)
                .unwrap();
        }};
    }

    use filesystem::devfs::drivers::*;
    use vfs::SpecialINodeType::CharDevice;

    define!("tty", CharDevice(DEV_TTY));
    define!("urandom", CharDevice(DEV_URANDOM));
    define!("null", CharDevice(DEV_NULL));
    define!("zero", CharDevice(DEV_ZERO));
}

fn init_libs(lib: vfs::Path) {
    let lib_inode = lib.0.data.lock().inode.clone().unwrap();

    macro_rules! define {
        ($from: expr => $to: literal) => {{
            let walked = vfs::walk_path_no_follow(vfs::PathToWalk::Relative(ByteStr::new($to), lib.clone()))
                .unwrap();

            match walked {
                vfs::WalkedPath::Positive(_) => log::trace!(concat!("/lib/", $to, " 已存在，跳过创建符号链接")),
                vfs::WalkedPath::Negative(npath) => {
                    use alloc::borrow::ToOwned;
                    lib_inode
                        .data
                        .write()
                        .symlink(ByteStr::new($from).to_owned(), npath.0.0)
                        .unwrap();

                    log::trace!(concat!("/lib/", $to, " 符号链接已创建，指向 ", $from));
                }
            };
        }};

        ($prefix: tt lib $($name: literal)+ => /lib/...) => {
            $(define!(concat!("/sdcd/", stringify!($prefix), "/lib/", $name) => $name);)+
        }
    }

    define!("/sdcd/musl/lib/libc.so" => "ld-musl-riscv64-sf.so.1");
    define!("/sdcd/musl/lib/libc.so" => "ld-musl-riscv64.so.1");

    define!("/sdcd/glibc/lib/libc.so" => "ld-libc.so.6");
    define!("/sdcd/glibc/lib/libm.so" => "ld-libm.so.6");

    // 此处有意先创建 musl 系的链接，因为基本上我们对 musl 支持得更好
    define!(musl lib "dlopen_dso.so" "libc.so" "tls_align_dso.so" "tls_get_new-dtv_dso.so" "tls_init_dso.so" => /lib/...);

    define!(glibc lib "dlopen_dso.so" "libc.so" "libm.so" "tls_align_dso.so" "tls_init_dso.so" "ld-linux-riscv64-lp64d.so.1" "libc.so.6" "libm.so.6" "tls_get_new-dtv_dso.so" => /lib/...);

    #[cfg(target_arch = "loongarch64")]
    {
        define!(glibc lib "ld-linux-loongarch-lp64d.so.1" => /lib/...);
        define!("/sdcd/musl/lib/libc.so" => "ld-musl-loongarch-lp64d.so.1");
        define!("/sdcd/musl/lib/libc.so" => "ld-musl-loongarch64-lp64d.so.1");
    }
}

fn init_proc() {
    let proc = vfs::walk_path(vfs::PathToWalk::Absolute(ByteStr::new("/proc")))
        .unwrap()
        .ensure()
        .expect("根目录下应该有 proc 目录");

    let proc_inode = proc.0.data.lock().inode.clone().unwrap();

    let mode = vfs::INodeMode::all_permitted();

    use filesystem::devfs::drivers::DeviceId;

    macro_rules! define {
        ($name:literal, $val:literal) => {{
            let neg = vfs::walk_path(vfs::PathToWalk::Relative(ByteStr::new($name), proc.clone()))
                .unwrap()
                .ensure_neg()
                .expect(concat!("/proc/", $name, " 应当初始不存在"));

            proc_inode
                .data
                .write()
                .make_node(
                    vfs::SpecialINodeType::BlockDevice(DeviceId(999, $val)),
                    mode,
                    neg.0.0,
                )
                .unwrap();
        }};
    }

    define!("meminfo", 0);
}
