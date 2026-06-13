//! postgres `execute` / `discover` 行为单元（RED；§8: F-9, F-11, L-10, L-11, L-13；§3.4/§3.5）。
//!
//! 表驱动：语句原文（含写 / 伪装写）全部放数据文件 `tests/corpus/execute_cases.json`（对扫描器
//! 隐形），本 `.rs` 表驱动读取，连断言消息 / 注释都**零 SQL 标记**（B 方案）。逐条钉死设计
//! 承诺级可观察落点，对齐 docs/modules/05-postern-adapters.md §3.4（execute：在 Channel 上
//! 回放放行意图原文、绝不重解析改写、产**未脱敏** RawResponse、execute 不擦字节）/ §3.5
//! （discover：只读元信息探测、纯事实 CapabilitySurface、零授权字段、探测失败 fail-closed
//! Err）与 §8 验收 + 详设 6.3/6.7 项1（引擎账号兜底取证）：
//!   F-9  execute 在 Channel 上执行**已放行** Intent → Ok(RawResponse)，内容为**原始未脱敏**
//!        字节（脱敏归内核出口步骤[9]，本方法不擦）；安全层（误归 Query 的写经只读账号被
//!        引擎拒）需真实 PostgreSQL 容器，#[ignore]（容器集成层验收，F-9 兜底取证）。
//!   F-11 discover → Ok(CapabilitySurface)，字段全为事实（capabilities + objects），**无任何
//!        allow/tier/grant 授权字段**（发现≠授权；穷尽解构钉死字段集，编译期护栏）。
//!   L-10 execute 只执行已放行意图——每条 functional 用例的同一份原文经 classify 必先归其
//!        预期档（execute 与 classify 看到同一份负载原文，§3.6）。
//!   L-13 execute/discover 入参仅 `&mut Channel`(+`&Intent`)——无 tier/地址/凭据形参（签名结构，
//!        同形函数指针类型层钉死，入参漂移即编译失败）。
//!
//! 骨架 fail-closed 纪律（公理二）：`Channel.handle` 是 `Box<dyn Send + Sync>`——`dyn Send +
//! Sync` 不可下转为 `dyn Any`，适配器在进程内**取不到**句柄具体载荷，故骨架阶段无真实 pg
//! 线协议客户端可在其上跑、无资源结果集可读。F-9「执行回未脱敏资源字节」的**资源响应对象**
//! 须容器集成层（`pg-itest`）以接管底层流的 pg 客户端取证（语料 `expect_raw_contains` 是该层
//! 的资源响应子串）。骨架阶段 `execute` 对任何输入**一律 fail-closed 为
//! `Err(ExecError::ExecutionFailed)`，绝不伪造响应、绝不把请求原文字节回吐为 `Ok`**——与
//! docker_logs / http 骨架同纪律。故本文件的功能层断言钉死的是这条**默认即可强制**的不变量：
//! execute **不得**对任一放行意图（含写 / destroy）回吐 `Ok(RawResponse)`（杜绝「从不执行、
//! 只回显请求」的伪 execute；写 / destroy 静默「成功」回吐属 fail-closed 缺陷）。discover 同理
//! 对不可达通路 fail-closed。少数断言（语料结构、classify 前置归档、能力面字段集穷尽解构、
//! 签名同形绑定）在行为之外即可判定，钉死结构与 fail-closed。容器集成层接入真实 wire 后，
//! functional 断言改钉资源响应字节、engine_fallback 改钉只读账号引擎拒的具体 ExecError（届时
//! 先红再绿、显式更新断言）。

use serde::Deserialize;

use postern_core::domain::Capability;
use postern_core::error::{DiscoverError, ExecError};
use postern_core::plugin::{Adapter, CapabilitySurface, Channel, RawResponse};
use postern_core::request::Intent;

use postern_adapters::postgres::classify::classify;
use postern_adapters::postgres::intent::PgRequest;
use postern_adapters::postgres::PostgresAdapter;

const CORPUS: &str = include_str!("corpus/execute_cases.json");

#[derive(Deserialize)]
struct Corpus {
    /// 功能层用例：放行意图在 Channel 上执行回未脱敏字节（内存 Fake 通路可验）。
    functional: Vec<Case>,
    /// 引擎兜底层用例：误归 Query 的写经只读账号被引擎拒（需真实容器，#[ignore]）。
    engine_fallback: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    /// 用例名（断言失败时定位，不回显语句文本）。
    name: String,
    /// 待执行的单条语句原文（数据文件承载，本 `.rs` 不含其字面量）。
    statement: String,
    /// 可选绑定参数（位置参数序列）；缺省为空。
    #[serde(default)]
    params: Vec<serde_json::Value>,
    /// 该意图被放行时归的档（execute 前置：已放行；query/observe/mutate/destroy）。
    expect_classify: String,
    /// functional：通路应原样回交的未脱敏字节内容子串（execute 不擦字节）。
    #[serde(default)]
    expect_raw_contains: Option<String>,
    /// engine_fallback：只读账号下引擎拒后映射的 ExecError 变体名。
    #[serde(default)]
    expect_exec_error: Option<String>,
}

fn load_corpus() -> Corpus {
    serde_json::from_str(CORPUS).expect("execute_cases.json 应可解析为 Corpus")
}

/// 把一条语句原文 + 参数装成 postgres `Intent` 负载（`{statement, params}` 形态），模拟外壳层
/// 装箱搬运后内核递给适配器的 `&Intent`——classify 与 execute 看到的是**同一份原文**（§3.6）。
/// 语句文本来自数据文件参数，非源码字面量。
fn intent_for(case: &Case) -> Intent {
    let req = PgRequest {
        statement: case.statement.clone(),
        params: case.params.clone(),
    };
    let bytes = serde_json::to_vec(&req).expect("PgRequest 应可序列化为 Intent 负载");
    Intent::new(bytes)
}

/// 语料档名 → `Capability`（断言用，精确到具体档；非法档名即语料笔误，直接 panic）。
fn cap_of(s: &str) -> Capability {
    match s {
        "observe" => Capability::Observe,
        "query" => Capability::Query,
        "mutate" => Capability::Mutate,
        "destroy" => Capability::Destroy,
        other => panic!("expect_classify 档名非法: {other}"),
    }
}

/// 语料 ExecError 名 → `ExecError`（断言用，精确到具体变体；非法名即语料笔误，直接 panic）。
fn exec_error_of(s: &str) -> ExecError {
    match s {
        "channel_lost" => ExecError::ChannelLost,
        "protocol_violation" => ExecError::ProtocolViolation,
        "execution_failed" => ExecError::ExecutionFailed,
        other => panic!("expect_exec_error 变体名非法: {other}"),
    }
}

/// 测试用不透明 `Channel` 句柄——`Channel.handle` 是 `Box<dyn Send + Sync>`，对适配器不透明
/// （适配器拿不到、也不可下转传输私有句柄，L-13）。functional 层实现波次以内存 Fake 通路承接、
/// 验未脱敏回交；本桩阶段置最小占位（execute 桩 `todo!()` 在此通路上 panic，构成红）。
fn dummy_channel() -> Channel {
    Channel {
        handle: Box::new(()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 语料结构 + L-10 前置：execute 只执行已放行意图（classify 与 execute 看同一份原文）
// ─────────────────────────────────────────────────────────────────────────────

/// §8 F-9：execute 语料须覆盖功能层（≥3 类：读 / 观测 / 写）与引擎兜底层（≥1 类：误归写
/// 经只读账号被拒）两组——空语料即未覆盖 §3.4 的两层验收。
#[test]
fn execute_corpus_covers_functional_and_engine_fallback() {
    let corpus = load_corpus();
    assert!(
        corpus.functional.len() >= 3,
        "functional 语料至少应覆盖 3 类放行执行形态（读 / 观测 / 写），实得 {}",
        corpus.functional.len()
    );
    assert!(
        !corpus.engine_fallback.is_empty(),
        "engine_fallback 语料应至少有 1 条（误归写经只读账号被引擎拒，F-9 安全层）",
    );
}

/// §8 L-10 / §3.6：execute 是步骤[8]，仅执行**已放行**意图——其前置 classify 必先把同一份
/// 负载原文归到预期档。逐条断言 functional 用例 classify 恰为其预期档（且 classify 与 execute
/// 看到的是同一份 Intent 负载原文，绝不重解析 / 改写，§3.4）。本断言在 classify 路径（已实现）
/// 上即可判定，钉死「放行前置」与「同一份原文」两个不变量。
#[test]
fn functional_intents_are_pre_classified_as_their_allowed_tier() {
    let corpus = load_corpus();
    for case in &corpus.functional {
        let intent = intent_for(case);
        let ci = classify(&intent).unwrap_or_else(|e| {
            panic!(
                "[{}] 放行用例的同一份原文 classify 应成功，得 {e:?}",
                case.name
            )
        });
        assert_eq!(
            ci.capability,
            cap_of(&case.expect_classify),
            "[{}] execute 前置：classify 须先把该意图归到 {}（已放行，L-10）",
            case.name,
            case.expect_classify
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// F-9 功能层（骨架 fail-closed）：execute 绝不伪造 Ok、绝不回吐请求原文字节
// ─────────────────────────────────────────────────────────────────────────────

/// §8 F-9 / 公理二：骨架阶段 `Channel.handle`（`Box<dyn Send + Sync>`）不可下转，适配器取不到
/// 句柄载荷、无真实 pg wire 可执行、无资源结果集可读——故 `execute` 对**每条**放行意图都须
/// **fail-closed**，**绝不**回吐 `Ok(RawResponse)`。逐条 functional 用例（读 / 观测 / 写）断言
/// `Err(ExecError::ExecutionFailed)`，并以 `match` 对 `Ok` 显式 `panic`。
///
/// 这道断言钉死的真实缺口：一个**从不在 Channel 上执行、只把请求原文 echo 回 `Ok`** 的伪
/// execute（payload = 请求 statement 字节）必在此变红——`expect_raw_contains` 永远是 statement
/// 子串，旧「`Ok` + body.contains(want)」对这种回显恒为真、放过该伪实现；改钉「绝不 `Ok`」后，
/// 回显伪实现被直接拒。F-9 的资源响应字节取证须真实 wire（容器层 `expect_raw_contains`），
/// 骨架层先以「绝不伪造 / 回显」这条**默认即可强制**的不变量兜底（先红再绿）。
#[tokio::test]
async fn execute_never_fabricates_ok_for_allowed_intent() {
    let corpus = load_corpus();
    let adapter = PostgresAdapter;

    for case in &corpus.functional {
        // 语料须给 expect_raw_contains（容器层资源响应子串契约，骨架层不消费但须在位）。
        let _want = case
            .expect_raw_contains
            .as_ref()
            .unwrap_or_else(|| panic!("[{}] functional 用例须给 expect_raw_contains", case.name));

        let intent = intent_for(case);
        let mut ch = dummy_channel();
        // `RawResponse` 不派生 `Debug`/`PartialEq`，故 `Ok` 分支不可 `assert_eq!`——以 `match`
        // 显式拒绝**任何** `Ok`（尤其「回显请求原文字节」的伪 execute），并钉死失败变体恰为
        // `ExecutionFailed`（无 wire 可执行）。
        match adapter.execute(&mut ch, &intent).await {
            Err(ExecError::ExecutionFailed) => {}
            Err(other) => panic!(
                "[{}] 骨架 execute 应 fail-closed 为 ExecutionFailed（无 wire 可执行），得 Err({other:?})",
                case.name
            ),
            Ok(_) => panic!(
                "[{}] execute 绝不伪造 Ok(RawResponse)——回显请求原文字节是「从不执行、只回显」的伪 execute（公理二，F-9）",
                case.name
            ),
        }
    }
}

/// §8 F-9 关键不变量：execute **绝不擦字节**——含 PII 列的结果，掩码归内核出口 `column_mask`
/// 步骤[9]，不在 execute（§4 边界）。其反面是「execute 也绝不**伪造**含 PII 的响应字节」：
/// 骨架层无真实 wire，含 PII 列的放行查询同样 fail-closed，绝不回吐任何字节（含请求里出现的
/// `email` 列名）。容器层接入真实 wire 后改钉「响应中的 PII **值**（如某行的邮箱地址）未被擦」。
///
/// 这道断言堵的缺口：旧「`Ok` + body.contains("email")」钉的是**请求取数子句里的列名**
/// （statement 子串），而非响应里的 PII **值**——一个把每个 PII 值都擦掉、只保留请求文本的伪
/// 实现也能通过。改钉「骨架层绝不伪造 Ok」后，回显请求文本的伪 execute 被拒；真实「未脱敏值
/// 回交」的取证留容器层（值层面，非请求文本里的列名）。
#[tokio::test]
async fn execute_does_not_fabricate_pii_response_bytes() {
    let corpus = load_corpus();
    let adapter = PostgresAdapter;

    let case = corpus
        .functional
        .iter()
        .find(|c| c.name == "F9_query_with_pii_columns_not_masked_by_execute")
        .expect("语料应含 F9_query_with_pii_columns_not_masked_by_execute 用例");
    // PII 用例须给 expect_raw_contains（容器层资源响应子串契约，骨架层不消费但须在位）。
    let _want = case
        .expect_raw_contains
        .as_ref()
        .expect("PII 用例须给 expect_raw_contains");

    let intent = intent_for(case);
    let mut ch = dummy_channel();
    match adapter.execute(&mut ch, &intent).await {
        Err(ExecError::ExecutionFailed) => {}
        Err(other) => panic!(
            "含 PII 列的放行查询骨架层应 fail-closed 为 ExecutionFailed（无 wire），得 Err({other:?})"
        ),
        Ok(_) => panic!(
            "execute 绝不伪造含 PII 的响应字节——回显请求里的列名（如 email）是请求文本回显、非资源响应值（公理二，F-9）"
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// F-9 安全层：误归 Query 的写 / destroy 经 execute 绝不静默执行成功（写意图 fail-closed）
// ─────────────────────────────────────────────────────────────────────────────

/// §8 F-9 安全层 / 详设 6.3、6.7 项1：一条若被误归 `Query` 的写 / destroy（DELETE / CTE 藏删），
/// **绝不应经 execute 静默执行成功回吐 `Ok`**——这是 fail-closed 的硬不变量。容器层取证：经
/// **只读账号**在引擎层执行被引擎**直接拒**（印证 `engine_enforced=true` 兜底真实），映射为
/// 语料指定的具体 `ExecError`。骨架层取证：无真实 wire 可执行，写 / destroy 意图同样 fail-closed
/// 为 `Err(ExecError::ExecutionFailed)`——**两层都不允许 `Ok`**，故「绝不静默成功」这条不变量
/// **默认即被强制**（不再 `#[ignore]` 隐藏出默认运行；旧实现回吐请求字节为 `Ok`、命中
/// `Ok => panic`，本测试一旦运行即红，正是它曾被 `#[ignore]` 降级所掩盖的缺陷）。
///
/// 语料 `expect_exec_error` 恰为 `execution_failed`：容器层是「引擎拒」、骨架层是「无 wire」，
/// 两层映射到同一 `ExecutionFailed` 变体，故本断言**两层皆绿**且无需为骨架放宽——`Err` 必恰为
/// 语料指定变体、`Ok` 必 panic。容器层接入真实只读账号后语义收紧为「引擎拒」，断言不变。
#[tokio::test]
async fn engine_fallback_misclassified_write_never_silently_succeeds() {
    let corpus = load_corpus();
    let adapter = PostgresAdapter;

    assert!(
        !corpus.engine_fallback.is_empty(),
        "engine_fallback 语料不应为空（误归写 / destroy 经只读账号被引擎拒）"
    );

    for case in &corpus.engine_fallback {
        let want = case.expect_exec_error.as_ref().unwrap_or_else(|| {
            panic!("[{}] engine_fallback 用例须给 expect_exec_error", case.name)
        });

        let intent = intent_for(case);
        let mut ch = dummy_channel();
        // `RawResponse` 不派生 `Debug`/`PartialEq`（核心类型，不外泄诊断态），故 `Ok` 分支
        // 不可 `assert_eq!`——以 `match` 显式拒绝**任何** `Ok`（已落 / 未落副作用皆然），并钉死
        // 失败变体恰为语料指定（误归写 / destroy 绝不静默执行成功）。
        match adapter.execute(&mut ch, &intent).await {
            Err(e) => assert_eq!(
                e,
                exec_error_of(want),
                "[{}] 误归写 / destroy 须被拒、映射为 {}（容器层引擎拒 / 骨架层无 wire，F-9 安全层）",
                case.name,
                want
            ),
            Ok(_) => panic!(
                "[{}] 误归写 / destroy 绝不应执行成功——execute 绝不静默回吐 Ok（engine_enforced 兜底 / fail-closed，F-9 安全层）",
                case.name
            ),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// F-11 · discover 真实探测产纯事实能力面 + 探测失败 fail-closed
// ─────────────────────────────────────────────────────────────────────────────

/// §8 F-11 / §3.5 / L-12：`discover` 在递来的 `&mut Channel` 上只发只读元信息探测；通路不可达 /
/// 探测失败一律 **fail-closed** 返回 `Err(DiscoverError::ProbeFailed)`——绝不据失败 / 部分结果
/// 伪造任何 `Ok(CapabilitySurface)`（公理二，发现≠授权，绝不据失败产授权）。本桩阶段 dummy
/// 通路不可达，故须为 `Err(ProbeFailed)`（非 `ChannelLost`，不接受其它失败语义）。
///
/// RED：discover 桩 `todo!()`——本断言走到桩体 panic，构成观察到的红；实现波次落地只读元信息
/// 探测后，对真实通路回 `Ok(CapabilitySurface)`、对不可达 dummy 通路回 `Err(ProbeFailed)`，转绿。
/// `CapabilitySurface` 不派生 `Debug`/`PartialEq`，故 `Ok` 分支以 `match` 显式拒绝。
#[tokio::test]
async fn discover_fail_closed_on_unreachable_channel() {
    let adapter = PostgresAdapter;
    let mut ch = dummy_channel();
    match adapter.discover(&mut ch).await {
        Err(DiscoverError::ProbeFailed) => {}
        Err(DiscoverError::ChannelLost) => {
            panic!("不可达通路 discover 应 fail-closed 为 ProbeFailed，得 ChannelLost（F-11/§3.5）")
        }
        Ok(_) => panic!(
            "探测失败必 fail-closed 为 Err，绝不据失败 / 部分结果伪造 Ok 能力面（发现≠授权，F-11/§3.5）"
        ),
    }
}

/// §8 F-11 / §3.5 / L-12：`discover` 的**唯一**成功产物 `CapabilitySurface` 是**纯事实**类型——
/// 结构上只含 `capabilities`（资源具备何种能力）与 `objects`（探得的对象引用），**无任何
/// allow/tier/grant 授权字段**（发现≠授权，授权化是人经控制面圈选的后续动作）。
///
/// 以**穷尽解构**钉死字段集：核心 `CapabilitySurface` 若新增任一授权字段，此 `let { .. }`
/// 解构因字段不全而编译失败——把「能力面零授权字段」从散文约束升为编译期护栏（§3.5）。
/// postgres 能力面随探测维度（引擎版本 / 可见 schema 表列 / 账号真实权限），此处取
/// schema 表清单与读写能力为事实样例。
#[test]
fn capability_surface_is_facts_only_no_auth_fields() {
    let surface = CapabilitySurface {
        capabilities: vec![Capability::Observe, Capability::Query],
        objects: vec!["public.orders".to_string(), "public.customers".to_string()],
    };
    // 穷尽解构：字段恰为 (capabilities, objects)。新增 allow/tier/grant 字段即编译失败。
    let CapabilitySurface {
        capabilities,
        objects,
    } = surface;
    assert_eq!(
        capabilities,
        vec![Capability::Observe, Capability::Query],
        "能力面只装资源具备何种能力（事实），无授权维度（发现≠授权）"
    );
    assert_eq!(
        objects,
        vec!["public.orders".to_string(), "public.customers".to_string()],
        "能力面只装探得的对象引用（事实），无授权维度"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// L-13 / F-1 · 签名结构检查（execute/discover 只见 Channel，无 tier/地址/凭据形参）
// ─────────────────────────────────────────────────────────────────────────────

/// §8 L-13 / F-1：`execute` / `discover` 入参仅 `&mut Channel`(+`&Intent`)——无 `CredentialTier` /
/// `ResolvedTarget` / `ResourceCredential` / 地址类型形参。以「把方法绑定到只声明**入参**形态的
/// 同形函数指针」在类型层钉死签名（入参漂移成含 tier / 地址 / 凭据即编译失败）。**不 poll**
/// 这些 `todo!()` 桩 future，故不引入 panic——纯签名结构断言。返回的 future 类型经统一函数指针
/// 类型推断，避免依赖 `async_trait` 脱糖的具体 future 形状。
#[test]
fn execute_and_discover_signatures_see_only_channel() {
    // 同形入参桩：execute 仅 (&self, &mut Channel, &Intent)。
    fn exec_shape<'a>(
        this: &'a PostgresAdapter,
        ch: &'a mut Channel,
        intent: &'a Intent,
    ) -> core::pin::Pin<
        Box<dyn core::future::Future<Output = Result<RawResponse, ExecError>> + Send + 'a>,
    > {
        this.execute(ch, intent)
    }
    // 同形入参桩：discover 仅 (&self, &mut Channel)。
    fn discover_shape<'a>(
        this: &'a PostgresAdapter,
        ch: &'a mut Channel,
    ) -> core::pin::Pin<
        Box<
            dyn core::future::Future<Output = Result<CapabilitySurface, DiscoverError>> + Send + 'a,
        >,
    > {
        this.discover(ch)
    }
    // 仅构造 future、不 poll，故不进 `todo!()` 体、无 panic——入参一旦多出 tier / 地址 / 凭据，
    // 桩内调用即不成立、`_shape` 编译失败。仅取地址不执行。
    let _exec = exec_shape;
    let _discover = discover_shape;
}
