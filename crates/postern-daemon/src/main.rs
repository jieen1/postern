//! posternd 二进制入口。
//!
//! 唯一职责：建立 tokio multi-thread 运行时，把控制权交给库的 [`boot`] 启动序列
//! （开库→重建快照→解锁保险箱→注册插件→最后开放 data.sock），并把 [`boot::run`] 产出的
//! **进程退出码**透到进程边界（公理二 fail-closed）：boot 失败 → 非零退出、data.sock 不
//! serving；boot 成功 → 进程留存 serve 直到信号、退出码 0。
//!
//! 退出码语义全在库面 [`boot::run`]（→ [`run_assembled`](postern_daemon::assemble::run_assembled)
//! 的 u8 语义）；main 只 `std::process::exit(code)` 把它透到进程边界（systemd/容器据此判定）。
#![forbid(unsafe_code)]

use postern_daemon::boot;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // boot 启动链产出进程退出码 u8（boot 失败 → 非零、data.sock 不 serving；boot 成功 → 三平面
    // spawn 并留存 serve 至 SIGINT/SIGTERM 后优雅退出 → 0）。退出码语义在库面 boot::run 内确定，
    // main 只把它透到进程边界（公理二 fail-closed：systemd/容器据非零码判定启动失败、不误判已就绪）。
    let code = boot::run().await;
    std::process::exit(code as i32);
}
