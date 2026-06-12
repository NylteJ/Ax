use core::sync::atomic::{AtomicIsize, AtomicU64, Ordering};

use num_enum::TryFromPrimitive;

use crate::{
    basic::timer::{CLOCK_FREQ, cpu_tick_to_us, get_cpu_tick, us_to_cpu_tick},
    memory::uaccess::{copy_one_from_user, copy_one_to_user},
    multitask::current_task,
    syscall::{SyscallResult, common::TimeSpec},
};

use super::err_codes::*;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[allow(non_camel_case_types)]
pub enum ClockId {
    REALTIME = 0,
    MONOTONIC = 1,
    PROCESS_CPUTIME_ID = 2,
    THREAD_CPUTIME_ID = 3,
    MONOTONIC_RAW = 4,
    REALTIME_COARSE = 5,
    MONOTONIC_COARSE = 6,
    BOOTTIME = 7,
}

fn get_clock_id(clockid: usize) -> Result<ClockId, SyscallResult> {
    let clockid: u8 = clockid.try_into().map_err(|_| EINVAL)?;
    let clockid = ClockId::try_from(clockid).map_err(|_| EINVAL)?;
    Ok(clockid)
}

static RT_CLOCK_OFFSET_TICKS: AtomicIsize = AtomicIsize::new(0);

pub extern "C" fn clock_getres(clockid: usize, res: *mut TimeSpec) -> SyscallResult {
    let _ = get_clock_id(clockid)?; // 目前所有时钟的分辨率都一样，因此只检测 id 是否合法即可

    if !res.is_null() {
        let ns = 1_000_000_000 / *CLOCK_FREQ;
        let ns = ns.max(1000) as u64; // 我们内部主要使用 us (1000 ns)

        copy_one_to_user(res, &TimeSpec::from_ns(ns))?;
    }

    SyscallResult::success(0)
}

pub extern "C" fn clock_gettime(clockid: usize, tp: *mut TimeSpec) -> SyscallResult {
    let clockid = get_clock_id(clockid)?;

    match clockid {
        ClockId::REALTIME | ClockId::REALTIME_COARSE => {
            let tick = get_cpu_tick();
            let tick = tick.saturating_add_signed(RT_CLOCK_OFFSET_TICKS.load(Ordering::Relaxed));
            let us = cpu_tick_to_us(tick) as u64;
            copy_one_to_user(tp, &TimeSpec::from_us(us))?;
        }

        ClockId::MONOTONIC
        | ClockId::MONOTONIC_RAW
        | ClockId::MONOTONIC_COARSE
        | ClockId::BOOTTIME => {
            let tick = get_cpu_tick();
            let us = cpu_tick_to_us(tick) as u64;
            copy_one_to_user(tp, &TimeSpec::from_us(us))?;
        }

        ClockId::THREAD_CPUTIME_ID => {
            let ticks = {
                let cur_task = unsafe { current_task().as_ref_unchecked() };
                let time_stats = cur_task.time_stats.lock();

                time_stats.kernel_time + time_stats.user_time
            };
            let us = cpu_tick_to_us(ticks) as u64;
            copy_one_to_user(tp, &TimeSpec::from_us(us))?;
        }

        ClockId::PROCESS_CPUTIME_ID => {
            debug_assert!(false);
            return EOPNOTSUPP;
        }
    }

    SyscallResult::success(0)
}

pub extern "C" fn clock_settime(clockid: usize, tp: *const TimeSpec) -> SyscallResult {
    let clockid = get_clock_id(clockid)?;

    match clockid {
        ClockId::REALTIME => {
            let tick = get_cpu_tick();
            let us = cpu_tick_to_us(tick) as u64;

            let tp = copy_one_from_user(tp)?;
            tp.check_regular()?;

            let now_should_be_us =
                tp.tv_sec.cast_unsigned() * 1_000_000 + tp.tv_nsec.cast_unsigned() / 1000;

            let now_should_be_tick = us_to_cpu_tick(now_should_be_us as usize);

            let offset = now_should_be_tick.cast_signed() - tick.cast_signed();
            RT_CLOCK_OFFSET_TICKS.store(offset, Ordering::Relaxed);
        }

        ClockId::REALTIME_COARSE => return EINVAL, // 其本身不支持设置，和 REALTIME 同步

        ClockId::MONOTONIC | ClockId::MONOTONIC_RAW | ClockId::MONOTONIC_COARSE => return EINVAL,
        ClockId::PROCESS_CPUTIME_ID | ClockId::THREAD_CPUTIME_ID => return EINVAL,
        ClockId::BOOTTIME => return EINVAL,
    }

    SyscallResult::success(0)
}
