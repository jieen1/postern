//! D1 配置子域行为测试（RED）。
//!
//! 钉死 [`DaemonConfig::from_env`] / [`parse_argv`]（模块文档 06 §5.1 进程对外形态）：
//! - **env 优先级**：显式 `POSTERN_*` 环境变量覆盖缺省路径；
//! - **缺省路径**：未设环境变量时落 `$XDG_RUNTIME_DIR/postern/<name>`，再缺省 `/run/postern/<name>`
//!   （与 cli `control_socket_path` 缺省约定逐字一致）；
//! - **argv 子命令解析**：`init` ⇒ [`Subcommand::Init`]，无参 / `run` / 未识别 ⇒ [`Subcommand::Run`]
//!   （缺省 run），**不引 clap**。
//!
//! 先红后绿：实现体为 `unimplemented!()`，凡调用 `from_env` / `runtime_base` / `parse_argv`
//! 即 panic → 观察到红。`from_env` 读进程环境变量，故每条用例在断言前显式设/清相关变量
//! （`POSTERN_*` / `XDG_RUNTIME_DIR`），避免相互串扰；env 用例集中在单一 `#[test]` 内串行设置，
//! 杜绝并行测试争用进程级环境变量。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::path::PathBuf;

use postern_daemon::config::{parse_argv, runtime_base, DaemonConfig, Subcommand};

// ════════════════════════════════════════════════════════════════════════
//  env 操作辅助（进程级，串行设置以免并行串扰）
// ════════════════════════════════════════════════════════════════════════

/// 清空本测试关心的全部环境变量（每条 env 断言前归零，确保从干净基线起算）。
fn clear_all_env() {
    for k in [
        "POSTERN_DB",
        "POSTERN_VAULT",
        "POSTERN_KEYFILE",
        "POSTERN_CONTROL_SOCK",
        "POSTERN_DATA_SOCK",
        "POSTERN_DATA_GROUP",
        "XDG_RUNTIME_DIR",
    ] {
        std::env::remove_var(k);
    }
}

// ════════════════════════════════════════════════════════════════════════
//  argv 子命令解析（不引 clap）
// ════════════════════════════════════════════════════════════════════════

/// `init` 子命令 → `Subcommand::Init`。
#[test]
fn parse_argv_recognizes_init() {
    let argv = ["posternd", "init"];
    assert_eq!(parse_argv(argv), Subcommand::Init);
}

/// 显式 `run` 子命令 → `Subcommand::Run`。
#[test]
fn parse_argv_recognizes_run() {
    let argv = ["posternd", "run"];
    assert_eq!(parse_argv(argv), Subcommand::Run);
}

/// 无子命令（仅程序名）→ 缺省 `Subcommand::Run`。
#[test]
fn parse_argv_defaults_to_run_when_no_subcommand() {
    let argv = ["posternd"];
    assert_eq!(parse_argv(argv), Subcommand::Run);
}

/// 未识别子命令 → 缺省 `Subcommand::Run`（不 panic、不引 clap 的报错退出）。
#[test]
fn parse_argv_unknown_subcommand_defaults_to_run() {
    let argv = ["posternd", "frobnicate"];
    assert_eq!(parse_argv(argv), Subcommand::Run);
}

// ════════════════════════════════════════════════════════════════════════
//  env 行为（runtime_base + from_env）：全部读/写进程级环境变量，
//  必须在「单一 #[test]」内按序执行——否则多个 #[test] 在默认并行下争用进程级
//  `POSTERN_*` / `XDG_RUNTIME_DIR`，一个线程在另一个的 set 与 assert 之间清/改 env，
//  令断言非确定性变红（实测 8 线程下可复现）。这里把四个 env 场景钉成一个测试的四个
//  连续相（每相起始 `clear_all_env()` 归零基线），从根上消除跨测试的 env 交错。
// ════════════════════════════════════════════════════════════════════════

/// env 子域的全部场景：`runtime_base` 的 XDG/兜底分支 + `from_env` 的缺省/全覆盖/混合覆盖。
/// 四相在同一测试内顺序执行，互不并发，故不会争用进程级环境变量。
#[test]
fn env_behavior_runs_serially_within_one_test() {
    // ── 相 1：runtime_base 取 XDG，未设则兜底 /run/postern。
    {
        clear_all_env();
        std::env::set_var("XDG_RUNTIME_DIR", "/tmp/xdg-test-123");
        assert_eq!(
            runtime_base(),
            PathBuf::from("/tmp/xdg-test-123/postern"),
            "phase=runtime_base xdg"
        );

        std::env::remove_var("XDG_RUNTIME_DIR");
        assert_eq!(
            runtime_base(),
            PathBuf::from("/run/postern"),
            "phase=runtime_base fallback"
        );
    }

    // ── 相 2：未设任何 POSTERN_*、设了 XDG → 六字段全落 <base>/<name> 缺省、组为 None。
    {
        clear_all_env();
        std::env::set_var("XDG_RUNTIME_DIR", "/tmp/xdg-defaults");
        let base = PathBuf::from("/tmp/xdg-defaults/postern");

        let cfg = DaemonConfig::from_env();
        assert_eq!(cfg.db_path, base.join("policy.db"), "phase=defaults db");
        assert_eq!(
            cfg.vault_path,
            base.join("vault.postern"),
            "phase=defaults vault"
        );
        assert_eq!(
            cfg.keyfile_path,
            base.join("keyfile"),
            "phase=defaults keyfile"
        );
        assert_eq!(
            cfg.control_sock,
            base.join("control.sock"),
            "phase=defaults control"
        );
        assert_eq!(cfg.data_sock, base.join("data.sock"), "phase=defaults data");
        assert_eq!(cfg.data_sock_group, None, "phase=defaults group");
    }

    // ── 相 3：显式 POSTERN_* 逐字段覆盖对应缺省。
    {
        clear_all_env();
        std::env::set_var("XDG_RUNTIME_DIR", "/tmp/xdg-override");
        std::env::set_var("POSTERN_DB", "/data/custom.db");
        std::env::set_var("POSTERN_VAULT", "/secrets/custom.vault");
        std::env::set_var("POSTERN_KEYFILE", "/keys/custom.key");
        std::env::set_var("POSTERN_CONTROL_SOCK", "/run/c.sock");
        std::env::set_var("POSTERN_DATA_SOCK", "/run/d.sock");
        std::env::set_var("POSTERN_DATA_GROUP", "postern-agents");

        let cfg = DaemonConfig::from_env();
        assert_eq!(
            cfg.db_path,
            PathBuf::from("/data/custom.db"),
            "phase=override db"
        );
        assert_eq!(
            cfg.vault_path,
            PathBuf::from("/secrets/custom.vault"),
            "phase=override vault"
        );
        assert_eq!(
            cfg.keyfile_path,
            PathBuf::from("/keys/custom.key"),
            "phase=override keyfile"
        );
        assert_eq!(
            cfg.control_sock,
            PathBuf::from("/run/c.sock"),
            "phase=override control"
        );
        assert_eq!(
            cfg.data_sock,
            PathBuf::from("/run/d.sock"),
            "phase=override data"
        );
        assert_eq!(
            cfg.data_sock_group,
            Some("postern-agents".to_string()),
            "phase=override group"
        );
    }

    // ── 相 4：仅覆盖 data_sock，其余字段仍落 <base> 缺省（混合优先级）。
    {
        clear_all_env();
        std::env::set_var("XDG_RUNTIME_DIR", "/tmp/xdg-partial");
        std::env::set_var("POSTERN_DATA_SOCK", "/run/postern/d.sock");
        let base = PathBuf::from("/tmp/xdg-partial/postern");

        let cfg = DaemonConfig::from_env();
        // 覆盖的字段取显式值。
        assert_eq!(
            cfg.data_sock,
            PathBuf::from("/run/postern/d.sock"),
            "phase=partial data"
        );
        // 未覆盖的字段仍落缺省。
        assert_eq!(cfg.db_path, base.join("policy.db"), "phase=partial db");
        assert_eq!(
            cfg.control_sock,
            base.join("control.sock"),
            "phase=partial control"
        );
        assert_eq!(cfg.data_sock_group, None, "phase=partial group");
    }

    clear_all_env();
}
