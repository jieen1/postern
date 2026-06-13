//! 审计读模型：store 本地读结构（AuditRecord / OriginEnvelope 等），不构造 core 来源类型。
//!
//! 写路径 `record(event: AuditEvent)` 接收并序列化 core 的 `AuditEvent`（只读取
//! `event.origin` 这个 `ConnOrigin` 值做序列化，绝不构造 `ConnOrigin`）；
//! 读路径反序列化只产 store 本地的 origin 结构（[`OriginEnvelope`]），全程不构造
//! `ConnOrigin`（设计裁定，§5 读模型）。

use serde::{Deserialize, Serialize};

/// store 本地的连接来源信封——倒序扫描反序列化的产物。
///
/// 与 core 的 `ConnOrigin` 解耦：store 源码禁止构造 `ConnOrigin`
/// (SEC_CONSTRUCTION_SITES 仅 daemon shells)，故读路径只产这个普通 serde 结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OriginEnvelope {
    /// 本地进程对端（SO_PEERCRED 信任域门，仅 uid/gid）。
    UnixPeer {
        /// 对端 uid。
        uid: u32,
        /// 对端 gid。
        gid: u32,
    },
    /// TCP 对端地址（已脱敏文本，不回显真实地址语义由内核出口 Sanitizer 保证）。
    Tcp {
        /// 对端地址文本。
        remote: String,
    },
}

/// store 本地审计读模型——`scan` 返回的逐行解析结果。
///
/// 不是 core 的 `AuditEvent`：`origin` 字段用 store 本地 [`OriginEnvelope`]，
/// 反序列化全程不构造 `ConnOrigin`。事件 `id` 为雪花字符串（写时由 core IdGen 序列化）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// 雪花事件 id，序列化为十进制字符串。
    pub id: String,
    /// 信封 schema 版本。
    pub v: u32,
    /// 事件 kind（`request` / `deny` / `policy_change` / `credential_event` / ...）。
    pub kind: String,
    /// 固定宽度 UTC 时间戳，恒长度 24。
    pub ts: String,
    /// shell 入口（`mcp` / `http`）。
    pub entry: String,
    /// 网关观测到的连接来源（store 本地信封，反序列化产物）。
    pub origin: OriginEnvelope,
    /// 决策词（`allow` / `deny` / `escalate_denied`）。
    pub decision: String,
    /// 目标资源代号（恒代号、绝非真实地址）。
    pub resource: String,
    /// 决策时刻策略修订号——对账锚点。
    pub policy_rev: u64,
    /// 窗口聚合计数：仅 deny 类在逼近配额降级时的聚合记录携带（`Some(n)` 表示
    /// 该 `(principal, kind)` 窗口内被折叠的 deny 事件条数）；普通逐事件记录恒 `None`
    /// （`skip_serializing_if` 保证普通行的 JSONL 不出现 `count` 字段，往返不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

impl AuditRecord {
    /// 序列化为单行 JSONL（不含换行）。写路径与扫描往返一致性的格式锚点。
    pub fn to_jsonl(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}
