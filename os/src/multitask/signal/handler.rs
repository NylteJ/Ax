use core::ptr::NonNull;

use crate::{
    multitask::signal::{SignalBehavior, SignalId},
    syscall::signal::SignalAction,
};

const HANDLERS_COUNT: usize = SignalId::MAX as usize + 1;

#[derive(Debug, Clone)]
pub struct SignalHandler {
    pub handlers: [SignalAction; HANDLERS_COUNT],
}

impl SignalHandler {
    pub fn new() -> Self {
        Self {
            handlers: [SignalAction::default(); HANDLERS_COUNT],
        }
    }

    #[must_use]
    pub fn handle(&self, signal: SignalId) -> SignalBehavior {
        let handler_type = self.handlers[signal as usize].get_handler_type();

        let (detailed, fptr) = match handler_type {
            SignalHandlerType::Default => return signal.default_behavior(),
            SignalHandlerType::Ignored => return SignalBehavior::Ignored,
            SignalHandlerType::UserDefinedSimple(fptr) => (false, fptr),
            SignalHandlerType::UserDefinedDetailed(fptr) => (true, fptr),
        };

        log::error!("TODO: 调用用户自定义的处理函数（{fptr:?}）");
        debug_assert!(false);

        // 信号已经由用户处理完毕，内核方可以忽略它了
        SignalBehavior::Ignored
    }
}

pub enum SignalHandlerType {
    Default,
    Ignored,
    UserDefinedSimple(NonNull<()>),
    UserDefinedDetailed(NonNull<()>),
}
