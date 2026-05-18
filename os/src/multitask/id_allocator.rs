use bitvec::prelude::*;

/// ID 分配器
///
/// 允许的 ID 范围是 [0, M)，M 是 N 向上取整到 64 的倍数后的值。
pub type IdAllocator<const N: usize> = IdAllocatorImpl<{ N.div_ceil(64) }>;

pub struct IdAllocatorImpl<const N: usize> {
    bitmap: BitArray<[u64; N], LocalBits>,
    current: usize,
}

impl<const N: usize> IdAllocatorImpl<N> {
    /// 创建一个新的 ID 分配器，初始状态下所有 ID 都未分配。
    pub const fn new() -> Self {
        Self {
            bitmap: BitArray::ZERO,
            current: 0,
        }
    }

    /// 分配一个 ID
    pub fn allocate(&mut self) -> Option<usize> {
        let (left_part, right_part) = self.bitmap.split_at(self.current);

        if let Some(pos) = right_part
            .first_zero()
            .map(|off| self.current + off)
            .or_else(|| left_part.first_zero())
        {
            self.bitmap.set(pos, true);
            self.current = pos + 1;

            Some(pos)
        } else {
            None
        }
    }

    /// 释放一个 ID
    ///
    /// # Panics
    /// 如果 ID 不在 [0, N) 范围内，或者该 ID 已经被释放，则会 panic。
    pub fn deallocate(&mut self, id: usize) {
        let old = self.bitmap.replace(id, false);

        assert!(old, "ID {id} 被释放了两次");
    }

    /// 分配指定 ID，如果该 ID 已经被分配则返回 `false`，否则分配并返回 `true`
    ///
    /// # Panics
    /// 如果 ID 不在 [0, N) 范围内，则会 panic。
    #[must_use]
    pub fn allocate_specific(&mut self, id: usize) -> bool {
        !self.bitmap.replace(id, true)
    }
}
