//! D1 真实可连 uid 探针（RealUidProbe）行为测试（RED）。
//!
//! 钉死 [`RealUidProbe`](postern_daemon::boot::real::RealUidProbe)（模块文档 06 §3.1·6 / §8 F-2）：
//! - `self_uid()` == 进程自身 uid（经 SO_PEERCRED 安全 API 取，无 unsafe / 无 libc 直调）；
//! - `connectable_uids()` 行为：返回 data.sock 在当前环境下**除 owner 自身以外**的他者可连 uid
//!   **有效集合**。owner 能连其自建 socket 是平凡真、**不是** F-2 要测的东西；F-2 测的是「有没有
//!   **别的**主体（Agent）与 daemon 同 uid」——故 owner 自身的 uid 绝不在此集合内（否则
//!   `connectable_uid_check` 必恒 RefuseSameUid、永不开放 data.sock）。D1 无 Agent 经专用组/ACL 被
//!   推导为可连他者，故该集合为空。
//!
//! 期望 uid 在测试侧**独立**经 `tokio::net::UnixStream::pair()` + `peer_cred().uid()` 求出（与被
//! 测实现同一安全来源、互为旁证），不引 libc / 不写 unsafe。先红后绿：探针体 `unimplemented!()`，
//! 调用即 panic → 红。
//!
//! 雷区：本文件零 SQL 标记、不构造 `ConnOrigin`/`ResolvedTarget`/`ResourceCredential`；异步用
//! `#[tokio::test]`（self-pair / peer_cred 经 async UDS）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::net::UnixStream;

use postern_daemon::boot::real::RealUidProbe;
use postern_daemon::boot::ConnectableUidProbe;

/// 进程唯一临时目录（无第三方 tempfile 依赖）。
fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("postern-d1-uid-{tag}-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// 经 SO_PEERCRED 安全 API 独立求进程自身 uid（与被测实现同一来源，互为旁证；无 unsafe）。
async fn expected_self_uid() -> u32 {
    let (a, _b) = UnixStream::pair().expect("UnixStream::pair");
    a.peer_cred().expect("peer_cred on self-pair").uid()
}

/// self_uid() 恰等于进程自身 uid（SO_PEERCRED 安全 API，无 libc 直调）。
#[tokio::test]
async fn self_uid_equals_process_uid() {
    let dir = temp_dir("self");
    let probe = RealUidProbe::new(dir.join("data.sock"));

    assert_eq!(
        probe.self_uid(),
        expected_self_uid().await,
        "self_uid must equal the process's own uid (via SO_PEERCRED)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// connectable_uids() **绝不含**自身 uid（F-2：该集合是「除 owner 外的他者可连 uid」——含自身
/// 即「别的主体与 daemon 同 uid」的危险态，会令自检恒 RefuseSameUid、永不开放 data.sock）。
/// D1 无 Agent 经专用组/ACL 被推导为可连他者，故该集合为空。
#[tokio::test]
async fn connectable_uids_excludes_self() {
    let dir = temp_dir("conn");
    let probe = RealUidProbe::new(dir.join("data.sock"));

    let connectable = probe.connectable_uids();
    let me = expected_self_uid().await;
    assert!(
        !connectable.contains(&me),
        "owner's own uid must NOT appear in the other-principal connectable set (F-2): \
         otherwise the self-check would always RefuseSameUid and data.sock would never open"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
