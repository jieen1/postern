//! `PolicyView` 实现：无锁只读快照视图，原子 `Arc` 替换（§5.1 / §6.2 / §7-13）。
//!
//! [`SnapshotView`] 持当前 [`PolicySnapshot`] 的 `Arc`，实现 core 的
//! [`PolicyView`](postern_core::plugin::PolicyView)：`snapshot()` 返回当前不可变
//! 快照（`Arc` 克隆，读端无锁、不失败）。控制面在写锁临界区内经 [`replace`](SnapshotView::replace)
//! 把整份 `Arc` 原子替换——读者要么看到"重建前完整旧快照"、要么"重建后完整新快照"，
//! 绝不读到半截中间态（L-8）。
//!
//! 实现以 `RwLock<Arc<PolicySnapshot>>` 承载：`snapshot()` 在读锁下克隆 `Arc`
//! （两次相邻读返回同一底层 `Arc` 的克隆，故 `Arc::ptr_eq` 成立——共享、非深拷贝）；
//! `replace` 在写锁下整份换上，旧读者持有的 `Arc` 仍指向旧快照直至释放（无撕裂）。
//! poisoned 锁恢复而非 unwrap（与 base / core IdGen 同纪律，临界区不 panic）。

use std::sync::{Arc, RwLock};

use postern_core::domain::PolicySnapshot;
use postern_core::plugin::PolicyView;

/// 无锁只读快照视图：数据面经此消费 `Arc<PolicySnapshot>`，控制面在写锁内原子替换。
pub struct SnapshotView {
    current: RwLock<Arc<PolicySnapshot>>,
}

impl SnapshotView {
    /// 以首份快照装配视图（boot 物化首份快照后注入数据面 router）。
    pub fn new(initial: Arc<PolicySnapshot>) -> Self {
        Self {
            current: RwLock::new(initial),
        }
    }

    /// 原子替换当前快照（控制面在写锁临界区内、事务 COMMIT 后调用）。整份 `Arc`
    /// 一次性换上，读者绝不见半截状态。poisoned 锁恢复（写临界区不 panic）。
    pub fn replace(&self, next: Arc<PolicySnapshot>) {
        let mut guard = self
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = next;
    }
}

impl PolicyView for SnapshotView {
    fn snapshot(&self) -> Arc<PolicySnapshot> {
        let guard = self
            .current
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(&guard)
    }
}
