//! postgres 执行：在 `Channel` 上以线协议执行已放行意图（骨架占位，§3.4）。
//!
//! 在递来的 `&mut Channel`（不透明本地通路抽象）上以 PostgreSQL 线协议执行 `Intent`
//! 携带的原文，产出**未脱敏** `RawResponse`（脱敏由内核出口完成，§4）。执行前**绝不
//! 重新解析或改写**已放行的原文——`classify` 阶段的语法树只用于定档，执行用原文，避免
//! 「解析与执行看到不同语句」的二义。会话只读等语义已在**建连时**由连接管理层统一施加，
//! 适配器不在 `execute` 内补打会话设定。
//!
//! 只见 `Channel`，拿不到地址 / 凭据 / tier（§4 / L-13）；通路中断经 `Channel` 收错并以
//! `ExecError` 上报，由内核脱敏返回。有副作用动词一旦已落库副作用，错误经出口脱敏返回、
//! **绝不再 deny**（时序不变量由内核守护，本 crate 经 `Err` 协同，L-11）。
//!
//! **骨架阶段 fail-closed（公理二）**：`Channel.handle` 是 `Box<dyn Send + Sync>` 的不透明
//! 句柄——`dyn Send + Sync` 不可下转为 `dyn Any`，适配器在进程内**取不到**句柄具体载荷，
//! 故无真实 pg 线协议客户端可在其上跑、也无资源结果集可读。真实回放须容器集成层
//! （`pg-itest`）以接管底层流的 pg 客户端落地。在此之前，`execute` 对任何输入**绝不伪造
//! 响应**：解码印证负载确为一份可回放的语句原文后，一律 fail-closed 为
//! `Err(ExecError::ExecutionFailed)`——与 docker_logs / http 骨架同纪律。**绝不**把请求
//! 原文字节回吐为 `Ok(RawResponse)`（那是「从不执行、只回显请求」的伪 execute，对写 /
//! destroy 意图更是静默「成功」回吐，违反 fail-closed）。

use postern_core::error::ExecError;
use postern_core::plugin::{Channel, RawResponse};
use postern_core::request::Intent;

use super::intent::PgRequest;

/// 步骤[8] 执行（§3.4）：在 `Channel` 上回放已放行 `Intent` 携带的原文，回未脱敏
/// `RawResponse`。
///
/// 负载先经 [`PgRequest::from_payload`] 反序列化（**仅 JSON 解码，绝不重新解析 / 改写
/// 已放行的语句原文**——`classify` 阶段的语法树只用于定档，执行用的是负载里那份逐字
/// 原文，杜绝「解析与执行看到不同语句」的二义）；解码失败一律 fail-closed 为
/// `Err(ExecError::ProtocolViolation)`（不 panic、不伪造响应，公理二）。
///
/// `Channel.handle` 是 `Box<dyn Send + Sync>` 的不透明本地通路抽象；`dyn Send + Sync`
/// 不可下转，适配器在进程内取不到句柄具体载荷、无真实 pg 线协议客户端可在其上跑。真实
/// pg 客户端在容器集成层（`pg-itest`）接管底层流、把结果集以未脱敏字节回交；脱敏归内核
/// 出口步骤[9]，`execute` 本方法**绝不擦字节**（F-9）。会话只读等语义已在建连时由连接
/// 管理层施加，`execute` 不补打会话设定、不池化、不 spawn 后台任务（§3.4 / §3.7）。
///
/// **本波次无真实引擎**：解码印证负载确为可回放的语句原文后，一律 fail-closed 为
/// `Err(ExecError::ExecutionFailed)`——**绝不**把请求原文字节回吐为 `Ok(RawResponse)`
/// （那是「从不执行、只回显请求」的伪 execute，对写 / destroy 意图更属静默「成功」回吐，
/// 违反公理二）；与 docker_logs / http 骨架同纪律。
///
/// 误归类的写经只读账号被引擎拒（`engine_enforced` 兜底）须真实 PostgreSQL 容器取证，属
/// 集成层验收（语料 `engine_fallback` 组）；本地通路无引擎权限模型可施压，骨架阶段对其
/// 同样 fail-closed（无 wire 可执行），故「绝不静默执行成功」这条不变量**默认即被强制**。
pub async fn execute(ch: &mut Channel, intent: &Intent) -> Result<RawResponse, ExecError> {
    let _ = ch;

    // 仅 JSON 解码，取出已放行的原文负载——绝不对其重新做语义解析或改写（§3.4）。
    // 解码即印证负载是一份可回放的语句原文；解码失败 fail-closed 为 ProtocolViolation。
    let _request =
        PgRequest::from_payload(intent.payload()).map_err(|_| ExecError::ProtocolViolation)?;

    // 句柄不透明（不可下转），无真实 pg wire 可执行、无资源结果集可读——fail-closed，
    // 绝不伪造 `Ok(RawResponse)`、绝不回吐请求原文字节（公理二）。真实回放归容器集成层。
    Err(ExecError::ExecutionFailed)
}
