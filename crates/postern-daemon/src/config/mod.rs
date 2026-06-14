//! 进程配置子域：从环境变量 + argv 解出 [`DaemonConfig`]（库面、零外部依赖）。
//!
//! 运行期形态（模块文档 06 §5.1 进程对外形态 / §8）：`posternd` 的全部启动参数——
//! policy.db / vault / keyfile 路径、control.sock / data.sock 路径与 data.sock 专用属组——
//! 经 `POSTERN_*` 环境变量提供，缺省落 `$XDG_RUNTIME_DIR/postern/…`（再缺省 `/run/postern/…`），
//! 与 cli 的 [`control_socket_path`](../../postern_cli) 缺省约定一致（同一 `XDG_RUNTIME_DIR/postern`
//! 根、同一 `/run/postern` 兜底）。子命令（`init` / `run`）经手写 argv 解析得出，缺省 `run`。
//!
//! 依赖纪律（编排白名单）：**绝不引 clap**——配置解析只用 `std::env::{var, args}` + 手写
//! argv 分流。本文件零 SQL 标记、不构造 `ConnOrigin`/`ResolvedTarget`/`ResourceCredential`、
//! `anyhow` 禁用（仅 main.rs）。

use std::path::PathBuf;

/// 进程子命令（argv 解析产物）。缺省（无子命令 / 未识别）落 [`Run`](Subcommand::Run)。
///
/// - [`Init`](Subcommand::Init)：首启初始化——生成主密钥 keyfile + 空 vault + 已迁移 db，
///   拒绝覆盖已存在文件（[`bootstrap::init`](crate::bootstrap::init)）。
/// - [`Run`](Subcommand::Run)：常规启动——驱动 boot 启动链开放两平面（缺省动作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subcommand {
    /// 首启初始化（生成 keyfile + 空 vault + 已迁移 db，幂等拒绝覆盖）。
    Init,
    /// 常规启动（驱动 boot 启动链；缺省子命令）。
    Run,
}

/// daemon 启动配置：四个文件路径 + 两个 socket 路径 + data.sock 专用属组。
///
/// 全部字段经 [`from_env`](DaemonConfig::from_env) 从 `POSTERN_*` 环境变量解出，缺省落
/// `$XDG_RUNTIME_DIR/postern/…`（再缺省 `/run/postern/…`）。`data_sock_group` 为 `None` 时
/// data.sock 不设专用属组（部署前置由运维补齐）。本结构只承载路径事实，无任何 IO / 解锁逻辑。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    /// policy.db 路径（`POSTERN_DB`，缺省 `<base>/policy.db`）。
    pub db_path: PathBuf,
    /// vault 文件路径（`POSTERN_VAULT`，缺省 `<base>/vault.postern`）。
    pub vault_path: PathBuf,
    /// keyfile（32B 主密钥）路径（`POSTERN_KEYFILE`，缺省 `<base>/keyfile`）。
    pub keyfile_path: PathBuf,
    /// control.sock 路径（`POSTERN_CONTROL_SOCK`，缺省 `<base>/control.sock`）。
    pub control_sock: PathBuf,
    /// data.sock 路径（`POSTERN_DATA_SOCK`，缺省 `<base>/data.sock`）。
    pub data_sock: PathBuf,
    /// data.sock 专用属组名（`POSTERN_DATA_GROUP`，缺省 `None`：不设专用组）。
    pub data_sock_group: Option<String>,
}

impl DaemonConfig {
    /// 从 `POSTERN_*` 环境变量解出配置：每个路径取显式环境变量、缺省落 `<base>/<name>`，
    /// `<base>` = `$XDG_RUNTIME_DIR/postern`（再缺省 `/run/postern`，与 cli 约定一致）。
    ///
    /// `data_sock_group` 取 `POSTERN_DATA_GROUP`，未设即 `None`。纯解析、无 IO、不创建任何
    /// 文件 / 目录（创建发生在 [`bootstrap::init`](crate::bootstrap::init) / boot socket 绑定期）。
    pub fn from_env() -> Self {
        let base = runtime_base();
        let path_or_default = |var: &str, name: &str| -> PathBuf {
            std::env::var_os(var)
                .map(PathBuf::from)
                .unwrap_or_else(|| base.join(name))
        };
        Self {
            db_path: path_or_default("POSTERN_DB", "policy.db"),
            vault_path: path_or_default("POSTERN_VAULT", "vault.postern"),
            keyfile_path: path_or_default("POSTERN_KEYFILE", "keyfile"),
            control_sock: path_or_default("POSTERN_CONTROL_SOCK", "control.sock"),
            data_sock: path_or_default("POSTERN_DATA_SOCK", "data.sock"),
            data_sock_group: std::env::var("POSTERN_DATA_GROUP").ok(),
        }
    }
}

/// daemon 运行期根目录（缺省路径基底）：`$XDG_RUNTIME_DIR/postern`，再缺省 `/run/postern`。
///
/// 与 cli `control_socket_path` 的缺省约定逐字一致（同 `XDG_RUNTIME_DIR/postern` 根、同
/// `/run/postern` 兜底），使两端在未显式指定 socket 路径时落同一目录。纯路径推导、无 IO。
pub fn runtime_base() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| PathBuf::from(dir).join("postern"))
        .unwrap_or_else(|| PathBuf::from("/run/postern"))
}

/// 手写 argv 子命令解析（**不引 clap**）：第一个非程序名参数为 `init` ⇒ [`Subcommand::Init`]，
/// 否则（含无参 / `run` / 未识别）⇒ [`Subcommand::Run`]（缺省 run）。
///
/// 只识别这两个子命令；其余参数不在本波次解析（路径经环境变量提供，非 argv）。`args` 为
/// 完整 argv（含程序名 `argv[0]`），本函数跳过 `argv[0]` 后判别第一个子命令 token。
pub fn parse_argv<I, S>(args: I) -> Subcommand
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    match args.into_iter().nth(1).as_ref().map(AsRef::as_ref) {
        Some("init") => Subcommand::Init,
        _ => Subcommand::Run,
    }
}
