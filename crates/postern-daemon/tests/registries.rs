//! registries 单元行为测试（RED）。
//!
//! 钉死 06 §8 对**适配器登记册**与**传输登记册**的承诺：
//! - 两表均以 `&'static str` 为键的确定性 `BTreeMap`（非 `HashMap`）；适配器以
//!   `protocol()`、传输以 `kind()` 为键。
//! - 由真实下游实现装配：`PostgresAdapter` / `DockerLogsAdapter` / `HttpAdapter` 按
//!   `protocol()` 登记；`DirectTransport` 按 `kind()="direct"` 登记（实现箱化为 trait 对象）。
//! - 选型未命中返回 `None`（上游映射为 fail-closed deny）。
//! - 一经装配即只读，仅以共享引用暴露（表本身无内部可变性）。
//!
//! 测试策略（06 §9）：登记册是纯数据结构、无 async，故本单元为同步表行为断言；
//! 选型助手的「命中 / 未命中」两路都钉一个确切结果（命中→Some 且键匹配，未命中→None）。
//! 雷区遵守：use core 的 `Adapter`/`Transport` trait 对象，不再实现具体适配器；Fake 仅为
//! 驱动登记册键序行为的最小实现（测试策略明示的 Fake 注入），零 SQL 标记、零 ConnOrigin 字面。

use std::collections::BTreeMap;

use async_trait::async_trait;

use postern_core::domain::{Capability, ConstraintSpec, ResolvedTarget, ResourceCredential};
use postern_core::error::{
    ClassifyError, ConstraintError, DiscoverError, ExecError, TransportError,
};
use postern_core::plugin::{Adapter, CapabilitySurface, Channel, RawResponse, Transport};
use postern_core::request::{ClassifiedIntent, Intent};

use postern_adapters::docker_logs::DockerLogsAdapter;
use postern_adapters::http::HttpAdapter;
use postern_adapters::postgres::PostgresAdapter;
use postern_transports::direct::DirectTransport;

use postern_daemon::registry::{AdapterRegistry, TransportRegistry};

// ----------------------------------------------------------------------------
// 真实下游实现装配出的两张登记册（boot 装配的同构路径）。
// ----------------------------------------------------------------------------

/// boot 装配点的真实适配器集合：三个唯一解释者箱化为 `Box<dyn Adapter>`。
/// 直接构造 `Box<dyn Adapter>` 即验收点「concrete impls boxed as trait objects」编译通过。
fn builtin_adapters() -> Vec<Box<dyn Adapter>> {
    vec![
        Box::new(PostgresAdapter),
        Box::new(DockerLogsAdapter),
        Box::new(HttpAdapter),
    ]
}

/// boot 装配点的真实传输集合：`DirectTransport` 箱化为 `Box<dyn Transport>`。
fn builtin_transports() -> Vec<Box<dyn Transport>> {
    vec![Box::new(DirectTransport::new())]
}

// ----------------------------------------------------------------------------
// §8 适配器登记册：真实实现按 protocol() 登记，命中返回同协议实现。
// ----------------------------------------------------------------------------

// §8 AdapterRegistry built from real downstream impls; each registers under its protocol().
#[test]
fn adapter_registry_registers_real_impls_under_their_protocol() {
    let reg = AdapterRegistry::new(builtin_adapters());

    let pg = reg
        .adapter_for("postgres")
        .expect("postgres adapter must be registered under its protocol() key");
    assert_eq!(
        pg.protocol(),
        "postgres",
        "命中实现的 protocol() 必与查键一致"
    );

    let dl = reg
        .adapter_for("docker_logs")
        .expect("docker_logs adapter must be registered under its protocol() key");
    assert_eq!(dl.protocol(), "docker_logs");

    let http = reg
        .adapter_for("http")
        .expect("http adapter must be registered under its protocol() key");
    assert_eq!(http.protocol(), "http");

    assert_eq!(reg.len(), 3, "恰三个真实适配器登记，无投机多余项");
    assert!(!reg.is_empty());
}

// §8 the registered real adapter is the same impl (engine_enforced fact carried through).
#[test]
fn adapter_registry_hit_returns_the_registered_impl_not_a_substitute() {
    let reg = AdapterRegistry::new(builtin_adapters());

    // postgres 为 SQL 类：engine_enforced 恒 true；容器/HTTP 类恒 false。登记册不得
    // 张冠李戴——命中实现须是登记进去的那一个，其引擎兜底事实原样可读。
    let pg = reg.adapter_for("postgres").expect("postgres present");
    assert!(
        pg.engine_enforced(),
        "postgres 是 SQL 类 → engine_enforced=true"
    );

    let dl = reg.adapter_for("docker_logs").expect("docker_logs present");
    assert!(
        !dl.engine_enforced(),
        "docker_logs 容器类 → engine_enforced=false"
    );

    let http = reg.adapter_for("http").expect("http present");
    assert!(!http.engine_enforced(), "http 类 → engine_enforced=false");
}

// §8 lookup miss returns None (caller maps to fail-closed deny).
#[test]
fn adapter_registry_miss_returns_none_for_fail_closed_deny() {
    let reg = AdapterRegistry::new(builtin_adapters());

    // 未登记协议必须是 None（不得回退到任意默认适配器）——上游据此 fail-closed deny。
    assert!(
        reg.adapter_for("mysql").is_none(),
        "未登记协议选型必须为 None（fail-closed deny 的前提）"
    );
    assert!(reg.adapter_for("").is_none(), "空键也不得命中");
    assert!(
        reg.adapter_for("Postgres").is_none(),
        "键区分大小写：PROTOCOL 是精确 &'static str，不做归一化"
    );
}

// §8 keys are &'static str protocol; BTreeMap => deterministic ascending iteration.
#[test]
fn adapter_registry_iterates_protocols_in_deterministic_btreemap_order() {
    let reg = AdapterRegistry::new(builtin_adapters());

    let protocols: Vec<&'static str> = reg.protocols().collect();
    // BTreeMap 按键升序，确定（非 HashMap 的随机序）：docker_logs < http < postgres。
    assert_eq!(
        protocols,
        vec!["docker_logs", "http", "postgres"],
        "协议键迭代必为 BTreeMap 升序（workspace 确定性纪律：BTreeMap 非 HashMap）"
    );
}

// §8 the table is &'static str-keyed — assembling from a Fake proves arbitrary protocol keying.
#[test]
fn adapter_registry_keys_each_impl_by_its_own_protocol() {
    // 两个 Fake 各报不同 protocol()；登记册必按各自 protocol() 落键，互不覆盖。
    let reg = AdapterRegistry::new(vec![
        Box::new(FakeAdapter { proto: "alpha" }) as Box<dyn Adapter>,
        Box::new(FakeAdapter { proto: "omega" }) as Box<dyn Adapter>,
    ]);

    assert_eq!(reg.len(), 2);
    assert_eq!(
        reg.adapter_for("alpha").expect("alpha keyed").protocol(),
        "alpha"
    );
    assert_eq!(
        reg.adapter_for("omega").expect("omega keyed").protocol(),
        "omega"
    );
    assert!(reg.adapter_for("beta").is_none());
}

// §8 registries are constructed once and exposed only by shared reference (read-only).
#[test]
fn adapter_registry_is_shared_by_reference_and_read_only() {
    let reg = AdapterRegistry::new(builtin_adapters());

    // 仅以共享引用暴露：多个 &reg 并存读取，选型不消耗也不改表（无内部可变性）。
    let view_a: &AdapterRegistry = &reg;
    let view_b: &AdapterRegistry = &reg;
    assert!(view_a.adapter_for("postgres").is_some());
    assert!(view_b.adapter_for("postgres").is_some());
    // 重复选型同键恒得同协议（只读、幂等）。
    assert_eq!(
        view_a.adapter_for("http").map(|a| a.protocol()),
        view_b.adapter_for("http").map(|a| a.protocol()),
    );
}

// ----------------------------------------------------------------------------
// §8 传输登记册：DirectTransport 按 kind()="direct" 登记；ssh/ssm feature 门控不在默认集。
// ----------------------------------------------------------------------------

// §8 DirectTransport registers under kind()="direct".
#[test]
fn transport_registry_registers_direct_under_its_kind() {
    let reg = TransportRegistry::new(builtin_transports());

    let direct = reg
        .transport_for("direct")
        .expect("DirectTransport must be registered under kind()=\"direct\"");
    assert_eq!(direct.kind(), "direct", "命中传输的 kind() 必与查键一致");

    // direct 非隧道直连：persistent 恒 false（连接管理层据此用毕即销）。事实须原样可读。
    assert!(!direct.persistent(), "direct 非长连接型 → persistent=false");

    assert_eq!(
        reg.len(),
        1,
        "默认集恰含 direct；ssh/ssm 为 feature 门控、不在默认集"
    );
    assert!(!reg.is_empty());
}

// §8 transport lookup miss returns None (caller maps to fail-closed deny).
#[test]
fn transport_registry_miss_returns_none_for_fail_closed_deny() {
    let reg = TransportRegistry::new(builtin_transports());

    // ssh/ssm 是 feature 门控、未编译进默认集 → 未登记 → None（不得回退默认传输）。
    assert!(
        reg.transport_for("ssh").is_none(),
        "ssh 未在默认集 → None（fail-closed deny 的前提）"
    );
    assert!(reg.transport_for("ssm").is_none(), "ssm 未在默认集 → None");
    assert!(reg.transport_for("").is_none(), "空键不得命中");
    assert!(reg.transport_for("Direct").is_none(), "键区分大小写");
}

// §8 transport keys are &'static str kind; BTreeMap => deterministic ascending iteration.
#[test]
fn transport_registry_iterates_kinds_in_deterministic_btreemap_order() {
    // 用两个 Fake 传输证明键序确定（单元素无法区分序）：BTreeMap 升序 "a" < "z"。
    let reg = TransportRegistry::new(vec![
        Box::new(FakeTransport { kind: "z" }) as Box<dyn Transport>,
        Box::new(FakeTransport { kind: "a" }) as Box<dyn Transport>,
    ]);

    let kinds: Vec<&'static str> = reg.kinds().collect();
    assert_eq!(
        kinds,
        vec!["a", "z"],
        "形态键迭代必为 BTreeMap 升序（确定性容器：BTreeMap 非 HashMap）"
    );
}

// §8 transport registry keys each impl by its own kind().
#[test]
fn transport_registry_keys_each_impl_by_its_own_kind() {
    let reg = TransportRegistry::new(vec![
        Box::new(FakeTransport { kind: "tunnel-x" }) as Box<dyn Transport>,
        Box::new(FakeTransport { kind: "tunnel-y" }) as Box<dyn Transport>,
    ]);

    assert_eq!(reg.len(), 2);
    assert_eq!(
        reg.transport_for("tunnel-x").expect("x keyed").kind(),
        "tunnel-x"
    );
    assert_eq!(
        reg.transport_for("tunnel-y").expect("y keyed").kind(),
        "tunnel-y"
    );
    assert!(reg.transport_for("tunnel-z").is_none());
}

// §8 transport registry is shared by reference and read-only.
#[test]
fn transport_registry_is_shared_by_reference_and_read_only() {
    let reg = TransportRegistry::new(builtin_transports());

    let view_a: &TransportRegistry = &reg;
    let view_b: &TransportRegistry = &reg;
    assert!(view_a.transport_for("direct").is_some());
    assert!(view_b.transport_for("direct").is_some());
    assert_eq!(
        view_a.transport_for("direct").map(|t| t.kind()),
        view_b.transport_for("direct").map(|t| t.kind()),
    );
}

// ----------------------------------------------------------------------------
// §8 两张登记册是 BTreeMap（结构性断言：键有序、确定）。
// ----------------------------------------------------------------------------

// §8 AdapterRegistry/TransportRegistry are BTreeMap-keyed (collect into BTreeMap roundtrips ordered).
#[test]
fn registries_are_btreemap_keyed_deterministic_containers() {
    let areg = AdapterRegistry::new(builtin_adapters());
    // 用 BTreeMap 收回键集再比对：键集合恰为三协议，且顺序由 BTreeMap 决定（确定）。
    let akeys: BTreeMap<&'static str, ()> = areg.protocols().map(|p| (p, ())).collect();
    let aordered: Vec<&'static str> = akeys.keys().copied().collect();
    assert_eq!(aordered, vec!["docker_logs", "http", "postgres"]);

    let treg = TransportRegistry::new(builtin_transports());
    let tkeys: BTreeMap<&'static str, ()> = treg.kinds().map(|k| (k, ())).collect();
    let tordered: Vec<&'static str> = tkeys.keys().copied().collect();
    assert_eq!(tordered, vec!["direct"]);
}

// ----------------------------------------------------------------------------
// 最小 Fake 实现（测试策略明示的 Fake 注入）：只为驱动登记册「按 self 报的键落键」行为。
// 仅 protocol()/kind() 被登记册触达；其余 trait 方法本单元永不调用（todo! 桩占位）。
// 雷区：use core 的 Adapter/Transport，不再实现具体适配器；零 SQL 标记、零机密构造。
// ----------------------------------------------------------------------------

struct FakeAdapter {
    proto: &'static str,
}

#[async_trait]
impl Adapter for FakeAdapter {
    fn protocol(&self) -> &'static str {
        self.proto
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[]
    }

    fn engine_enforced(&self) -> bool {
        false
    }

    fn classify(&self, _intent: &Intent) -> Result<ClassifiedIntent, ClassifyError> {
        // 登记册单元永不调用此路径（仅 protocol() 被触达）。
        todo!("not exercised by registries unit")
    }

    fn check_constraint(
        &self,
        _spec: &ConstraintSpec,
        _ci: &ClassifiedIntent,
    ) -> Result<bool, ConstraintError> {
        todo!("not exercised by registries unit")
    }

    async fn execute(&self, _ch: &mut Channel, _intent: &Intent) -> Result<RawResponse, ExecError> {
        todo!("not exercised by registries unit")
    }

    async fn discover(&self, _ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError> {
        todo!("not exercised by registries unit")
    }
}

struct FakeTransport {
    kind: &'static str,
}

#[async_trait]
impl Transport for FakeTransport {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn persistent(&self) -> bool {
        false
    }

    async fn open(
        &self,
        _target: ResolvedTarget,
        _cred: ResourceCredential,
    ) -> Result<Channel, TransportError> {
        // 登记册单元永不调用此路径（仅 kind() 被触达）；按值消费机密但绝不构造它们。
        todo!("not exercised by registries unit")
    }
}
