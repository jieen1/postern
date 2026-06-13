//! CLI 错误词汇与退出码映射（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.6/§3.9、§8、详细设计 7.1）：CLI 自身的失败用本 crate 的
//! `thiserror` 错误枚举建模，映射到非零退出码 + 明确呈现。CLI 端失败只分三类
//! （无数据面"拒绝阶段"概念，CLI 不在 [0]~[10] 求值管线内，§3.9 明示）：
//! - **本地语法拒绝**（缺参 / 互斥 / 格式非法 / `--cap` 非合法动词字面量）——对
//!   `control.sock` 零请求（L-1），唯一"未发请求即失败"的类别；
//! - **daemon 不可达**（`control.sock` 缺失 / 无权连 / 未监听 / 连接或读取超时）——绝不
//!   回退本地策略或缓存决策（L-2，结构性：无 store/secrets 依赖即无可回退路径）；
//! - **daemon 返回错误信封 / 4xx-5xx**（含 `{error:{code,message}}`、`409` 冲突、写端点
//!   5xx）——原样呈现，不本地补偿（L-7）。
//!
//! 外加一个**响应不可解析**变体（响应不符合 core 共享类型契约：缺字段 / 类型错），
//! 落实 fail-closed 的客户端延续（§3.9、L-3）：任何不确定（连不上 / 解不出 / 缺字段）
//! 一律报错非零退出，绝不静默成功或猜测补全。
//!
//! 本枚举**不**复用 core 的"错误变体 → 拒绝阶段"穷尽 match——那是数据面求值路径的产物，
//! CLI 无"拒绝阶段"概念；本文件不 import 任何 `Stage` / deny-stage 类型。`anyhow` 仅允许
//! 在二进制 `main`（详细设计 7.1），本结构化库侧错误不引入。
//!
//! 退出码（§3.6）：成功路径 → 0；三类失败 + 解析失败各映射到**互异的非零**退出码，
//! 供消费侧（含脚本/CI）据码分流。

use thiserror::Error;

/// 成功路径退出码。
pub const EXIT_OK: i32 = 0;

/// CLI 端可观测失败面的唯一结构化错误枚举（每 crate 一个 thiserror 枚举纪律，
/// 详细设计 7.1）。恰建模 §3.6/§3.9 的三类失败 + 一个响应解析失败变体；变体文案为
/// 常量英文，只转述 daemon 已脱敏的事实，绝不内嵌真实地址 / 凭据 / 账号明文，也绝不
/// 本地补全或推测（公理六、L-4）。
#[derive(Debug, Error)]
pub enum CliError {
    /// **本地语法拒绝**（§3.6 第一类，L-1）：clap 或本地字面量校验直接失败——缺必填参数 /
    /// 互斥参数同给 / 格式非法（如 `--ttl` 非时长）/ `--cap` 非合法动词字面量。这是唯一
    /// "未发请求即失败"的类别，对 `control.sock` 零请求。`usage` 携带 clap 已渲染好的用法
    /// 文本（纯本地语法事实，无任何安全判断、无真实地址）。
    #[error("invalid command usage")]
    LocalReject { usage: String },

    /// **daemon 不可达**（§3.6 第二类，L-2）：`control.sock` 缺失 / 无权连 / daemon 未监听 /
    /// 连接或读取超时。报错且输出**无**任何决策结论（无 allow/deny/授权视图）；CLI 结构上
    /// 无可回退的本地安全路径（无 store/secrets 依赖）。
    #[error("daemon unreachable")]
    DaemonUnreachable,

    /// **daemon 返回错误信封 / 4xx-5xx**（§3.6 第三类，L-7）：原样呈现统一
    /// `{error:{code,message}}` 信封（含 `409` 冲突、写端点 5xx）。`code`/`message` 取自信封、
    /// 逐字转述——`message` 是 daemon 侧已脱敏常量文案，CLI 不展开、不补全、不重写、不本地
    /// 补偿（不假定部分生效、不回滚、不重试）。
    #[error("daemon returned error envelope")]
    DaemonError { code: String, message: String },

    /// **响应不可解析**（fail-closed 延续，L-3）：响应不符合 core 共享类型契约
    /// （缺字段 / 类型错 / 非预期形态）。本地报错非零退出，绝不补默认值、绝不当成功渲染。
    /// `detail` 是本地解码器给出的常量类别描述，不回显响应原文（避免外泄未脱敏字节）。
    #[error("response did not match shared-type contract")]
    DecodeFailed { detail: String },
}

impl CliError {
    /// 本失败类别对应的非零退出码（§3.6）。四类两两互异，供消费侧据码分流：
    /// 本地语法拒绝 / daemon 不可达 / daemon 错误信封 / 响应解析失败。
    /// 0 由成功路径（[`EXIT_OK`]）独占，每个失败类别取一个互异的非零码。
    pub fn code(&self) -> i32 {
        match self {
            CliError::LocalReject { .. } => 2,
            CliError::DaemonUnreachable => 3,
            CliError::DaemonError { .. } => 4,
            CliError::DecodeFailed { .. } => 5,
        }
    }
}

/// 把一条命令的最终结果映射为进程退出码（§3.6）：成功 → [`EXIT_OK`]（0）；
/// 失败 → 该 [`CliError`] 类别的互异非零码。`main` 据此 `process::exit`。
pub fn exit_code(outcome: &Result<(), CliError>) -> i32 {
    match outcome {
        Ok(()) => EXIT_OK,
        Err(err) => err.code(),
    }
}
