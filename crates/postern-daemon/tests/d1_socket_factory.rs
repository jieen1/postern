//! D1 真实 socket 工厂（RealSocketFactory）行为测试（RED）。
//!
//! 钉死 [`RealSocketFactory`](postern_daemon::boot::real::RealSocketFactory)（模块文档 06 §8 L-1
//! / boot/sockets.rs）：在临时目录建两平面 UDS——
//! - `create_control` → control.sock 权限恰 **0600**（仅属主，[`CONTROL_PERMS`]）；
//! - `create_data` → data.sock 权限恰 **0660**（属主+组，[`DATA_PERMS`]）；
//! - **坏路径 → Err**（fail-closed）：不可写目录下创建必失败。
//!
//! 权限断言取 socket inode 的低 9 位 mode（`bind → 立即 chmod → listen` 原子序后的目标权限，
//! 无 umask 竞态窗口，L-1）。先红后绿：`create_*` 体 `unimplemented!()`，调用即 panic → 红。
//!
//! 雷区：本文件零 SQL 标记、不构造 `ConnOrigin`/`ResolvedTarget`/`ResourceCredential`；异步用
//! `#[tokio::test]`（socket 创建经 async 原语）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use postern_daemon::boot::real::RealSocketFactory;
use postern_daemon::boot::SocketFactory;

/// 进程唯一临时目录（无第三方 tempfile 依赖）。
fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("postern-d1-sock-{tag}-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// 取一个文件 inode 的低 9 位权限。
fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("socket inode must exist")
        .permissions()
        .mode()
        & 0o777
}

/// create_control 建 0600 control.sock，create_data 建 0660 data.sock（先 control 后 data）。
#[tokio::test]
async fn factory_creates_control_0600_and_data_0660() {
    let dir = temp_dir("perms");
    let control = dir.join("control.sock");
    let data = dir.join("data.sock");
    let factory = RealSocketFactory::new(control.clone(), data.clone(), None);

    factory.create_control().expect("create_control must Ok");
    assert_eq!(mode_of(&control), 0o600, "control.sock must be 0600");

    factory.create_data().expect("create_data must Ok");
    assert_eq!(mode_of(&data), 0o660, "data.sock must be 0660");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 坏路径（不存在的父目录）→ create_control Err（fail-closed，bind 失败短路）。
#[tokio::test]
async fn factory_bad_control_path_errs() {
    let dir = temp_dir("badctl");
    // 父目录不存在 → bind 失败。
    let control = dir.join("no-such-subdir").join("control.sock");
    let data = dir.join("data.sock");
    let factory = RealSocketFactory::new(control, data, None);

    assert!(
        factory.create_control().is_err(),
        "create_control on unbindable path must Err (fail-closed)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 坏路径（不存在的父目录）→ create_data Err（fail-closed）。
#[tokio::test]
async fn factory_bad_data_path_errs() {
    let dir = temp_dir("baddata");
    let control = dir.join("control.sock");
    let data = dir.join("no-such-subdir").join("data.sock");
    let factory = RealSocketFactory::new(control, data, None);

    assert!(
        factory.create_data().is_err(),
        "create_data on unbindable path must Err (fail-closed)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
