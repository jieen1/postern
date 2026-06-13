//! `postern-cli`（二进制 `postern`）：控制面瘦客户端 + `mcp-stdio` 数据面字节桥。
//!
//! 设计依据：docs/modules/07-postern-cli.md（与详细设计 4.6、第八部分 8.12 外壳层
//! 客户端侧）。本 crate 处于依赖图末端、不向工作区其他 crate 暴露库接口；其"接口"是
//! 面向人的命令行契约（clap derive 命令树）与面向 daemon 的网络协议
//! （HTTP/JSON over `control.sock`，数据面 `data.sock` 仅 `mcp-stdio` 字节桥）。
//!
//! 必守不变量（§7）：零本地安全逻辑、零本地状态、每条命令 = 一次控制面往返 + 渲染；
//! 雪花 id 恒以字符串呈现（不数值化）；分页交给后端；乐观锁 `version` 只透传不自造；
//! fail-closed 的客户端延续。架构禁止边（契约 ARCH_FORBIDDEN_EDGES）：cli ↛ store/secrets。
//!
//! 本文件声明顶层模块布局，声明后冻结。
pub mod bridge;
pub mod command;
pub mod error;
pub mod init;
pub mod render;
pub mod reqspec;
pub mod transport;
