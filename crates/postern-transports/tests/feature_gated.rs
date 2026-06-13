//! feature 门控形态（ssh / ssm）占位实现的行为测试（RED）。
//!
//! 被测对象：`postern_transports::ssh::SshTransport` 与 `postern_transports::ssm::SsmTransport`
//! ——两个以 cargo feature（`ssh` / `ssm`）门控、与 `direct` 平行的 `core::Transport` 实现。
//! 本波次只钉**形态骨架的固定取值**（§2 第6项、§3.2 ssh·ssm、§8 F-8 / F-6 / L-9）：
//!
//!   - `kind()` 取值精确到具体串（`"ssh"` / `"ssm"`），不取其它形态串；
//!   - `persistent()` 恒为常量 `true`（长连接型——隧道 / 端口转发会话建立成本高、可池化），
//!     且对**同一实例多次调用恒等**（F-6：不读运行时状态）；
//!   - 三形态对上呈现一致的 `core::Channel` 类型与 `open` 签名（L-9：差异不外溢到上层接口）。
//!
//! 各形态用例以 `#[cfg(feature = "ssh")]` / `#[cfg(feature = "ssm")]` 门控：feature 关闭时
//! 对应 `Ssh/SsmTransport` 类型根本不编译，故引用它们的用例也随 feature 一并 cfg 排除——这
//! 保证 `cargo hack check --each-feature` 的 `--no-default-features` 一档下本测试目标空集可编译，
//! `--all-features` 一档下全用例编译（§8 F-8）。
//!
//! **RED 约定**：`Ssh/SsmTransport` 的 `kind()` / `persistent()` 占位体当前为 `todo!()`，故下列
//! 断言在 RED 阶段以 panic 失败；GREEN 阶段填入固定常量后逐条转绿。`open` 的真实隧道路径消费
//! 机密类型（`ResolvedTarget` / `ResourceCredential`），本 crate **不能构造**它们（契约
//! `SEC_CONSTRUCTION_SITES` 仅 secrets），故 `open` 的驱动如实标注为集成层（daemon 注入真实
//! 机密）覆盖，见 type_level_notes——本单元**不构造任何机密类型**，亦不嵌 SQL / ConnOrigin 字面。

// ----------------------------------------------------------------------------
// ssh 形态（feature = "ssh"）
// ----------------------------------------------------------------------------

// §8 F-8 / §3.2 ssh：kind() 精确取 "ssh"，不取其它形态串。
#[cfg(feature = "ssh")]
#[test]
fn ssh_transport_kind_is_exactly_ssh() {
    use postern_core::plugin::Transport;
    use postern_transports::ssh::SshTransport;

    let t = SshTransport;
    assert_eq!(t.kind(), "ssh");
}

// §8 F-8 / §3.2：ssh 的 kind() 不与 direct / ssm 形态串混淆（钉死非其它取值）。
#[cfg(feature = "ssh")]
#[test]
fn ssh_transport_kind_is_not_direct_or_ssm() {
    use postern_core::plugin::Transport;
    use postern_transports::ssh::SshTransport;

    let k = SshTransport.kind();
    assert_ne!(k, "direct");
    assert_ne!(k, "ssm");
}

// §8 F-6 / L-9：ssh 属长连接型，persistent() 恒为常量 true（隧道建立成本高、可池化复用）。
#[cfg(feature = "ssh")]
#[test]
fn ssh_transport_persistent_is_true() {
    use postern_core::plugin::Transport;
    use postern_transports::ssh::SshTransport;

    let t = SshTransport;
    assert!(t.persistent());
}

// §8 F-6：persistent() 是固定常量，同一实例多次调用恒等（不读运行时状态）。
#[cfg(feature = "ssh")]
#[test]
fn ssh_transport_persistent_is_constant_across_calls() {
    use postern_core::plugin::Transport;
    use postern_transports::ssh::SshTransport;

    let t = SshTransport;
    let first = t.persistent();
    let second = t.persistent();
    let third = t.persistent();
    assert_eq!(first, second);
    assert_eq!(second, third);
    assert!(first);
}

// §8 F-8 / L-9：ssh 形态对上呈现一致的 Transport 接口——SshTransport 可作为
// &dyn core::Transport 使用（trait 对象），未引入形态相关的上层接口；其 kind() 即 "ssh"。
#[cfg(feature = "ssh")]
#[test]
fn ssh_transport_is_usable_as_core_transport_object() {
    use postern_core::plugin::Transport;
    use postern_transports::ssh::SshTransport;

    let obj: &dyn Transport = &SshTransport;
    assert_eq!(obj.kind(), "ssh");
    assert!(obj.persistent());
}

// ----------------------------------------------------------------------------
// ssm 形态（feature = "ssm"）
// ----------------------------------------------------------------------------

// §8 F-8 / §3.2 ssm：kind() 精确取 "ssm"，不取其它形态串。
#[cfg(feature = "ssm")]
#[test]
fn ssm_transport_kind_is_exactly_ssm() {
    use postern_core::plugin::Transport;
    use postern_transports::ssm::SsmTransport;

    let t = SsmTransport;
    assert_eq!(t.kind(), "ssm");
}

// §8 F-8 / §3.2：ssm 的 kind() 不与 direct / ssh 形态串混淆（钉死非其它取值）。
#[cfg(feature = "ssm")]
#[test]
fn ssm_transport_kind_is_not_direct_or_ssh() {
    use postern_core::plugin::Transport;
    use postern_transports::ssm::SsmTransport;

    let k = SsmTransport.kind();
    assert_ne!(k, "direct");
    assert_ne!(k, "ssh");
}

// §8 F-6 / L-9：ssm 属长连接型，persistent() 恒为常量 true（端口转发会话建立成本高、可续约复用）。
#[cfg(feature = "ssm")]
#[test]
fn ssm_transport_persistent_is_true() {
    use postern_core::plugin::Transport;
    use postern_transports::ssm::SsmTransport;

    let t = SsmTransport;
    assert!(t.persistent());
}

// §8 F-6：persistent() 是固定常量，同一实例多次调用恒等（不读运行时状态 / 不依赖会话时限）。
#[cfg(feature = "ssm")]
#[test]
fn ssm_transport_persistent_is_constant_across_calls() {
    use postern_core::plugin::Transport;
    use postern_transports::ssm::SsmTransport;

    let t = SsmTransport;
    let first = t.persistent();
    let second = t.persistent();
    let third = t.persistent();
    assert_eq!(first, second);
    assert_eq!(second, third);
    assert!(first);
}

// §8 F-8 / L-9：ssm 形态对上呈现一致的 Transport 接口——SsmTransport 可作为
// &dyn core::Transport 使用（trait 对象），未引入形态相关的上层接口；其 kind() 即 "ssm"。
#[cfg(feature = "ssm")]
#[test]
fn ssm_transport_is_usable_as_core_transport_object() {
    use postern_core::plugin::Transport;
    use postern_transports::ssm::SsmTransport;

    let obj: &dyn Transport = &SsmTransport;
    assert_eq!(obj.kind(), "ssm");
    assert!(obj.persistent());
}

// ----------------------------------------------------------------------------
// 形态平行一致（L-9）：两形态同时启用时，kind() 取值彼此相异、persistent() 同为 true。
// 仅在 ssh 与 ssm 两 feature 同时开启（如 --all-features）时编译。
// ----------------------------------------------------------------------------

// §8 L-9 / F-8：ssh 与 ssm 的 kind() 互异（各自唯一选择键），但 persistent() 同为 true——
// 长 / 非长差异仅由 persistent() 承载，本波次两形态同属长连接型，对上接口一致。
#[cfg(all(feature = "ssh", feature = "ssm"))]
#[test]
fn ssh_and_ssm_kinds_differ_but_both_persistent() {
    use postern_core::plugin::Transport;
    use postern_transports::ssh::SshTransport;
    use postern_transports::ssm::SsmTransport;

    let ssh = SshTransport;
    let ssm = SsmTransport;
    assert_ne!(ssh.kind(), ssm.kind());
    assert_eq!(ssh.kind(), "ssh");
    assert_eq!(ssm.kind(), "ssm");
    assert_eq!(ssh.persistent(), ssm.persistent());
    assert!(ssh.persistent());
}
