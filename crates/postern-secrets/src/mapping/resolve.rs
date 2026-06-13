//! 代号解析为真实目标地址（设计承诺级桩，函数体 `todo!()`）。
//!
//! 唯一对外接口（签名权威：§5.4 与详细设计 4.3）：
//! `resolve(code: &ResourceCode) -> Result<ResolvedTarget, ResolveError>`，挂在
//! 已解锁保险箱句柄上（`impl UnlockedVault`）——句柄的存在即"保险箱可用"前提，
//! 解析只对句柄持有的 `targets` 段做纯查表。
//!
//! 物化规则（详细设计 4.3 的 `targets` 字段形态）：某代号下的字段映射拼成
//! `ResolvedTarget` 的 `endpoint` 明文——`{host, port}` → `host:port`；
//! `{instance_id, region}` → `instance_id@region`。字段值在 `Zeroizing<String>`
//! 内流转，物化时不复制明文到非 `Zeroizing` 中间串。
//!
//! fail-closed（§8 F-4 / L-5 / L-11）：未知代号 → `Err(UnknownCode)`、无产物；
//! 字段不成形（缺 `host`/`port` 且缺 `instance_id`/`region`）→ `Err(UnknownCode)`；
//! 错误码恒为常量英文文案、绝不内插 code / 真实地址明文。

use postern_core::domain::{ResolvedTarget, ResourceCode};

use crate::error::ResolveError;
use crate::vault::UnlockedVault;

impl UnlockedVault {
    /// 代号→真实地址的不透明解析：对句柄持有的 `targets` 段做纯查表，把命中的
    /// 字段映射物化成不透明 `ResolvedTarget`（本 crate 唯一构造点）。
    ///
    /// 纯内存、零 IO、常数级查表；成功只返回不透明产物（`Debug=REDACTED`），
    /// 失败 fail-closed 返回 `ResolveError`、无产物。
    pub fn resolve(&self, code: &ResourceCode) -> Result<ResolvedTarget, ResolveError> {
        // 句柄持有的 `targets` 段；纯查表，零 IO、无运行期状态。
        let fields = self
            .payload()
            .targets
            .get(code.as_str())
            .ok_or(ResolveError::UnknownCode)?;

        // 物化（详细设计 4.3 的两种地址形态）。先认 `{host, port}`，再认
        // `{instance_id, region}`；两形态都不成形 → fail-closed `UnknownCode`，
        // 绝不用单字段拼半截端点。明文取自 `Zeroizing<String>`，仅在最终物化进
        // `ResolvedTarget::endpoint`（本 crate 唯一构造点）这一步落入非 `Zeroizing`
        // 串，不另设非 `Zeroizing` 中间副本。
        let endpoint = if let (Some(host), Some(port)) = (fields.get("host"), fields.get("port")) {
            let mut e = String::with_capacity(host.len() + 1 + port.len());
            e.push_str(host);
            e.push(':');
            e.push_str(port);
            e
        } else if let (Some(instance_id), Some(region)) =
            (fields.get("instance_id"), fields.get("region"))
        {
            let mut e = String::with_capacity(instance_id.len() + 1 + region.len());
            e.push_str(instance_id);
            e.push('@');
            e.push_str(region);
            e
        } else {
            return Err(ResolveError::UnknownCode);
        };

        // 唯一构造点（契约 SEC_CONSTRUCTION_SITES）：本 crate 写结构体字面量。
        // 开花括号置于注释后独占一行，与 core 中机密类型同纪律。
        Ok(ResolvedTarget // sole construction point in this crate
        {
            endpoint,
        })
    }
}
