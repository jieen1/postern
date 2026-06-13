//! 命令面（骨架占位）。
//!
//! 职责（07-postern-cli §3.1，F-1/F-2）：以 clap（derive）定义命令树，把命令行解析为强
//! 类型管理意图，再经请求规格映射到 6.5 控制面端点，走统一翻译管线发起一次往返并渲染。
//! 命令树覆盖 §3 全表 22 个命令组（`daemon`/`init`/`resource`/`principal`/`role`/
//! `credential`/`grants`/`elevate`/`revoke-grant`/`mode`/`freeze`/`constraint`/`condition`/
//! `deny-note`/`settings`/`approvals`/`denials`/`audit`/`verify`/`export`/`import`/
//! `mcp-stdio`），缺任一组即不过 F-1。
//!
//! 统一翻译管线（§3.1，命令间只在端点 / 参数 / 请求体 / 渲染器四处不同，公共主干一份）：
//! 解析 → 意图结构 → 请求规格 → 序列化 / 发送 / 反序列化 → 渲染 → 退出码。`mcp-stdio`
//! 是唯一例外形态（数据面字节桥，在 `bridge` 域）。
//!
//! 子模块：`tree`（clap derive 命令树定义）、`intent`（解析后的强类型管理意图）、
//! `dispatch`（意图 → 请求规格 → 往返 → 渲染 → 退出码的公共主干）。
pub mod dispatch;
pub mod intent;
pub mod tree;
