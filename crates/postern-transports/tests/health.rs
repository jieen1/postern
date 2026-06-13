//! 通路健康事实视图（channel-health 单元）行为测试（RED）。
//!
//! 被测对象：`postern_transports::health::{health_view, Health, HealthReader, HealthWriter}`
//! ——`Channel` 三件套之一的「健康事实视图」（§3.1 / §3.4）：一个**单调推进**的
//! 死活事实位枚举 + 写半 / 读半分离的共享同步原语句柄。供 pump / keepalive / chan
//! 单元写入死亡 / 关闭事实，供连接管理层**被动读取**当前事实（本域不主动 push、
//! 不调 daemon、无回调注册入口，§6.2）。
//!
//! 覆盖 §8 条目（逐条加注释）：
//! - F-4：初态读取返回 Alive；写入 Dead 后读取返回 Dead；写入 Closed 后读取返回
//!   Closed；健康事实不含真实地址 / 凭据 / 拓扑子串。
//! - L-8 / §7-8：Health 枚举及健康视图类型字段不承载机密——纯状态判别枚举的
//!   渲染输出无任何真实地址 / 凭据 / 拓扑子串（结构保证，非运行期擦除）。
//! - 单调性（§3.4 末、L-3/L-4）：Alive 可转 Dead/Closed；Dead/Closed 绝不翻回
//!   Alive（无 `mark_alive` 复活接口）；写入 Closed 后读取恒返回 Closed。
//! - 被动读取语义（§3.4 / §6.2）：读半提供同步查询当前事实的读接口，跨任务共享
//!   （Layer 0 无运行时任务依赖），多读不改状态。
//!
//! 本单元不构造机密类型（不写 `ResolvedTarget` / `ResourceCredential` /
//! `ConnOrigin` 字面），不嵌裸数据库写标记，不依赖兄弟单元。

use std::thread;

use postern_transports::health::{health_view, Health, HealthReader, HealthWriter};

/// 健康事实渲染中绝不允许出现的敏感子串清单（L-8 / §7-8）：真实地址、凭据、
/// 拓扑标识样本。健康事实位是无字段枚举，类型层即不可表达机密；本数组用于钉死
/// 「即便误把机密塞进类型，渲染也会暴露」这一回归面。本数组本身受全波次雷区约束，
/// 不含任何裸数据库写标记 / 机密类型字面。
const FORBIDDEN_SUBSTRINGS: &[&str] = &[
    "10.0.3.17",             // 详细设计点名的真实地址样本，绝不外泄
    "connection refused to", // 底层原始错误串片段
    "i-0abc",                // SSM instance_id 拓扑标识样本
    "5432",                  // 真实端口样本
    "readonly",              // tier / 账号样本
    "readwrite",             // tier / 账号样本
    "password",              // 凭据字段名样本
    "key_pem",               // 凭据字段名样本
];

// ── F-4：初态 / 写入 Dead / 写入 Closed 的读取事实 ────────────────────────

/// §8 F-4：健康视图初态读取返回 `Alive`。
#[test]
fn initial_read_returns_alive() {
    let (_writer, reader) = health_view();
    assert_eq!(reader.get(), Health::Alive);
}

/// §8 F-4：写入 Dead 后读取返回 `Dead`。
#[test]
fn mark_dead_then_read_returns_dead() {
    let (writer, reader) = health_view();
    writer.mark_dead();
    assert_eq!(reader.get(), Health::Dead);
}

/// §8 F-4：写入 Closed 后读取返回 `Closed`。
#[test]
fn mark_closed_then_read_returns_closed() {
    let (writer, reader) = health_view();
    writer.mark_closed();
    assert_eq!(reader.get(), Health::Closed);
}

// ── 单调性（§3.4 末、L-3/L-4）：绝不翻回 Alive，Closed 终态 ──────────────

/// §8 单调性：对初态 `Alive` 写入 `Dead` 后，写半仅余的推进接口
/// （`mark_dead` 幂等、`mark_closed` 推进终态）均不可把事实翻回 `Alive`
/// ——无 `mark_alive` 复活接口（L-3/L-4 核心不变量）。
#[test]
fn dead_never_flips_back_to_alive() {
    let (writer, reader) = health_view();
    writer.mark_dead();
    assert_eq!(reader.get(), Health::Dead);

    // 写半在 Dead 态下仅有的写操作：再 mark_dead（幂等）—— 仍不得翻回 Alive。
    writer.mark_dead();
    assert_eq!(reader.get(), Health::Dead);
    assert_ne!(reader.get(), Health::Alive);
}

/// §8 单调性：写入 `Closed` 后再写入 `Dead` 不得把终态降级——读取恒返回
/// `Closed`（Closed 是终态，Dead < Closed 不回退，L-3/L-4）。
#[test]
fn closed_is_not_downgraded_by_mark_dead() {
    let (writer, reader) = health_view();
    writer.mark_closed();
    assert_eq!(reader.get(), Health::Closed);

    writer.mark_dead();
    assert_eq!(reader.get(), Health::Closed);
    assert_ne!(reader.get(), Health::Dead);
}

/// §8 单调性 / §3.4：`Alive` 可推进到 `Dead`，`Dead` 可再推进到终态 `Closed`
/// （非降序推进合法）。
#[test]
fn dead_can_advance_to_closed() {
    let (writer, reader) = health_view();
    writer.mark_dead();
    assert_eq!(reader.get(), Health::Dead);

    writer.mark_closed();
    assert_eq!(reader.get(), Health::Closed);
}

/// §8 F-4 / §3.4：`Alive` 可被优雅关闭直接推进到 `Closed`（无须先经 Dead）。
#[test]
fn alive_can_advance_directly_to_closed() {
    let (writer, reader) = health_view();
    assert_eq!(reader.get(), Health::Alive);

    writer.mark_closed();
    assert_eq!(reader.get(), Health::Closed);
}

/// §8 单调性：`mark_dead` 幂等——多次写入 `Dead` 读取恒为 `Dead`，无副作用。
#[test]
fn mark_dead_is_idempotent() {
    let (writer, reader) = health_view();
    writer.mark_dead();
    writer.mark_dead();
    writer.mark_dead();
    assert_eq!(reader.get(), Health::Dead);
}

/// §8 单调性：`mark_closed` 幂等——多次写入 `Closed` 读取恒为 `Closed`，无副作用。
#[test]
fn mark_closed_is_idempotent() {
    let (writer, reader) = health_view();
    writer.mark_closed();
    writer.mark_closed();
    writer.mark_closed();
    assert_eq!(reader.get(), Health::Closed);
}

// ── 被动读取语义（§3.4 / §6.2）：同步查询、共享原语、多读不改状态 ────────

/// §8 被动读取语义：读半 `get()` 是被动同步查询，多次读取不改变事实位
/// （读不是状态转移，§3.4 只读快照）。
#[test]
fn repeated_reads_do_not_mutate_state() {
    let (writer, reader) = health_view();
    writer.mark_dead();
    assert_eq!(reader.get(), Health::Dead);
    assert_eq!(reader.get(), Health::Dead);
    assert_eq!(reader.get(), Health::Dead);
}

/// §8 被动读取语义：写半与读半共享同一事实位——经写半推进，读半立即可见
/// （共享原子 / watch 语义，§3.4）。
#[test]
fn reader_observes_writer_updates_through_shared_cell() {
    let (writer, reader) = health_view();
    assert_eq!(reader.get(), Health::Alive);
    writer.mark_dead();
    assert_eq!(reader.get(), Health::Dead);
}

/// §8 被动读取语义：写半可派生共享同一事实位的读半，派生读半观察到推进。
#[test]
fn writer_derived_reader_shares_fact() {
    let (writer, _reader) = health_view();
    let derived: HealthReader = writer.reader();
    assert_eq!(derived.get(), Health::Alive);
    writer.mark_closed();
    assert_eq!(derived.get(), Health::Closed);
}

/// §8 被动读取语义：读半可 `Clone`，克隆体共享同一事实位（多个连接管理读侧）。
#[test]
fn cloned_reader_shares_same_fact() {
    let (writer, reader) = health_view();
    let reader2 = reader.clone();
    writer.mark_dead();
    assert_eq!(reader.get(), Health::Dead);
    assert_eq!(reader2.get(), Health::Dead);
}

/// §8 被动读取语义 / Layer 0：底座是可跨任务共享的同步原语——写半移入另一线程
/// 写入后，主线程读半被动读到推进（不依赖 tokio runtime 任务，普通 `std::thread`
/// 即可证明跨任务共享性）。
#[test]
fn writer_is_shareable_across_threads() {
    let (writer, reader) = health_view();
    let handle: thread::JoinHandle<()> = thread::spawn(move || {
        let w: HealthWriter = writer;
        w.mark_dead();
    });
    handle.join().expect("writer thread must complete");
    assert_eq!(reader.get(), Health::Dead);
}

// ── L-8 / §7-8：健康事实不含机密（结构保证 + 渲染观察） ──────────────────

/// §8 L-8：`Health` 三态彼此可判别（事实位是纯状态枚举，无字段歧义）。
#[test]
fn health_variants_are_pairwise_distinct() {
    assert_ne!(Health::Alive, Health::Dead);
    assert_ne!(Health::Alive, Health::Closed);
    assert_ne!(Health::Dead, Health::Closed);
}

/// §8 L-8 / §7-8：`Health` 各变体的 `Debug` 渲染只含状态判别名，绝不含任何
/// 真实地址 / 凭据 / 拓扑子串——健康事实类型字段不承载机密（结构保证）。
#[test]
fn health_debug_contains_no_secret_substrings() {
    for variant in [Health::Alive, Health::Dead, Health::Closed] {
        let rendered = format!("{variant:?}");
        for needle in FORBIDDEN_SUBSTRINGS {
            assert!(
                !rendered.contains(needle),
                "Health::{variant:?} Debug rendering must not contain secret substring {needle:?}, got {rendered:?}",
            );
        }
    }
}

/// §8 L-8：经任意健康态推进后，读半返回的事实位 `Debug` 渲染仍不含机密子串
/// （行为观察：无论怎么写入，呈现的健康事实都不携带机密）。
#[test]
fn read_fact_rendering_contains_no_secret_substrings() {
    let (writer, reader) = health_view();
    writer.mark_dead();
    let fact = reader.get();
    let rendered = format!("{fact:?}");
    for needle in FORBIDDEN_SUBSTRINGS {
        assert!(
            !rendered.contains(needle),
            "read health fact Debug must not contain secret substring {needle:?}, got {rendered:?}",
        );
    }
}
