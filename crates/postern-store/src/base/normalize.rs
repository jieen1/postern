//! 归一化入库：名称 trim + 明示小写策略，唯一索引作用于归一化值。
//!
//! `principals.name` / `roles.name` / `resources.codename` 等入库前由 `base`
//! 统一归一化：`trim` 去首尾空白 + 明示**小写**策略。归一化函数供唯一索引值产出
//! （防 `Admin`、` admin ` 类大小写/空白绕过；§3.1/§3.2）。

/// 把入库名称归一化为唯一索引值：`trim` 首尾空白后转小写（明示小写策略）。
///
/// `Admin` / ` admin ` / `ADMIN` 三者归一化后相同；归一化值用于 partial unique
/// 索引与 `roles` 表的禁 admin `CHECK`，杜绝绕过。
pub fn normalize_name(raw: &str) -> String {
    raw.trim().to_lowercase()
}
