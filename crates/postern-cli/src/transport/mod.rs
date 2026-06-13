//! 传输面（骨架占位）。
//!
//! 职责（07-postern-cli §3.2/§3.9，F-3、L-2）：把"HTTP over UDS"封装为一次性客户端调用，
//! 经 `hyper + hyperlocal` 建立到 `control.sock` 的连接，发起**恰好一次**请求并接收完整
//! 响应——每条命令新建连接、发一次、读完整、关闭，**无连接复用、无后台保活**（瘦客户端
//! 的正确形态是无状态单发，与 F-3"恰一次往返"一致）。
//!
//! 不可达的诚实失败（§3.2，L-2）：`control.sock` 不存在 / 无权连（`0600` 属主外）/
//! daemon 未监听 → 连接阶段即失败，直接转为"daemon 不可达"本地错误；**绝不**因连不上
//! 而走任何本地策略 / 缓存路径——CLI 结构上无此路径可走（无 store/secrets 依赖）。权限
//! 边界（`control.sock` 的 `0600` + 控制面认证）是部署前置，非 CLI 设防。
//!
//! 子模块：`uds`（hyperlocal 连 `control.sock` 的一次性请求 / 响应往返）。`mcp-stdio` 的
//! `data.sock` 长连接桥在 `bridge` 域，不在本一次往返模型内。
pub mod uds;

pub use uds::{HttpResponse, UdsTransport};
