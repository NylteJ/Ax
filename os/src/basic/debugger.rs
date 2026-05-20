use core::{arch::asm, ptr};

use crate::{print, println};

pub fn debugbreak() {
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("ebreak") },
        target_arch = "loongarch64" => unsafe { asm!("break 0") },
    };
}

/// 回溯打印当前调用栈，在 Debug 构建下带符号。
///
/// 目前实现依赖于帧指针。
#[cfg(feature = "framepointer")]
pub fn backtrace() {
    let frame_pointer: *const usize;
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("mv   {0}, s0",  out(reg) frame_pointer) },
        target_arch = "loongarch64" => unsafe { asm!("move {0}, $fp", out(reg) frame_pointer) },
    }
    backtrace_at(frame_pointer);
}

/// 回溯打印指定调用栈，在 Debug 构建下带符号。
///
/// 目前实现依赖于帧指针。
#[cfg(feature = "framepointer")]
pub fn backtrace_at(mut frame_pointer: *const usize) {
    if frame_pointer == ptr::null() {
        println!("ERROR: 帧指针为空");
        return;
    }

    for depth in 0..100 {
        use crate::memory::address::LINEAR_MAPPING_OFFSET;

        if !frame_pointer.is_aligned() {
            println!("帧指针未对齐，帧数据很可能已损坏，终止回溯...");
            return;
        }

        let pc = unsafe { *frame_pointer.sub(1) };
        let fp = unsafe { *frame_pointer.sub(2) };

        if cfg_select! {
            target_arch = "riscv64" => fp < 0x80200000, // RISC-V 这部分貌似会一路回溯到 SBI 里头，这部分我们不感兴趣
            target_arch = "loongarch64" => fp == 0, // 到达调用链末尾
        } {
            break;
        }

        #[cfg(target_arch = "riscv64")]
        if fp <= crate::memory::address::VIRTUAL_ADDRESS_HIGH_BASE {
            break;
        }

        print!("#{depth:2} {pc:#18x}, fp = {fp:#18x}  ");
        print_pc_info_if_debug(pc);
        println!();

        if pc <= LINEAR_MAPPING_OFFSET {
            break;
        }

        frame_pointer = fp as *const usize;
    }
}

pub fn print_pc_info_if_debug(pc: usize) {
    #[cfg(debug_assertions)]
    match DEBUG_CONTEXT.find_location(pc as u64) {
        Ok(None) => print!("<No location>"),
        Err(err) => print!("err: {err:?}"),
        Ok(Some(location)) => {
            if let Some(mut file) = location.file {
                if let Some(start) = file.find("/os/src/") {
                    file = &file[start + 1..];
                }
                print!("{file}");
            } else {
                print!("<No file>");
            }
            if let Some(line) = location.line {
                print!(":{line}");
            } else {
                print!(":<No line>");
            }
            if let Some(column) = location.column {
                print!(":{column}");
            } else {
                print!(":<No column>");
            }
        }
    }
}

#[cfg(debug_assertions)]
pub struct DebugContext(
    super::sync::OnceInited<
        addr2line::Context<addr2line::gimli::EndianSlice<'static, addr2line::gimli::LittleEndian>>,
    >,
);

// SAFETY: 我没细看但其实大概是不安全的，不过没关系！
#[cfg(debug_assertions)]
unsafe impl Send for DebugContext {}
#[cfg(debug_assertions)]
unsafe impl Sync for DebugContext {}

#[cfg(debug_assertions)]
pub static DEBUG_CONTEXT: DebugContext = DebugContext(super::sync::OnceInited::new());

#[cfg(debug_assertions)]
impl DebugContext {
    pub fn init(&self) {
        let __debug_data_start: usize;
        let __debug_data_end: usize;
        unsafe { core::arch::asm!("la {0}, __debug_data_start", out(reg) __debug_data_start) };
        unsafe { core::arch::asm!("la {0}, __debug_data_end", out(reg) __debug_data_end) };

        let elf_bytes = unsafe {
            core::slice::from_raw_parts(
                __debug_data_start as *const u8,
                __debug_data_end - __debug_data_start,
            )
        };

        let elf_file = object::File::parse(elf_bytes).unwrap();

        use addr2line::gimli;

        let file = gimli::Dwarf::load(
            |id| -> Result<gimli::EndianSlice<'static, gimli::LittleEndian>, gimli::Error> {
                use object::Object;
                use object::ObjectSection;

                let section = elf_file
                    .section_by_name(id.name())
                    .map(|s| s.data().unwrap())
                    .unwrap_or_default();

                Ok(gimli::EndianSlice::new(section, gimli::LittleEndian))
            },
        )
        .unwrap();

        super::sync::OnceInited::init(&self.0, addr2line::Context::from_dwarf(file).unwrap())
    }
}

#[cfg(debug_assertions)]
impl core::ops::Deref for DebugContext {
    type Target =
        addr2line::Context<addr2line::gimli::EndianSlice<'static, addr2line::gimli::LittleEndian>>;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

#[cfg(debug_assertions)]
#[used]
#[unsafe(link_section = ".debug_infos")]
static _PLACEHOLDER: [u8; 32 * 1024 * 1024] = [1; 32 * 1024 * 1024];
