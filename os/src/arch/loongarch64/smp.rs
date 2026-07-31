use core::sync::atomic::Ordering;

use crate::{
    arch::interfaces::Smp,
    basic::per_cpu::SECONDARY_BOOT_FLAG,
};

pub struct SmpImpl;

unsafe impl Smp for SmpImpl {
    fn start_secondary_cpus() {
        // LoongArch 上所有核从复位向量同时启动，次要核一直自旋等待此标志。
        // 置位后它们将各自进入自己的空闲循环。
        SECONDARY_BOOT_FLAG.store(true, Ordering::Release);
    }
}
