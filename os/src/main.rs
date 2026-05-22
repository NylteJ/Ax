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
#![feature(generic_atomic)]
#![feature(ptr_metadata)]
#![feature(bstr)]
#![feature(deref_pure_trait)]
#![feature(debug_closure_helpers)]
#![feature(try_trait_v2)]
#![feature(deque_extend_front)]
#![feature(slice_split_once)]
#![feature(unique_rc_arc)]
#![feature(sized_hierarchy)]
#![feature(get_mut_unchecked)]
#![feature(const_type_name)]
#![feature(offset_of_enum)]
#![feature(const_result_unwrap_unchecked)]
#![feature(forget_unsized)]
#![feature(const_convert)]
#![feature(const_trait_impl)]
#![feature(never_type)]
#![feature(ptr_as_uninit)]
//
#![deny(unused_must_use)]

extern crate alloc;

use core::arch::asm;
use core::arch::global_asm;
use core::arch::naked_asm;
use core::bstr::ByteStr;
use core::intrinsics::volatile_set_memory;
use core::ops::Deref;

use flat_device_tree::Fdt;

use crate::basic::debugger::backtrace;
use crate::basic::global_allocator::init_heap;
use crate::basic::logger;
use crate::basic::shared_with_asm::SharedWithAsm;
use crate::devices::BLOCK_DEVICES;
use crate::devices::pci::scan_pci_devices;
use crate::devices::virtio::scan_virtio_devices;
use crate::filesystem::vfs;
use crate::filesystem::vfs::ROOT_PATH;
use crate::filesystem::vfs::walk_path;
use crate::memory::ZERO_PAGE;
use crate::memory::address::LINEAR_MAPPING_OFFSET;
use crate::memory::address::PhysAddr;
use crate::memory::address::VirtAddr;
use crate::memory::frame_allocator;
use crate::memory::page_meta::PAGE_METADATAS;
use crate::multitask::task::Task;

mod basic;
mod devices;
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

    // 然后，由于 VirtIO 设备普遍使用 0x10000000 附近的地址，我们将这部分也进行映射
    entries[256 + 0] = PhysPageNum::<2>(0).mask() | VRWX;

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
            // LoongArch 64 支持设备树，但不会像 RISC-V 64 那样自动通过寄存器传参，而是始终为定值
            // 这里我们手动传给 main
            "li.d $a1, 0x100000",
            "b {main}",
            LINEAR_MAPPING_OFFSET = const LINEAR_MAPPING_OFFSET,
            main = sym main,
        ),
    }
}

// main 必须不能返回（因为 _start 中用的是无条件跳转）
pub extern "C" fn main(_: usize, dtb: PhysAddr) -> ! {
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

    filesystem::register_all();

    #[cfg(debug_assertions)]
    basic::debugger::DEBUG_CONTEXT.init();

    log::debug!("设备树物理地址：{:#x}", dtb.0);
    let fdt = unsafe { Fdt::from_ptr(dtb.to_virtual_address().0 as *const u8) }.unwrap();

    scan_virtio_devices(&fdt);
    scan_pci_devices(&fdt);

    log::info!("设备树扫描完毕！");
    for device in BLOCK_DEVICES.deref() {
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
        let ext4_type = filesystem::filesystems::get_filesystem_type("ext4").unwrap();
        let tmpfs_type = filesystem::filesystems::get_filesystem_type("tmpfs").unwrap();
        let devfs_type = filesystem::filesystems::get_filesystem_type("devfs").unwrap();

        filesystem::init_root(ext4_type);

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
        let tmp_fs = tmpfs_type.ops.new_fs();
        let tmp_2 = tmp_fs.root_dentry.clone();
        vfs::MountPoint::mount(tmp_1, tmp_fs, tmp_2);
        log::info!("已将 tmpfs 挂载到 /tmp");

        let dev_1 = get_or_create_path(&root_fs, "/dev");
        let dev_fs = devfs_type.ops.new_fs();
        let dev_2 = dev_fs.root_dentry.clone();
        vfs::MountPoint::mount(dev_1, dev_fs, dev_2);
        log::info!("已将 devfs 挂载到 /dev");
        init_devs();
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

    // FIXME

    println!("准备进入用户态...");

    // trap::set_global_interrupts_enable(true);
    trap::enable_timer_interrupt();
    basic::timer::start_timer_tick_tock();

    const INIT_PATH: &str = cfg_select! {
        target_arch = "riscv64" => "/rv-init",
        target_arch = "loongarch64" => "/la-init",
    };

    let rv = vfs::walk_path(vfs::PathToWalk::Absolute(ByteStr::new(INIT_PATH)))
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
        multitask::SchedulePolicy::RoundRobin,
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

    unsafe { volatile_set_memory(sbss, 0, ebss.offset_from_unsigned(sbss)) };
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
            let neg = vfs::walk_path(vfs::PathToWalk::Relative(ByteStr::new($name), dev))
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
}
