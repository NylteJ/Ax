use bitvec::{BitArr, array::BitArray, order::Lsb0, view::BitView};

use crate::multitask::signal::{SignalId, SignalIdUnderlying, SignalMask};

const NORMAL_SIGNAL_COUNT: usize = SignalId::NORMAL_MAX as usize + 1;
static_assertions::const_assert_eq!(NORMAL_SIGNAL_COUNT, 32);

#[derive(Debug)]
pub struct PendingSignals {
    normal: BitArr!(for NORMAL_SIGNAL_COUNT, in u32, Lsb0),
}

impl PendingSignals {
    pub fn new() -> Self {
        Self {
            normal: BitArray::ZERO,
        }
    }

    #[inline]
    pub fn set_normal(&mut self, signal: SignalId) {
        self.normal.set(signal as usize, true);
    }
    #[inline]
    pub fn clear_normal(&mut self, signal: SignalId) {
        self.normal.set(signal as usize, false);
    }
}

#[derive(Debug)]
pub struct SignalsWaitingList {
    pending: PendingSignals,
    pub mask: SignalMask,
}

impl SignalsWaitingList {
    pub fn new() -> Self {
        Self {
            pending: PendingSignals::new(),
            mask: SignalMask::empty(),
        }
    }

    pub fn clone_for_fork(&self) -> Self {
        // 注意信号掩码总是会复制但不共享，未决信号则总是直接清零
        Self {
            pending: PendingSignals::new(),
            mask: self.mask,
        }
    }

    pub fn clear_for_exec(&mut self) {
        self.pending = PendingSignals::new();
    }

    pub fn add_normal(&mut self, signal: SignalId) {
        self.pending.set_normal(signal);
    }

    pub fn try_consume_one(&mut self) -> Option<SignalId> {
        if let Some(signal) = self.find_can_consume() {
            debug_assert!(!signal.is_realtime()); // TODO: 处理实时信号
            self.pending.clear_normal(signal);
            Some(signal)
        } else {
            None
        }
    }

    pub fn find_can_consume(&self) -> Option<SignalId> {
        let full_mask = self.mask.bits();
        let normal_mask = full_mask as u32;

        let normal_pending = &self.pending.normal.data;
        debug_assert_eq!(normal_pending.len(), 1);
        if let Some(index) = (normal_pending[0] & !normal_mask)
            .view_bits::<Lsb0>()
            .first_one()
        {
            Some(SignalId::try_from(index as SignalIdUnderlying).unwrap())
        } else {
            // TODO: 处理实时信号
            None
        }
    }
}
