//! posternd 二进制入口。
//!
//! 唯一职责：建立 tokio multi-thread 运行时，把控制权交给库的 [`boot`] 启动序列
//! （开库→重建快照→解锁保险箱→注册插件→最后开放 data.sock）。任一步 Err 在 socket
//! 创建前短路，进程以非零码退出（公理二 fail-closed）：boot 的 Err 不经 `?` 传播，而是
//! 显式映射为 [`EXIT_BOOT_FAILED`](postern_daemon::assemble::EXIT_BOOT_FAILED) 非零退出码。
//!
//! 本波次为骨架：入口连通到 boot 桩，零业务逻辑。
#![forbid(unsafe_code)]

use postern_daemon::assemble::EXIT_BOOT_FAILED;
use postern_daemon::boot;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // boot 启动链：开库→重建快照→解锁保险箱→注册插件→最后开放 data.sock。任一步 Err 在
    // socket 创建前短路。boot 的 Err **不**经 `?` 传播（那会把退出码语义丢给 anyhow），而是
    // 显式映射为非零进程退出码（公理二 fail-closed：systemd/容器据此判定启动失败、不误判已就绪）。
    if boot::run().await.is_err() {
        std::process::exit(EXIT_BOOT_FAILED as i32);
    }
}
