use core::ptr;

use crate::{print, println};

/// 断点函数，调试时会在调试器内触发断点，其他时候编译为无操作。
#[cfg_attr(debug_assertions, inline(never))]
#[cfg_attr(not(debug_assertions), inline(always))]
pub fn debugbreak() {
    // Makefile 魔术
}

/// 回溯打印当前调用栈，在 Debug 构建下带符号。
///
/// 目前实现依赖于帧指针。
#[cfg(feature = "framepointer")]
#[inline(always)]
pub fn backtrace() {
    use crate::arch::{self, interfaces::Regs};

    backtrace_at(arch::NativeRegs::get_fp() as *const usize);
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

        if fp < 0x80200000 {
            break;
        }

        #[cfg(target_arch = "riscv64")]
        if fp <= crate::memory::address::VIRTUAL_ADDRESS_HIGH_BASE || fp >= 0xfffffffffffff000 {
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
    #[cfg(feature = "has-debug-info")]
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

#[cfg(feature = "has-debug-info")]
pub struct DebugContext(
    crate::basic::sync::OnceInited<
        addr2line::Context<addr2line::gimli::EndianSlice<'static, addr2line::gimli::LittleEndian>>,
    >,
);

// SAFETY: 我没细看但其实大概是不安全的，不过没关系！
#[cfg(feature = "has-debug-info")]
unsafe impl Send for DebugContext {}
#[cfg(feature = "has-debug-info")]
unsafe impl Sync for DebugContext {}

#[cfg(feature = "has-debug-info")]
pub static DEBUG_CONTEXT: DebugContext = DebugContext(crate::basic::sync::OnceInited::new());

#[cfg(feature = "has-debug-info")]
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

        crate::basic::sync::OnceInited::init(&self.0, addr2line::Context::from_dwarf(file).unwrap())
    }
}

#[cfg(feature = "has-debug-info")]
impl core::ops::Deref for DebugContext {
    type Target =
        addr2line::Context<addr2line::gimli::EndianSlice<'static, addr2line::gimli::LittleEndian>>;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

#[cfg(feature = "has-debug-info")]
#[used]
#[unsafe(link_section = ".debug_infos")]
static _PLACEHOLDER: [u8; 32 * 1024 * 1024] = [1; 32 * 1024 * 1024];
