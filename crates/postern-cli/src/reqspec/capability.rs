//! `--cap <res:verb>` 本地字面量语法校验（骨架占位）。
//!
//! 职责（07-postern-cli §3.6，L-1）：仅做**本地参数语法校验**——`--cap` 是否为合法
//! `<res:verb>` 形态、`verb` 是否为合法动词字面量（六动词字面量集），`--ttl` 是否为合法
//! 时长形态等。语法非法即本地拒绝、非零退出、对 `control.sock` 零请求。
//!
//! 红线（§4、L-1）：本域只判"形态合法"，**绝不**判"是否被允许 / 是否被授权"——任何
//! 语义合法性判断一律不在 CLI（否则即成客户端安全逻辑）。tier 选择 / RBAC / 细则 / 条件
//! 全在 daemon 侧裁决，CLI 不持、不比对、不实现。
//!
//! 雷区（§6.1、本 unit 概要）：core 的 `Capability` 只 derive `Serialize`，无 `FromStr` /
//! `Deserialize`——`--cap` 的 verb 字面量只能与 `Capability::as_str()` 文本集逐字比对
//! （六动词：observe/query/mutate/execute/manage/destroy），**绝不**经反序列化构造
//! `Capability`（那会把"是否合法字面量"误读为"构造出一个授权语义"）。

use postern_core::domain::Capability;

/// 一条 `--cap <res:verb>` 字面量解析成功的产物：拆出资源段与动词段的**原样字符串**。
///
/// 关键纪律（§6.1、L-1）：本结构**不**持 `Capability` 值——它只是把 `<res:verb>` 拆成两段
/// 已通过"形态合法 + verb ∈ 六动词字面量集"语法检查的不透明字符串，供请求构造原样落入
/// body / path。CLI 永不据此下授权判断、永不把 verb 反序列化成 `Capability`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapLiteral {
    /// 资源段（冒号左侧），不透明字符串，CLI 不校验其语义存在性（那是 daemon 职责）。
    pub resource: String,
    /// 动词段（冒号右侧），已校验属于 `Capability::as_str()` 六动词字面量集之一。
    pub verb: String,
}

/// `--cap` 本地字面量校验失败的原因（§3.6 本地语法拒绝类、L-1）。语法非法即本地拒绝、
/// 对 `control.sock` 零请求——是唯一"未发请求即失败"的类别。**不**含任何授权语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapParseError {
    /// 缺少冒号分隔（如 `frobnicate`）：不是合法 `<res:verb>` 形态。
    MissingColon,
    /// 资源段为空（如 `:destroy`）。
    EmptyResource,
    /// 动词段不在六动词字面量集（`Capability::as_str()` 集）内：本地拒绝、非授权判断。
    UnknownVerb {
        /// 被拒的 verb 原文（用于本地用法呈现，纯语法事实，无授权含义）。
        verb: String,
    },
}

/// 六动词字面量集——`Capability::as_str()` 在每个变体上的镜像，作为 `--cap` verb 段的
/// 唯一合法集（§6.1、雷区）。**唯一来源是 core 的 `as_str()`**，非本 crate 私自硬编码字符串，
/// 故核中新增 / 改名动词时本集随之变动，杜绝字面量集与权威枚举漂移。`as_str()` 非 `const fn`，
/// 故以函数（而非 `const` 数组）求值——仍只引用 core 枚举值，不在本侧硬写动词文本。
pub fn valid_verbs() -> [&'static str; 6] {
    [
        Capability::Observe.as_str(),
        Capability::Query.as_str(),
        Capability::Mutate.as_str(),
        Capability::Execute.as_str(),
        Capability::Manage.as_str(),
        Capability::Destroy.as_str(),
    ]
}

/// 把 `--cap <res:verb>` 实参做**纯本地字面量语法校验**并拆段（§3.6、L-1）。
///
/// 接受当且仅当：含恰一个冒号分隔、资源段非空、动词段 ∈ [`valid_verbs`]
/// （`Capability::as_str()` 集）。任何其它形态 → [`CapParseError`]，本地拒绝、零请求。
/// 本函数**不**做任何授权判断、**不**构造 `Capability`、**不**比对 RBAC——只判"是否合法
/// 动词字面量"。
pub fn parse_cap(raw: &str) -> Result<CapLiteral, CapParseError> {
    let (resource, verb) = raw.split_once(':').ok_or(CapParseError::MissingColon)?;

    if resource.is_empty() {
        return Err(CapParseError::EmptyResource);
    }

    if !valid_verbs().contains(&verb) {
        return Err(CapParseError::UnknownVerb {
            verb: verb.to_string(),
        });
    }

    Ok(CapLiteral {
        resource: resource.to_string(),
        verb: verb.to_string(),
    })
}
