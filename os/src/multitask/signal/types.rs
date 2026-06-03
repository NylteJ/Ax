use num_enum::TryFromPrimitive;

use bitflags::bitflags;

macro_rules! make_enums {
    ($id_name:ident, $mask_name:ident : $first:ident, $($idents:ident),* $(,)?) => {
        make_enums! { @accum $id_name, $mask_name, 1, { $first = 0, }, {}, $($idents),* }
    };

    (@accum $id_name:ident, $mask_name:ident, $offset:expr, { $($output1:tt)* }, { $($output2:tt)* }, ) => {
        #[repr(u8)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
        #[allow(non_camel_case_types)]
        pub enum $id_name {
            $($output1)*
        }

        bitflags! {
            #[repr(transparent)]
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct $mask_name: u64 {
                $($output2)*
            }
        }
    };

    (@accum $id_name:ident, $mask_name:ident, $offset:expr, { $($output1:tt)* }, { $($output2:tt)* }, $first:ident $(, $rest:ident)*) => {
        make_enums! { @accum $id_name, $mask_name, $offset + 1, { $($output1)* $first = $offset, }, { $($output2)* const $first = 1 << ($offset - 1); }, $($rest),* }
    };
}

make_enums! {
    SignalId, SignalMask:
    SIGINVAL,
    SIGHUP,
    SIGINT,
    SIGQUIT,
    SIGILL,
    SIGTRAP,
    SIGABRT,
    SIGBUS,
    SIGFPE,
    SIGKILL,
    SIGUSR1,
    SIGSEGV,
    SIGUSR2,
    SIGPIPE,
    SIGALRM,
    SIGTERM,
    SIGSTKFLT,
    SIGCHLD,
    SIGCONT,
    SIGSTOP,
    SIGTSTP,
    SIGTTIN,
    SIGTTOU,
    SIGURG,
    SIGXCPU,
    SIGXFSZ,
    SIGVTALRM,
    SIGPROF,
    SIGWINCH,
    SIGIO,
    SIGPWR,
    SIGSYS,
    SIGTIMER,
    SIGCANCEL,
    SIGSYNCCALL,
    SIGRT_3,
    SIGRT_4,
    SIGRT_5,
    SIGRT_6,
    SIGRT_7,
    SIGRT_8,
    SIGRT_9,
    SIGRT_10,
    SIGRT_11,
    SIGRT_12,
    SIGRT_13,
    SIGRT_14,
    SIGRT_15,
    SIGRT_16,
    SIGRT_17,
    SIGRT_18,
    SIGRT_19,
    SIGRT_20,
    SIGRT_21,
    SIGRT_22,
    SIGRT_23,
    SIGRT_24,
    SIGRT_25,
    SIGRT_26,
    SIGRT_27,
    SIGRT_28,
    SIGRT_29,
    SIGRT_30,
    SIGRT_31,
    SIGRTMAX,
}

pub type SignalIdUnderlying = u8;

impl SignalId {
    pub const NORMAL_MAX: Self = Self::SIGSYS;
    pub const MAX: Self = Self::SIGRTMAX;

    pub fn value(self) -> SignalIdUnderlying {
        self as SignalIdUnderlying
    }

    pub fn is_realtime(&self) -> bool {
        self.value() >= 32
    }

    /// 返回此信号是否是特殊信号（即无法修改或查询处理方式，也无法捕获或忽略的信号）
    pub fn is_special(&self) -> bool {
        matches!(self, Self::SIGKILL | Self::SIGSTOP)
    }

    pub fn default_behavior(&self) -> SignalBehavior {
        use SignalBehavior::*;
        use SignalId::*;
        match *self {
            SIGCHLD | SIGURG | SIGWINCH => Ignored,

            SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => Stop,

            SIGABRT | SIGBUS | SIGFPE | SIGILL | SIGQUIT | SIGSEGV | SIGSYS | SIGTRAP | SIGXCPU
            | SIGXFSZ => TerminateAndDumpCore,

            SIGCONT => ContinueIfStopped,

            _ => Terminate,
        }
    }
}

pub enum SignalBehavior {
    /// 终止进程
    Terminate,
    /// 忽略信号
    Ignored,
    /// 终止进程并转储核心
    TerminateAndDumpCore,
    /// 停止进程
    Stop,
    /// 如果进程当前处于停止状态则继续执行进程
    ContinueIfStopped,
}
