use alloc::collections::vec_deque::VecDeque;
use bitvec::{order::Lsb0, view::BitView};

use crate::multitask::signal::{SignalId, SignalIdUnderlying, SignalMask};

const NORMAL_SIGNAL_COUNT: usize = SignalId::NORMAL_MAX as usize - SignalId::SIGINVAL as usize;
static_assertions::const_assert_eq!(NORMAL_SIGNAL_COUNT, 32);
const RT_SIGNAL_COUNT: usize = SignalId::MAX as usize - SignalId::NORMAL_MAX as usize;
static_assertions::const_assert_eq!(RT_SIGNAL_COUNT, 32);

static_assertions::const_assert_eq!(SignalId::MAX as usize, 64);

const RT_OFF: usize = NORMAL_SIGNAL_COUNT + 1;

pub type RtSignalData = Option<usize>;

#[derive(Debug)]
pub struct SignalsWaitingList {
    pending: u64,
    realtime_queue: [VecDeque<RtSignalData>; RT_SIGNAL_COUNT],
    pub mask: SignalMask,
}

impl SignalsWaitingList {
    pub fn new() -> Self {
        Self {
            pending: 0,
            realtime_queue: Default::default(),
            mask: SignalMask::empty(),
        }
    }

    pub fn clone_for_fork(&self) -> Self {
        // 注意信号掩码总是会复制但不共享，未决信号则总是直接清零
        Self {
            pending: 0,
            realtime_queue: Default::default(),
            mask: self.mask,
        }
    }

    pub fn clear_for_exec(&mut self) {
        self.pending = 0;
        self.realtime_queue.iter_mut().for_each(VecDeque::clear);
    }

    pub fn add(&mut self, signal: SignalId) {
        self.set_pending(signal);
        if signal.is_realtime() {
            self.realtime_queue[signal as usize - RT_OFF].push_back(None);
        }
    }

    pub fn try_consume_one(&mut self) -> Option<(SignalId, Option<RtSignalData>)> {
        self.try_consume_one_with_mask(self.mask)
    }

    pub fn find_can_consume(&self) -> Option<SignalId> {
        self.find_can_consume_with_mask(self.mask)
    }

    /// 和 `try_consume_one` 类似，但使用指定的信号掩码，而忽略 `self` 中的掩码
    pub fn try_consume_one_with_mask(
        &mut self,
        mask: SignalMask,
    ) -> Option<(SignalId, Option<RtSignalData>)> {
        if let Some(signal) = self.find_can_consume_with_mask(mask) {
            let rt_data = if signal.is_realtime() {
                let queue = &mut self.realtime_queue[signal as usize - RT_OFF];

                let data = queue.pop_front();
                debug_assert!(data.is_some());

                if queue.is_empty() {
                    self.clear_pending(signal);
                }

                data
            } else {
                self.clear_pending(signal);
                None
            };
            Some((signal, rt_data))
        } else {
            None
        }
    }

    /// 和 `find_can_consume` 类似，但使用指定的信号掩码，而忽略 `self` 中的掩码
    pub fn find_can_consume_with_mask(&self, mask: SignalMask) -> Option<SignalId> {
        if let Some(index) = (self.pending & !mask.bits())
            .view_bits::<Lsb0>()
            .first_one()
        {
            Some(SignalId::try_from(index as SignalIdUnderlying + 1).unwrap())
        } else {
            // TODO: 处理实时信号
            None
        }
    }

    fn set_pending(&mut self, signal: SignalId) {
        debug_assert_ne!(signal, SignalId::SIGINVAL);
        unsafe {
            self.pending
                .view_bits_mut::<Lsb0>()
                .set_unchecked(signal as usize - 1, true)
        };
    }
    fn clear_pending(&mut self, signal: SignalId) {
        debug_assert_ne!(signal, SignalId::SIGINVAL);
        unsafe {
            self.pending
                .view_bits_mut::<Lsb0>()
                .set_unchecked(signal as usize - 1, false)
        };
    }
}
