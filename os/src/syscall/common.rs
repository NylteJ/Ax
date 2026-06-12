use core::ffi::c_long;

use crate::{
    memory::uaccess::copy_one_from_user,
    syscall::{SyscallResult, err_codes::EINVAL},
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TimeSpec {
    pub tv_sec: i64,
    pub tv_nsec: c_long,
}

impl TimeSpec {
    pub fn from_ns(ns: u64) -> Self {
        Self {
            tv_sec: (ns / 1_000_000_000) as c_long,
            tv_nsec: (ns % 1_000_000_000) as c_long,
        }
    }
    pub fn from_ms(ms: u64) -> Self {
        Self::from_ns(ms * 1_000_000)
    }
    pub fn from_us(us: u64) -> Self {
        Self::from_ns(us * 1_000)
    }
    pub fn from_sec(sec: u64) -> Self {
        Self {
            tv_sec: sec as c_long,
            tv_nsec: 0,
        }
    }

    pub fn check_regular(&self) -> Result<(), SyscallResult> {
        if self.tv_sec < 0 || self.tv_nsec < 0 || self.tv_nsec >= 1_000_000_000 {
            Err(EINVAL)
        } else {
            Ok(())
        }
    }
}

pub fn get_timeout_us(timeout: *const TimeSpec) -> Result<Option<u64>, SyscallResult> {
    Ok(if timeout.is_null() {
        None
    } else {
        let TimeSpec { tv_sec, tv_nsec } =
            copy_one_from_user(timeout).map_err(|e| -> SyscallResult { Err(e)? })?;

        let us = tv_sec
            .checked_mul(1_000_000)
            .and_then(|s| s.checked_add(tv_nsec / 1000));

        if us.is_some_and(|value| value < 0) {
            return Err(EINVAL);
        }

        us.map(|signed| signed.cast_unsigned())
    })
}
