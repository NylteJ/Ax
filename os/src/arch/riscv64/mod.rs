#![cfg(target_arch = "riscv64")]

use core::{arch::naked_asm, bstr::ByteStr};

use crate::{
    arch::interfaces::Arch,
    main,
    memory::{
        address::{LINEAR_MAPPING_OFFSET, PhysPageNum},
        page::PTEFlagsNative,
    },
};

mod basic;
mod chrono;
mod regs;
mod trap;
mod uaccess_raw;

pub struct RiscV64;

unsafe impl Arch for RiscV64 {
    const INIT_EXEC_PATH: &ByteStr = ByteStr::new("/rv-init");

    type Regs = regs::Regs;
    type Basic = basic::Basic;
    type Chrono = chrono::Chrono;
    type UAccessRaw = uaccess_raw::UAccessRaw;
    type Trap = trap::Trap;

    #[unsafe(no_mangle)]
    #[unsafe(naked)]
    #[unsafe(link_section = ".text.entry")]
    extern "C" fn _start() {
        naked_asm!(
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
        );
    }
}

#[repr(align(4096))]
#[allow(dead_code)] // 尽管 Rust 侧不会读取其成员，汇编侧会访问它，因此并非“死代码”
struct BootPageTable([usize; 512]);

static BOOT_PAGE_TABLE: BootPageTable = const {
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
