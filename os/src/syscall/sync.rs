use core::ptr::NonNull;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;

use alloc::vec::Vec;
use bitflags::bitflags;
use num_enum::TryFromPrimitive;

use crate::basic::timer::get_cpu_tick;
use crate::basic::timer::us_to_cpu_tick;
use crate::filesystem::vfs::INodeArc;
use crate::memory::PAGE_BYTES;
use crate::memory::address::PhysAddr;
use crate::memory::address::PhysPageNum;
use crate::memory::address::VirtAddr;
use crate::memory::address::VirtPageNum;
use crate::memory::uaccess::copy_one_from_user;
use crate::memory::virtual_memory_area::VMAType;
use crate::multitask::current_task;
use crate::multitask::futex::FutexKey;
use crate::multitask::futex::FutexMask;
use crate::multitask::futex::futex_lock;
use crate::multitask::futex::futex_requeue;
use crate::syscall::SyscallResult;

use crate::syscall::common::TimeSpec;
use crate::syscall::common::get_timeout_us;
use crate::syscall::err_codes::*;
use crate::trap::page_fault_type::PageFaultType;

#[repr(u8)]
#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[allow(non_camel_case_types)]
enum FutexOp {
    /// 此操作检测由地址 `uaddr` 指向的 futex 字的值是否仍包含期望值 `val`，如果是，则休眠等待对该 futex 字执行 `FUTEX_WAKE(2const)` 操作。
    ///
    /// 对 futex 字值的加载是原子内存访问（即使用相应架构的原子机器指令）。
    /// 此加载、与期望值的比较以及开始休眠，这三个操作是原子地执行的，并且相对于同一 futex 字上的其他 futex 操作是完全有序的。
    ///
    /// 如果线程开始休眠，它被视为该 futex 字上的一个等待者。如果 futex 值与 `val` 不匹配，则调用立即失败，并返回错误 `EAGAIN`。
    ///
    /// 与期望值比较的目的是防止丢失唤醒。
    /// 如果另一个线程在调用线程基于先前的值决定阻塞之后更改了 futex 字的值，并且如果该另一个线程在更改值之后、在此 `FUTEX_WAIT` 操作之前执行了 `FUTEX_WAKE(2const)` 操作（或类似的唤醒操作），
    /// 那么调用线程将观察到值的更改，并且不会开始休眠。
    ///
    /// 如果 `timeout` 不为 `NULL`，则它指向的结构指定了等待的超时时间。（此时间间隔将向上舍入到系统时钟粒度，并保证不会提前到期。）如果 `timeout` 为 `NULL`，则调用将无限期阻塞。
    ///
    /// ## 返回值
    /// 出现错误时，返回 -1，并设置 `errno` 以指示错误。
    ///
    /// 成功时，如果调用者被唤醒，`FUTEX_WAIT` 返回 0。
    /// 请注意，唤醒也可能由不相关代码中常见的 futex 使用模式引起，这些代码恰好之前使用过该 futex 字的内存位置（例如，典型的基于 futex 的 Pthreads 互斥锁实现在某些条件下可能导致此情况）。
    /// 因此，调用者应始终保守地假设返回值 0 可能意味着虚假唤醒，并使用 futex 字的值（即用户空间同步方案）来决定是继续阻塞还是不再阻塞。
    ///
    /// ## 错误
    /// 参见 `futex(2)`。
    ///
    /// `EAGAIN` 或 `EWOULDBLOCK`
    /// 调用时，`uaddr` 指向的值不等于期望值 `val`。
    ///
    /// `EFAULT` `timeout` 未指向有效的用户空间地址。
    ///
    /// `EINTR` 操作被信号中断（参见 `signal(7)`）。在 Linux 2.6.22 之前，虚假唤醒也可能返回此错误；自 Linux 2.6.22 起，此情况不再发生。
    ///
    /// `EINVAL` 提供的 `timeout` 参数无效（`tv_sec` 小于零，或 `tv_nsec` 不小于 1,000,000,000）。
    ///
    /// `ETIMEDOUT`
    /// 超时在操作完成之前已到期。
    WAIT = 0,
    /// 此操作最多唤醒在地址 uaddr 处的 futex 字上等待（例如，在 FUTEX_WAIT(2const) 内）的 val 个等待者。
    ///
    /// 最常见的情况是，val 指定为 1（唤醒单个等待者）或 INT_MAX（唤醒所有等待者）。
    ///
    /// 对于唤醒哪些等待者，不作任何保证（例如，不保证具有较高调度优先级的等待者会优先于具有较低优先级的等待者被唤醒）。
    /// ## 返回值
    /// 出错时，返回 -1，并设置 errno 以指示错误。
    ///
    /// 成功时，FUTEX_WAKE 返回被唤醒的等待者数量。
    /// ## 错误
    /// 参见 futex(2)。
    ///
    /// `EINVAL` 内核检测到 uaddr 处的用户空间状态与内核状态之间存在不一致——即，内核检测到一个正在 uaddr 上以 FUTEX_LOCK_PI(2const) 或 FUTEX_LOCK_PI2(2const) 方式等待的等待者。
    WAKE = 1,
    /// 唤醒部分等待者，并将其他等待者重新排队。
    ///
    /// ```c
    ///       long syscall(SYS_futex, uint32_t *uaddr, FUTEX_REQUEUE,
    ///                    uint32_t val, uint32_t val2, uint32_t *uaddr2);
    /// ```
    /// 此操作执行的任务与 FUTEX_CMP_REQUEUE(2const) 相同，区别在于它不比较 futex 字。
    ///
    /// ## 返回值
    /// 出错时，返回 -1，并设置 errno 以指示错误。
    ///
    /// 成功时，FUTEX_REQUEUE 返回被唤醒的等待者数量。
    ///
    /// ## 错误
    /// 参见 futex(2)。
    ///
    /// `EFAULT` uaddr2 未指向有效的用户空间地址。
    ///
    /// `EINVAL` uaddr2 未指向有效对象——即，该地址不是四字节对齐的。
    ///
    /// `EINVAL` 内核检测到 uaddr 处的用户空间状态与内核状态不一致——即，它检测到一个等待者正在 uaddr 上等待 FUTEX_LOCK_PI(2const) 或 FUTEX_LOCK_PI2(2const)。
    REQUEUE = 3,
    /// 比较一个 futex，唤醒一些等待者，并将其他等待者重新排队。
    ///
    /// ```c
    ///        long syscall(SYS_futex, uint32_t *uaddr, FUTEX_CMP_REQUEUE,
    ///                     uint32_t val, uint32_t val2, uint32_t *uaddr2,
    ///                     uint32_t val3);
    /// ```
    ///
    /// 此操作首先检查位置 uaddr 是否仍包含值 val3。若非如此，操作将以错误 EAGAIN 失败。
    /// 否则，操作最多唤醒 val 个在 uaddr 处的 futex 上等待的等待者。
    /// 若等待者多于 val 个，则剩余的等待者将从 uaddr 处的源 futex 的等待队列中移除，并添加到 uaddr2 处的目标 futex 的等待队列中。
    /// val2 参数指定了被重新排队到 uaddr2 处的 futex 的等待者数量的上限。
    ///
    /// 从 uaddr 的加载是一次原子内存访问（即使用相应架构的原子机器指令）。
    /// 此加载、与 val3 的比较，以及任何等待者的重新排队操作，均以原子方式执行，且相对于同一 futex 字上的其他操作，是完全有序的。
    ///
    /// 为 val 指定的典型值为 0 或 1。（将 val 指定为 INT_MAX 并不有用，因为这将使 FUTEX_CMP_REQUEUE 操作等效于 FUTEX_WAKE(2const)。）
    /// 通过 val2 指定的限制值通常为 1 或 INT_MAX。（将 val2 指定为 0并无用处，因为这将使 FUTEX_CMP_REQUEUE 操作等效于 FUTEX_WAKE(2const)。）
    ///
    /// 添加 FUTEX_CMP_REQUEUE 操作是为了替代先前的 FUTEX_REQUEUE(2const)。
    /// 区别在于，可以利用对 uaddr 处值的检查来确保重新排队仅在特定条件下发生，这在某些用例中可避免竞争条件。
    ///
    /// 当使用 FUTEX_WAKE(2const) 可能导致所有被唤醒的等待者都需获取另一个 futex 的情况下，FUTEX_REQUEUE(2const) 和 FUTEX_CMP_REQUEUE 均可用来避免可能发生的“惊群”唤醒。
    /// 考虑以下场景，其中多个等待者线程在 B 上等待，B 是一个使用 futex 实现的等待队列：
    /// ```c
    /// lock(A)
    /// while (!check_value(V)) {
    ///     unlock(A);
    ///     block_on(B);
    ///     lock(A);
    /// };
    /// unlock(A);
    /// ```
    /// 若某个唤醒者线程使用了 FUTEX_WAKE(2const)，那么所有在 B 上等待的等待者都将被唤醒，且它们都将尝试获取锁 A。
    /// 然而，以这种方式唤醒所有线程并无意义，因为除一个线程外，其余线程都将立即再次阻塞在锁 A 上。
    /// 相比之下，重新排队操作仅唤醒一个等待者并将其他等待者移至锁 A，而当被唤醒的等待者解锁 A 时，下一个等待者便可继续运行。
    ///
    /// ## 返回值
    /// 出错时，返回 -1，并设置 errno 以指示错误。
    ///
    /// 成功时，FUTEX_CMP_REQUEUE 返回被唤醒或被重新排队到 uaddr2 处 futex 字的 futex 的等待者总数。
    /// 若此值大于 val，则差值即为被重新排队到 uaddr2 处 futex 字的 futex 的等待者数量。
    ///
    /// ## 错误
    /// 参见 futex(2)。
    ///
    /// `EAGAIN` uaddr 指向的值不等于期望值 val3。
    ///
    /// `EFAULT` uaddr2 未指向有效的用户空间地址。
    ///
    /// `EINVAL` uaddr2 未指向有效对象——即地址未按四字节对齐。
    ///
    /// `EINVAL` 内核检测到 uaddr 处的用户空间状态与内核状态不一致——即内核在 uaddr 上检测到一个正在等待 FUTEX_LOCK_PI(2const) 或 FUTEX_LOCK_PI2(2const) 的等待者。
    CMP_REQUEUE = 4,

    /// 选择性的 futex 等待
    ///
    /// ```c
    /// long syscall(SYS_futex, uint32_t *uaddr, FUTEX_WAIT_BITSET, uint32_t val,
    /// const struct timespec *timeout, NULL,
    /// uint32_t val3);
    /// ```
    ///
    /// ## 描述
    /// 此操作类似于 FUTEX_WAIT(2const)，区别在于使用 val3 向内核提供一个 32 位的位掩码。
    /// 该位掩码（其中至少有一个位必须被设置）被存储于等待者的内核内部状态中。
    /// 更多细节参见 FUTEX_WAKE_BITSET 的描述。
    ///
    /// 如果 timeout 非 NULL，它所指向的结构体指定了等待操作的绝对超时时间。
    /// 如果 timeout 为 NULL，该操作可能会无限期阻塞。
    ///
    /// FUTEX_WAIT_BITSET 和 FUTEX_WAKE_BITSET 的作用是允许在阻塞于同一个 futex 上的多个等待者之间进行选择性唤醒。
    /// 但请注意，根据使用场景，在 futex 上使用这种位掩码复用特性可能不如直接使用多个 futex 高效，因为使用位掩码复用需要内核检查 futex 上的所有等待者，包括那些对唤醒不感兴趣的等待者（即它们的“等待”位掩码中没有设置相关的位）。
    ///
    /// 常量 FUTEX_BITSET_MATCH_ANY（对应位掩码中所有 32 位都被设置）可以用作 FUTEX_WAIT_BITSET 和 FUTEX_WAKE_BITSET 的 val3 参数。
    /// 除了超时参数的处理方式不同之外，FUTEX_WAIT(2const) 操作等价于将 val3 指定为 FUTEX_BITSET_MATCH_ANY 的 FUTEX_WAIT_BITSET；也就是说，允许被任何唤醒者唤醒。
    /// FUTEX_WAKE(2const) 操作等价于将 val3 指定为 FUTEX_BITSET_MATCH_ANY 的 FUTEX_WAKE_BITSET；也就是说，唤醒任意等待者。
    ///
    /// ## 返回值
    /// 出错时，返回 -1，并设置 errno 以指示错误类型。
    ///
    /// 成功时的返回值取决于操作，如下列表所述：
    ///
    /// 如果调用者被唤醒，则返回 0。关于如何在实际中正确解释此返回值，参见 FUTEX_WAIT(2const)。
    ///
    /// ## 错误
    /// 参见 futex(2)。
    ///
    /// `EAGAIN` 或 `EWOULDBLOCK`
    /// (FUTEX_WAIT_BITSET) 调用时，uaddr 所指向的值不等于期望值 val。
    ///
    /// `EFAULT` timeout 未指向有效的用户空间地址。
    ///
    /// `EINTR` FUTEX_WAIT_BITSET 操作被信号中断（参见 signal(7)）。在 Linux 2.6.22 之前，此错误也可能因虚假唤醒而返回；自 Linux 2.6.22 起，这种情况不再发生。
    ///
    /// `EINVAL` 提供的 timeout 参数无效（tv_sec 小于零，或 tv_nsec 不小于 1,000,000,000）。
    ///
    /// `EINVAL` uaddr2 未指向有效对象——即该地址未按四字节对齐。
    ///
    /// `EINVAL` val3 中提供的位掩码为零。
    ///
    /// `EINVAL` (FUTEX_WAKE_BITSET) 内核检测到 uaddr 处的用户空间状态与内核状态之间存在不一致——即检测到有等待者在 uaddr 上等待于 FUTEX_LOCK_PI(2const) 或 FUTEX_LOCK_PI2(2const) 中。
    ///
    /// `ETIMEDOUT` 操作完成前超时时间已到。
    WAIT_BITSET = 9,
    /// 选择性的 futex 唤醒
    ///
    /// ```c
    /// long syscall(SYS_futex, uint32_t *uaddr, FUTEX_WAKE_BITSET, uint32_t val,
    /// NULL, NULL,
    /// uint32_t val3);
    /// ```
    ///
    /// ## 描述
    /// 此操作与 FUTEX_WAKE(2const) 相同，区别在于使用 val3 参数向内核提供一个 32 位的位掩码。
    /// 该位掩码（其中至少有一个位必须被设置）用于选择应唤醒哪些等待者。
    /// 选择是通过对“唤醒”位掩码（即 val3 中的值）与存储在等待者内核内部状态的位掩码（使用 FUTEX_WAIT_BITSET 设置的“等待”位掩码）进行按位与操作来完成的。
    /// 所有与该按位与操作结果非零的等待者都会被唤醒；其余等待者则保持睡眠状态。
    ///
    /// FUTEX_WAIT_BITSET 和 FUTEX_WAKE_BITSET 的作用是允许在阻塞于同一个 futex 上的多个等待者之间进行选择性唤醒。
    /// 但请注意，根据使用场景，在 futex 上使用这种位掩码复用特性可能不如直接使用多个 futex 高效，因为使用位掩码复用需要内核检查 futex 上的所有等待者，包括那些对唤醒不感兴趣的等待者（即它们的“等待”位掩码中没有设置相关的位）。
    ///
    /// 常量 FUTEX_BITSET_MATCH_ANY（对应位掩码中所有 32 位都被设置）可以用作 FUTEX_WAIT_BITSET 和 FUTEX_WAKE_BITSET 的 val3 参数。
    /// 除了超时参数的处理方式不同之外，FUTEX_WAIT(2const) 操作等价于将 val3 指定为 FUTEX_BITSET_MATCH_ANY 的 FUTEX_WAIT_BITSET；也就是说，允许被任何唤醒者唤醒。
    /// FUTEX_WAKE(2const) 操作等价于将 val3 指定为 FUTEX_BITSET_MATCH_ANY 的 FUTEX_WAKE_BITSET；也就是说，唤醒任意等待者。
    ///
    /// ## 返回值
    /// 出错时，返回 -1，并设置 errno 以指示错误类型。
    ///
    /// 成功时的返回值取决于操作，如下列表所述：
    ///
    /// 返回被唤醒的等待者数量。
    ///
    /// ## 错误
    /// 参见 futex(2)。
    ///
    /// `EAGAIN` 或 `EWOULDBLOCK`
    /// (FUTEX_WAIT_BITSET) 调用时，uaddr 所指向的值不等于期望值 val。
    ///
    /// `EFAULT` timeout 未指向有效的用户空间地址。
    ///
    /// `EINTR` FUTEX_WAIT_BITSET 操作被信号中断（参见 signal(7)）。在 Linux 2.6.22 之前，此错误也可能因虚假唤醒而返回；自 Linux 2.6.22 起，这种情况不再发生。
    ///
    /// `EINVAL` 提供的 timeout 参数无效（tv_sec 小于零，或 tv_nsec 不小于 1,000,000,000）。
    ///
    /// `EINVAL` uaddr2 未指向有效对象——即该地址未按四字节对齐。
    ///
    /// `EINVAL` val3 中提供的位掩码为零。
    ///
    /// `EINVAL` (FUTEX_WAKE_BITSET) 内核检测到 uaddr 处的用户空间状态与内核状态之间存在不一致——即检测到有等待者在 uaddr 上等待于 FUTEX_LOCK_PI(2const) 或 FUTEX_LOCK_PI2(2const) 中。
    ///
    /// `ETIMEDOUT` 操作完成前超时时间已到。
    WAKE_BITSET = 10,

    // TODO
    FD = 2,
    WAKE_OP = 5,
    // TODO: PI Mutex
}

bitflags! {
    struct FutexOpFlags: u16 {
        /// 此选项位可用于所有 futex 操作。它告知内核该 futex 是进程私有的，不与其他进程共享（即，它仅用于同一进程的线程间同步）。
        /// 这允许内核进行一些额外的性能优化。
        const PRIVATE = 1 << 7;
        /// 此选项位仅能用于 FUTEX_WAIT_BITSET(2const)、FUTEX_WAIT_REQUEUE_PI(2const)、（自 Linux 4.5 起）FUTEX_WAIT(2const) 以及（自 Linux 5.14 起）FUTEX_LOCK_PI2(2const) 操作。
        ///
        /// 如果设置了此选项，内核将根据 CLOCK_REALTIME 时钟来测量超时。
        ///
        /// 如果未设置此选项，内核将根据 CLOCK_MONOTONIC 时钟来测量超时。
        const CLOCK_REALTIME = 1 << 8;
    }
}

pub extern "C" fn futex(
    uaddr: *mut u32,
    op: isize,
    val: usize,
    val2: usize,
    val3: usize,
    val4: usize,
) -> SyscallResult {
    let op: u16 = op.try_into().map_err(|_| EINVAL)?;
    let flags = FutexOpFlags::from_bits_truncate(op);
    let op: u8 = (op & !FutexOpFlags::all().bits())
        .try_into()
        .map_err(|_| EINVAL)?;
    let op: FutexOp = op.try_into().map_err(|_| EINVAL)?;

    if flags.contains(FutexOpFlags::CLOCK_REALTIME)
        && !matches!(op, FutexOp::WAIT_BITSET | FutexOp::WAIT)
    {
        return EINVAL;
    }

    let is_private = flags.contains(FutexOpFlags::PRIVATE);
    let futex_key = get_futex_key(uaddr, is_private)?;
    let mut futex_guard = futex_lock(&futex_key);
    // 另外注意，futex 本身不建立任何全局内存序，它只保证对 word 的访问原子且相对同一 word 上的其他操作完全有序

    match op {
        FutexOp::WAIT | FutexOp::WAIT_BITSET => {
            let val = val as u32;
            let val3 = val4 as u32;

            if futex_guard.relaxed_atom_load(uaddr)? != val {
                log::trace!("FUTEX 提前返回");
                return EAGAIN;
            }

            let mask = if let FutexOp::WAIT_BITSET = op {
                val3
            } else {
                FutexMask::MAX
            };

            let timeout = val2 as *const TimeSpec;
            let timeout_us = get_timeout_us(timeout)?;

            let ddl_tick = if let FutexOp::WAIT_BITSET = op {
                // 此时超时是绝对时间
                timeout_us.map(|us| us_to_cpu_tick(us as usize))
            } else {
                timeout_us.map(|us| get_cpu_tick() + us_to_cpu_tick(us as usize))
            };

            futex_guard.wait(ddl_tick, mask)?;
            log::trace!("FUTEX 等待到了");

            SyscallResult::success(0)
        }
        FutexOp::WAKE | FutexOp::WAKE_BITSET => {
            let val3 = val4 as u32;

            let mask = if let FutexOp::WAKE_BITSET = op {
                val3
            } else {
                FutexMask::MAX
            };

            let count = futex_guard.wake(val, mask);

            log::trace!("FUTEX 唤醒了 {count} / {val} 个等待者");

            SyscallResult::success(count)
        }

        FutexOp::REQUEUE | FutexOp::CMP_REQUEUE => {
            let uaddr2 = val3 as *mut u32;
            let val3 = val4 as u32;

            if let FutexOp::CMP_REQUEUE = op
                && futex_guard.relaxed_atom_load(uaddr)? != val3
            {
                log::trace!("FUTEX_CMP_REQUEUE 提前返回");
                return EAGAIN;
            }

            let woken = futex_guard.wake(val, FutexMask::MAX);
            log::trace!("FUTEX_REQUEUE 唤醒了 {woken} / {val} 个等待者");

            if woken < val {
                // 所有等待者都已被唤醒但数量还是不够
                // 此时没有必要再进行后续操作，因为一定无法转移哪怕一个等待者，退化为 WAKE
                return SyscallResult::success(woken);
            }

            let futex_key_2 = get_futex_key(uaddr2, is_private)?;
            let requeued = futex_requeue(futex_guard, &futex_key_2, val2);

            log::trace!(
                "FUTEX_REQUEUE 从源 futex 字 {futex_key:?} 提取了 {requeued} 个等待者到 {futex_key_2:?}"
            );
            SyscallResult::success(woken + requeued)
        }

        _ => todo!("{op:?}"),
    }
}

pub fn get_futex_key(uaddr: *mut u32, private: bool) -> Result<FutexKey, SyscallResult> {
    let cur_task = unsafe { current_task().as_ref_unchecked() };
    let cur_addr_space = &cur_task.address_space;

    if !uaddr.is_aligned_to(4) {
        return Err(EINVAL); // futex 强制要求地址按 4 字节对齐
    }

    Ok(if private {
        let uaddr = NonNull::new(uaddr.cast()).ok_or(EFAULT)?;
        FutexKey::Private {
            addr_space: cur_addr_space.clone(),
            uaddr,
        }
    } else {
        let inpage_off = uaddr.addr() % PAGE_BYTES;
        let uvaddr = VirtAddr::from_ptr(uaddr);

        let addr_space = cur_addr_space.read();

        let vma = addr_space.find_vma(uvaddr).ok_or(EFAULT)?;

        match &vma.vma_type {
            VMAType::File { file, page_index } => FutexKey::SharedFile {
                inode: unsafe { INodeArc::clone_from_value(file.get_inode()) },
                offset: *page_index * PAGE_BYTES as u64 + inpage_off as u64,
            },
            VMAType::Anonymous => {
                let existing_pte = addr_space.get_page_table().at::<0>(uvaddr.into());

                let pte = if let Some(pte) = existing_pte {
                    *pte
                } else {
                    drop(addr_space);

                    let mut addr_space = cur_addr_space.write();
                    addr_space
                        .handle_page_fault(uvaddr, PageFaultType::Read)
                        .map_err(|_| EFAULT)?;
                    *addr_space.get_page_table().at::<0>(uvaddr.into()).unwrap()
                };

                let phys_page = PhysPageNum::from_pte(&pte);

                FutexKey::SharedAnon {
                    addr: PhysAddr(PhysAddr::from(phys_page).0 + inpage_off),
                }
            }
        }
    })
}
