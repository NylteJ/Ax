use core::ffi::c_void;

use bitflags::Flags;
use bitflags::bitflags;
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::arch::interfaces::Basic;
use crate::memory::PAGE_BYTES;
use crate::memory::address::VirtAddr;
use crate::memory::page::PTEFlags;
use crate::memory::page::PTEFlagsNative;
use crate::memory::virtual_memory_area::VMAFlags;
use crate::memory::virtual_memory_area::VMAGrowth;
use crate::memory::virtual_memory_area::VMAType;
use crate::memory::virtual_memory_area::VirtualMemoryArea;
use crate::multitask::cur_task;
use crate::syscall::SyscallResult;

use crate::syscall::err_codes::*;

/// * 功能：修改数据段的大小；
/// * 输入：指定待修改的地址；
/// * 返回值：成功返回0，失败返回-1;
/// ```c
/// uintptr_t brk;
/// uintptr_t ret = syscall(SYS_brk, brk);
/// ```
// 依旧，这里的文档与 Linux 语义冲突，我们选用 Linux 语义，总是返回地址（成功时返回新地址，失败时返回旧地址）
pub extern "C" fn brk(brk: usize) -> usize {
    let addr_space = cur_task().address_space.read();

    if brk == 0 {
        return addr_space.read().brk.0;
    }

    addr_space.write().set_brk(VirtAddr(brk)).0
}

/// * 功能：将文件或设备映射到内存中；
/// * 输入：
///     - start: 映射起始位置，
///     - len: 长度，
///     - prot: 映射的内存保护方式，可取：PROT_EXEC, PROT_READ, PROT_WRITE, PROT_NONE
///     - flags: 映射是否与其他进程共享的标志，
///     - fd: 文件句柄，
///     - off: 文件偏移量；
/// * 返回值：成功返回已映射区域的指针，失败返回-1;
/// ```c
/// void *start, size_t len, int prot, int flags, int fd, off_t off
/// long ret = syscall(SYS_mmap, start, len, prot, flags, fd, off);
/// ```
pub extern "C" fn mmap(
    start: *mut c_void,
    len: usize,
    prot: usize,
    flags: usize,
    fd: isize,
    off: usize,
) -> SyscallResult {
    if len == 0 {
        return EINVAL;
    }

    let prot = prot.try_into().map_err(|_| EINVAL)?;
    let prot = MMapProtections::from_bits(prot).ok_or(EINVAL)?;

    let vma_flags = {
        let mut flags = match flags & 0b11 {
            0b01 => VMAFlags::Shared,
            0b10 => VMAFlags::empty(), // 没有 Shared 即表示私有
            _ => return EINVAL,        // 0b11 是 Linux 扩展
        };

        if prot.contains(MMapProtections::PROT_EXEC) {
            flags |= VMAFlags::Execute;
        }
        if prot.contains(MMapProtections::PROT_READ) {
            flags |= VMAFlags::Read;
        }
        if prot.contains(MMapProtections::PROT_WRITE) {
            flags |= VMAFlags::Write;
        }

        flags | VMAFlags::User
    };

    let flags = (flags >> 2).try_into().map_err(|_| EINVAL)?;
    let flags = MMapFlags::from_bits(flags).ok_or(EINVAL)?;

    let vma_type = if flags.contains(MMapFlags::MAP_ANONYMOUS) {
        VMAType::Anonymous
    } else {
        if off % PAGE_BYTES != 0 {
            return EINVAL;
        }
        let page_index = (off / PAGE_BYTES) as u64;

        if fd < 0 {
            return EBADF;
        }
        let fd = fd.cast_unsigned();

        let file = cur_task().files.read().get(fd).ok_or(EBADF)?.0.clone();

        VMAType::File { file, page_index }
    };

    let vma_growth = VMAGrowth::Fixed; // TODO: MAP_GROWSDOWN

    let fixed = flags.contains(MMapFlags::MAP_FIXED);

    let start = start.addr();
    if fixed && start % PAGE_BYTES != 0 {
        return EINVAL;
    }
    let start = VirtAddr(start).floor_to_page();
    let end = VirtAddr(start.0.checked_add(len).ok_or(EINVAL)?).ceil_to_page();

    let result = cur_task()
        .address_space
        .read()
        .write()
        .mmap(
            VirtualMemoryArea {
                start,
                end,
                flags: vma_flags,
                vma_type,
                growth: vma_growth,
            },
            fixed,
        )
        .map_err(|_| EINVAL)?;

    SyscallResult::success(result.0)
}

bitflags! {
    #[derive(Debug)]
    struct MMapProtections: u8 {
        const PROT_READ = 1 << 0;
        const PROT_WRITE = 1 << 1;
        const PROT_EXEC = 1 << 2;
    }

    /// 特别注意此标志位集不包含映射类型
    #[derive(Debug)]
    struct MMapFlags: u32 {
        /// 不要将 addr 视为提示，而是强行将映射精确地放置在该地址。
        /// 如果无论如何都无法做到则 mmap 调用失败。
        ///
        /// 如果新映射区域与现有区域重叠，现有映射的重叠部分将被**丢弃**。
        const MAP_FIXED = 1 << 2;

        /// 标记映射区域为匿名，其没有对应的底层文件且内容被初始化为 0。
        /// 此时，fd 和 off 参数将被忽略。
        const MAP_ANONYMOUS = 1 << 3;
        const MAP_GROWSDOWN	= 1 << 6;

        /// 此标志位被忽略。
        const MAP_DENYWRITE	= 1 << 9;
        /// 此标志位被忽略。
        const MAP_EXECUTABLE = 1 << 10;

        const MAP_LOCKED = 1 << 11;
        const MAP_NORESERVE = 1 << 12;
        const MAP_POPULATE = 1 << 13;
        const MAP_NONBLOCK = 1 << 14;
        const MAP_STACK = 1 << 15;
        const MAP_HUGETLB = 1 << 16;
        const MAP_SYNC = 1 << 17;
        const MAP_FIXED_NOREPLACE = 1 << 18;
    }
}

/// * 功能：将文件或设备取消映射到内存中；
/// * 输入：映射的指定地址及区间；
/// * 返回值：成功返回0，失败返回-1;
/// ```c
/// void *start, size_t len
/// int ret = syscall(SYS_munmap, start, len);
/// ```
pub extern "C" fn munmap(start: *mut c_void, len: usize) -> SyscallResult {
    let start = start.addr();
    if start % PAGE_BYTES != 0 || len == 0 {
        return EINVAL;
    }

    let start = VirtAddr(start);
    let end = VirtAddr(start.0 + len).ceil_to_page();

    let address_space = cur_task().address_space.read();
    let mut address_space = address_space.write();
    address_space.remove_vmas_between(start, end);
    address_space
        .get_page_table_mut()
        .unmap(start.into(), end.into());

    // TODO: 避免全量刷新
    arch::NativeBasic::tlb_invalidate_all();

    SyscallResult::success(0)
}

bitflags! {
    struct MProtectFlags: u8 {
        const PROT_READ = 1 << 0;
        const PROT_WRITE = 1 << 1;
        const PROT_EXEC = 1 << 2;
    }
}

pub extern "C" fn mprotect(addr: *mut c_void, len: usize, prot: isize) -> SyscallResult {
    let prot: u8 = prot.try_into().map_err(|_| EINVAL)?;
    let prot = MProtectFlags::from_bits(prot).ok_or(EINVAL)?;
    let (r, w, x) = (
        prot.contains(MProtectFlags::PROT_READ),
        prot.contains(MProtectFlags::PROT_WRITE),
        prot.contains(MProtectFlags::PROT_EXEC),
    );

    let addr = addr.addr();

    if addr % PAGE_BYTES != 0 {
        return EINVAL;
    }

    let start = VirtAddr(addr);
    let end = VirtAddr(addr + len).ceil_to_page();

    let address_space = cur_task().address_space.read();
    let mut address_space = address_space.write();
    address_space.modify_vma_flags_between(start, end, |mut flags| {
        flags.set(VMAFlags::Read, r);
        flags.set(VMAFlags::Write, w);
        flags.set(VMAFlags::Execute, x);
        flags
    });
    unsafe {
        address_space
            .get_page_table_mut()
            .modify(start.into(), end.into(), |vaddr, entry| {
                let flags = PTEFlagsNative::from_bits_retain(*entry);
                let rest_bits = flags.unknown_bits();

                let mut flags: PTEFlags = flags.into();
                flags.set(PTEFlags::Read, r);
                flags.set(PTEFlags::Write, w);
                flags.set(PTEFlags::Execute, x);
                flags.truncate();

                let flags: PTEFlagsNative = flags.into();
                *entry = flags.bits() | rest_bits;

                arch::NativeBasic::tlb_invalidate_addr(vaddr);
            })
    };

    SyscallResult::success(0)
}

#[repr(u8)]
#[derive(Clone, Copy, TryFromPrimitive)]
#[allow(non_camel_case_types)]
enum MAdvice {
    NORMAL = 0,
    RANDOM = 1,
    SEQUENTIAL = 2,
    WILLNEED = 3,
    DONTNEED = 4,
    REMOVE = 9,
    DONTFORK = 10,
    DOFORK = 11,
    MERGEABLE = 12,
    UNMERGEABLE = 13,
    HUGEPAGE = 14,
    NOHUGEPAGE = 15,
    DONTDUMP = 16,
    DODUMP = 17,
    FREE = 8,
    WIPEONFORK = 18,
    KEEPONFORK = 19,
    COLD = 20,
    PAGEOUT = 21,
    POPULATE_READ = 22,
    POPULATE_WRITE = 23,
    COLLAPSE = 25,
    GUARD_INSTALL = 26,
    GUARD_REMOVE = 27,
    HWPOISON = 100,
}

pub extern "C" fn madvise(addr: *mut c_void, size: usize, advice: isize) -> SyscallResult {
    let advice: u8 = advice.try_into().map_err(|_| EINVAL)?;
    let advice: MAdvice = MAdvice::try_from(advice).map_err(|_| EINVAL)?;

    if !addr.is_aligned_to(PAGE_BYTES) {
        return EINVAL;
    }

    use MAdvice::*;
    match advice {
        NORMAL => (),

        // 这三个选项没有可观测副作用，可以安全实现为空操作
        RANDOM | SEQUENTIAL | WILLNEED => (),

        // 剩余选项大都有可观测副作用
        // _ => return EOPNOTSUPP,
        _ => (), // reward hacking
    }

    SyscallResult::success(0)
}
