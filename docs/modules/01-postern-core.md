# 模块详细设计 · `postern-core`

> 本篇遵循《模块详细设计 · 索引与规约》（`00-模块详细设计-索引与规约.md`）规定的七小节统一结构。领域归属、属主与边界一律以《详细设计文档》第八部分（领域范围与内涵）为准；与上层行文冲突时，以第八部分的领域裁决与《技术设计文档》七公理为准。本篇为纯设计：接口签名作为"设计承诺"出现（与详细设计第四部分一致），不含任何实现逻辑代码块。

---

## 1. 定位（一句话）

`postern-core` 是系统全部安全语义共享的**词汇表、类型系统、全部插件 trait 的定义处，以及零 IO 的纯函数策略引擎**——它定义"领域是什么"和"求值如何裁决"，但不触碰任何 IO、不依赖工作区内任何其他 crate，被所有其他 crate 依赖。

---

## 2. 承载领域与职责范围

本 crate 承载《详细设计文档》第八部分的两个领域：

- **8.1 领域核心模型**（`postern-core::domain / request / decision`）—— 全部安全语义共享的词汇表与类型系统。
- **8.3 策略引擎**（`postern-core::eval`）—— 对单个已归一化请求、基于单一权威策略快照，产出确定性三值决策与完整求值轨迹的纯函数域。

附带承载两项跨领域的统一基础设施（其语义属主在第八部分中分别由领域模型与存储层裁决，载体落在 core）：

- **统一雪花 ID 生成器**（`postern-core::id::IdGen`）—— 全工作区唯一的 id 来源（契约 `DB_UNIFIED_ID_GENERATOR`、第八部分 8.0 速查表"id 生成 → core IdGen"）。
- **统一分页**（`postern-core::page` 的 `PageQuery` / `Page<T>`）—— 全部集合查询的唯一形态（契约 `DB_PAGINATION_MANDATORY`）。

**职责范围（封闭列举）**：

1. 定义领域类型词汇表（见第 5 节），及其代数关系与不变量的类型化表达。
2. 定义授权空间展开的纯语义（`Principal —绑定→ (Role × Scope)` 展开为 `Resource × Capability × 细则` 的判定空间）。
3. 定义全部插件 trait 的**接口签名**（实现归各域）。
4. 提供纯函数 `Evaluator`：编排求值步骤 `[1][3][5][6]`，消费 kernel 先行物化的归类/细则结果，产出 `Decision` 与 `EvalTrace`，并在 allow 时完成 tier 选择。
5. 定义各类错误枚举与"错误 → 拒绝阶段"的穷尽映射。
6. 提供 `IdGen` 雪花规格与 `PageQuery`/`Page<T>` 分页原语（含上限钳制）。

`postern-core` 自身**不持有任何可变运行时状态、不发起任何 IO、不依赖工作区内任何 crate**；它是契约测试可以纯内存驱动的求值核（公理三、契约 `ARCH_FORBIDDEN_EDGES`）。

---

## 3. 支持的功能

按对外接口组织，本 crate 对其余 crate 提供的能力：

### 3.1 领域类型词汇表（`domain / request / decision`）

- **参与者与对象**：`PrincipalId`、`ResourceCode`、`Role`、`Scope`、`Capability`（六动词，无 `Admin` 变体）、`CredentialTier`。
- **授权结构**：授权格（`Resource × Capability` 格子）、细则（constraints）、条件（conditions）的类型表达；`MatchedGrant`（命中的授权格）。
- **请求模型**：`NormalizedRequest`、`Intent`、`ClassifiedIntent`、`ConnOrigin`（仅 `UnixPeer{uid,gid}` / `Tcp{remote}` 两态）。
- **决策模型**：`Decision`（`Allow{grant,tier}` / `Deny` / `Escalate{fallback}`）、`DenyResponse`（结构化拒绝响应）、`EvalTrace`（求值轨迹）。

### 3.2 全部插件 trait 的定义

`Authenticator`、`Adapter`、`Transport`、`CredentialProvider`、`ConditionPredicate`、`AuditSink`、`Sanitizer`、`PolicyView`——**定义在 core，实现在各面 crate**（接口隔离点；与详细设计第四部分及附录 C 一一对应）。

### 3.3 纯函数求值

`Evaluator::evaluate(req, ci, constraint_check, policy, now)` —— 编排求值步骤 `[1][3][5][6]`，产出 `(Decision, EvalTrace)`；allow 时依快照 tier 声明完成动词→凭据等级选择；DenyResponse 的事实组装（reason / your_grants / request_hint / operator_note）全部取自快照。

### 3.4 错误 → 拒绝阶段映射

各域错误枚举（`AuthError` / `ClassifyError` / `ConstraintError` / `PredicateError` / `TransportError` / `CredentialError` / `ExecError` / `DiscoverError` / `AuditError` …）及其到拒绝阶段（`stage`）的穷尽映射，供审计 `stage` 字段与拒绝响应组装使用。

### 3.5 统一 ID 与分页

`IdGen`（雪花规格，时钟回拨拒绝生成）、`PageQuery`（含 `clamp` 上限钳制）、`Page<T>`（统一分页信封）。

---

## 4. 明确边界（不做什么）

每项排除指明其属主域（第八部分裁决）：

| 不做 | 归属域 / crate |
|---|---|
| 任何 IO 与副作用 | 各实现域（store/secrets/transports/adapters/daemon） |
| 策略状态的持久化与 schema 落地 | 存储层 `postern-store`（8.11） |
| `PolicySnapshot` 的构建与重建时机 | 存储层 `postern-store::snapshot`（8.11） |
| 策略状态的任何**变更** | 控制面 `postern-daemon::control`（8.10）；策略引擎对策略只读 |
| `ResolvedTarget` / `ResourceCredential` 的**构造** | 机密面 `postern-secrets`（8.8）；core 中仅有不透明声明，构造受契约 `SEC_CONSTRUCTION_SITES` 约束 |
| ScrubSet 的构造、更新与持有 | 机密面 `postern-secrets`（8.8） |
| 认证机制的**实现**（如何验证出示物） | 身份与凭证域，实现于 `postern-daemon`（8.4）；core 只定义 `Authenticator` trait |
| 协议解释（classify / check_constraint / execute / discover 的实现） | 适配器 `postern-adapters`（8.6）；core 只定义 `Adapter` trait |
| 通路获取与治理、tier 连接隔离 | 连接管理 `postern-daemon::connpool`（8.5） |
| 单条通路建立与保活 | 传输 `postern-transports`（8.7） |
| 决策的**执行**（把 Decision 翻译为执行或拒绝、出口脱敏的调用） | 数据面内核 `postern-daemon::kernel`（8.2） |
| `ConstraintCheck` 布尔事实的**得出**（跑 `Adapter::check_constraint`） | 数据面内核 `postern-daemon::kernel`（CONS-8）；core 只接收已物化的 `ConstraintCheck` 入参 |
| `ConnOrigin` 的**采集/构造** | 外壳层 listener（8.12）；策略引擎只消费 |
| 凭证元数据的持久化、签发/轮换/吊销的操作入口 | 存储层 / 控制面（8.4/8.10/8.11） |
| 审计事件的落地、脱敏的执行 | 存储层载体 / 数据面内核（8.9/8.2） |

**关于"tier 选择"的归属（与第八部分 8.0 速查表一致）**：本 crate **承载** tier 选择（动词→凭据等级映射，由 `evaluate` 在 allow 时产出 `Allow{tier}`，无匹配 tier → deny）；但 **tier 的声明**归策略状态（存储承载），**(资源, tier)→凭据解析**归机密面，**tier 连接隔离**归连接管理。core 只做"选哪个 tier"的纯语义判定，不解析凭据、不建连接、不触碰任何 tier 之外的连接生命周期。

---

## 5. 对外接口

以下为 `postern-core` 暴露给其他 crate 的类型与 trait（设计级签名，非实现）。标注"定义"者其实现在别处；标注"提供"者其逻辑由本 crate 给出（但仍零 IO）。签名是设计承诺，与详细设计第四部分一致。

### 5.1 领域类型（domain / request / decision）—— 本 crate **提供**定义

```rust
pub enum Capability { Observe, Query, Mutate, Execute, Manage, Destroy }
// 不存在 Admin 变体——"admin 不可授予"在类型层面成立(公理三;契约 SEC_ADMIN_NOT_GRANTABLE)

pub struct ResourceCode(String);      // 资源代号,如 "db-main";真实地址类型只存在于 secrets crate
pub struct PrincipalId(SnowflakeId);  // 雪花 ID,全工作区唯一来源是 core::id::IdGen
pub struct CredentialTier(String);    // 资源凭据等级名,如 "readonly"

/// 外壳归一化产物(步骤[0]输出)——自此请求与外壳无关(公理七)
pub struct NormalizedRequest {
    pub presented: PresentedCredential,   // Agent 出示的网关凭证(或本地进程上下文)
    pub origin: ConnOrigin,               // 网关可观测的连接来源,绝不采信自报字段
    pub resource: ResourceCode,
    pub intent: Intent,                   // 协议原始意图(SQL 文本/容器日志请求/模板调用...)
}

pub enum ConnOrigin {
    UnixPeer { uid: u32, gid: u32 },      // SO_PEERCRED:仅取 uid/gid 作信任域门
    Tcp { remote: SocketAddr },
}
// 不取 pid/exe 路径作身份依据——PID 可复用、/proc 读取存在 TOCTOU 伪冒面,均为可伪造特征。
// 构造权仅属外壳层 listener(契约 SEC_CONSTRUCTION_SITES);本域仅定义类型、由身份域消费校验。

/// 适配器语义归一化产物(步骤[2]输出)
pub struct ClassifiedIntent {
    pub capability: Capability,
    pub objects: Vec<ObjectRef>,          // 表/列、容器名、路径、模板 id...
}

pub enum Decision {
    Allow { grant: MatchedGrant, tier: CredentialTier },   // tier 由 evaluate 在 allow 时选定
    Deny(DenyResponse),
    Escalate { fallback: DenyResponse },  // 审批关闭时即取 fallback(恒 deny)
}

/// 结构化拒绝响应(技术设计文档 6.4;只含事实或人预写内容,公理六)
#[derive(Serialize)]
pub struct DenyResponse {
    pub decision: &'static str,           // "deny"
    pub denied: DeniedFacts,              // 已匿名化、已脱敏(脱敏由内核出口保证)
    pub reason: String,                   // 引用策略事实
    pub your_grants: BTreeMap<ResourceCode, Vec<String>>,
    pub request_hint: Option<String>,     // 由策略机械生成的 postern elevate 命令
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator_note: Option<String>,    // 资源所有者预写,原样转述
}
```

`EvalTrace` 记录"在哪一步、因何判定"，供审计 `stage`/`reason` 与拒绝响应组装；`DenyResponse` 字段全部取自快照、受 Scope 约束（只暴露该 Principal 自身授权世界，契约 `DENY_RESPONSE_SCOPE_BOUNDED`）。

`PresentedCredential` 属机密敏感族类型，遵循通用约定（不可 Clone/Serialize、`Debug=REDACTED`、无 `Display`）。

### 5.2 插件 trait —— 本 crate **定义**，实现在各面 crate

```rust
/// 认证器族(步骤[1])。实现:local_process / api_key / token;放大后增 mTLS / SSO(身份与凭证域,8.4)
pub trait Authenticator: Send + Sync {
    fn kind(&self) -> &'static str;
    fn authenticate(&self, presented: &PresentedCredential, origin: &ConnOrigin,
                    creds: &CredentialView, now: Timestamp) -> Result<PrincipalId, AuthError>;
    // 凭证无效/过期/吊销/可信域不符/来源无法判定 → Err → deny(公理二)
    // now 显式传入(对齐 Evaluator):凭证 expires_at/revoked_at/可信域时效在求值时刻按墙钟二次校验,
    // 过期即刻失效——不依赖后台 sweeper 时序(详细设计 6.2)
}

/// 适配器(步骤[2][4][8],发现于控制面触发)。实现:postgres / docker_logs / http(8.6)
#[async_trait]
pub trait Adapter: Send + Sync {
    fn protocol(&self) -> &'static str;
    fn capabilities(&self) -> &'static [Capability];
    /// 引擎级强制可用性:SQL 类为 true(凭据分级兜底),HTTP/容器类为 false(归类+细则是唯一防线)
    fn engine_enforced(&self) -> bool;
    fn classify(&self, intent: &Intent) -> Result<ClassifiedIntent, ClassifyError>; // Err → deny
    fn check_constraint(&self, spec: &ConstraintSpec, ci: &ClassifiedIntent)
        -> Result<bool, ConstraintError>;                                            // Err → deny
    async fn execute(&self, ch: &mut Channel, intent: &Intent) -> Result<RawResponse, ExecError>;
    async fn discover(&self, ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError>; // 发现≠授权
}

/// 传输(步骤[7b]取连接的底层)。实现:ssh / ssm / direct(8.7)
#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> &'static str;
    fn persistent(&self) -> bool;        // 长连接型→入池;非长连接型→用毕即销
    async fn open(&self, target: ResolvedTarget, cred: ResourceCredential)
        -> Result<Channel, TransportError>;
    // ResolvedTarget / ResourceCredential 由 daemon 从机密面取出注入,二者不实现 Clone/Serialize,
    // Debug 输出恒为 REDACTED,生命周期不出本调用(契约 SEC_SECRET_TYPE_DISCIPLINE)
}

/// 资源凭据来源(技术设计文档 10.6)。实现:静态保险箱;接口预留动态签发/证书(机密面,8.8)
#[async_trait]
pub trait CredentialProvider: Send + Sync {
    async fn credential_for(&self, res: &ResourceCode, tier: &CredentialTier)
        -> Result<ResourceCredential, CredentialError>;
}

/// 条件谓词(步骤[5],可扩展集合)。内置:rate_limit / time_window / mode / ttl
pub trait ConditionPredicate: Send + Sync {
    fn kind(&self) -> &'static str;
    fn eval(&self, ctx: &EvalContext, spec: &serde_json::Value) -> Result<bool, PredicateError>;
    // Err 或无法判定 → 视为不满足 → deny(公理二)
}

/// 审计写入(隔离点:append-only 自我观测 / 放大后防篡改实现)。实现:JsonlAuditSink(store,8.11/8.9)
pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent) -> Result<(), AuditError>;
}

/// 响应脱敏(步骤[9])。小响应整体脱敏;流式大输出走滑动重叠窗口(详细设计 6.4)
/// 实现:daemon::sanitize 应用机密面签发的 ScrubSet 不透明句柄 + 声明级 MaskRule
pub trait Sanitizer: Send + Sync {
    fn scrub(&self, payload: RawResponse, declared: &[MaskRule]) -> SanitizedResponse;
    fn scrub_stream(&self, declared: &[MaskRule]) -> Box<dyn StreamScrubber>;
}

/// 策略读取视图:求值面对的"单一权威策略状态"。实现:store::snapshot(8.11)
pub trait PolicyView: Send + Sync {
    fn snapshot(&self) -> Arc<PolicySnapshot>;   // 热生效机制见详细设计 6.2
}
```

机密类型在本域**仅有不透明声明**（`ResolvedTarget` / `ResourceCredential` / `PresentedCredential`），其构造权属机密面与外壳层（契约 `SEC_CONSTRUCTION_SITES` / `SEC_SECRET_TYPE_DISCIPLINE`）。core 内**不出现**任何这些机密类型的构造点。

### 5.3 求值器 —— 本 crate **提供**（纯逻辑，零 IO）

```rust
pub struct Evaluator { /* 持有 Authenticator 注册表、ConditionPredicate 注册表 */ }

/// 步骤[4]细则校验的结果,由 kernel 先行调用 Adapter::check_constraint 得出后传入(CONS-8)
pub struct ConstraintCheck { pub passed: bool /* false → evaluate 据此 deny */ }

impl Evaluator {
    /// 输入:归一化请求 + 适配器归类结果 + 细则校验结果 + 策略快照 + 墙钟;
    /// 输出:三值决策 + 完整求值轨迹
    pub fn evaluate(&self, req: &NormalizedRequest, ci: &ClassifiedIntent,
                    constraint: &ConstraintCheck,
                    policy: &PolicySnapshot, now: Timestamp) -> (Decision, EvalTrace);
}
```

`evaluate` 编排步骤 `[1]`（认证：调用 `Authenticator` 注册表）、`[3]`（RBAC 展开查表）、`[5]`（条件谓词逐一求值）、`[6]`（动作分流；审批关闭时 escalate 取 fallback 恒 deny）；allow 时依快照 tier 声明完成动词→凭据等级映射（无匹配 → deny）。步骤 `[2]`（classify）与 `[4]`（check_constraint）的**实现**在适配器，其结果 `ci` 与 `ConstraintCheck` 由 kernel 先行物化后传入——`evaluate` 因此仍是纯逻辑，契约测试可纯内存构造这两个入参驱动求值。

### 5.4 统一 ID 与分页 —— 本 crate **提供**

```rust
pub struct PageQuery { pub page_no: u32, pub page_size: u32 }   // page_no 从 1 起
pub struct Page<T> { pub items: Vec<T>, pub page_no: u32, pub page_size: u32, pub total: u64 }

impl PageQuery {
    pub const DEFAULT_SIZE: u32 = 20;
    pub const MAX_SIZE: u32 = 200;       // 全局上限:超出钳制到上限,不报错
    pub fn clamp(self) -> Self { /* page_no>=1, page_size ∈ [1, MAX_SIZE] */ }
}
```

`IdGen`（雪花规格，见第 7 节）—— 41 bit 毫秒时间戳（纪元 `2026-01-01T00:00:00Z`）+ 10 bit 节点号（config，默认 0）+ 12 bit 序列号；时钟回拨时拒绝生成。JSON 序列化时雪花 id 一律为**字符串**。

---

## 6. 与相邻模块的交互

> 依赖方向（权威依赖图，索引文档 3.2 / 契约 `ARCH_FORBIDDEN_EDGES`）：**`postern-core` 不依赖工作区内任何 crate；它被所有其他 crate 依赖。** 因此本节描述的全部交互方向均为"相邻 crate → core"（消费 core 的类型与 trait 定义，或调用 core 的纯函数），不存在任何"core → 工作区 crate"的边。下文按"定义在 core、实现/消费在各 crate"的关系逐一展开。

### 6.1 ← `postern-daemon`（组装点 / 数据面内核 / 连接管理 / 启动序列）

`postern-daemon` 是唯一组装点，对 core 的消费最密集，跨多个求值管线步骤。

- **方向**：daemon → core（依赖并调用 core 的纯函数与类型；core 反向无任何对 daemon 的引用）。
- **内容**：
  - 内核调用 `Evaluator::evaluate(req, ci, constraint_check, snapshot, now)`，传入 `NormalizedRequest`、`ClassifiedIntent`、`ConstraintCheck`、`Arc<PolicySnapshot>`、`Timestamp`，收回 `(Decision, EvalTrace)`。
  - 内核持有 core 定义的 `PolicyView`（取只读快照）、`AuditSink`、`Sanitizer`、`Adapter`/`Authenticator`/`ConditionPredicate` 注册表句柄；这些 trait 均定义于 core、实现于 daemon 或其依赖。
  - 连接管理层消费 `Decision::Allow{tier}` 中由 core 选定的 `(ResourceCode, CredentialTier)` 作为池键，并据此调用 `Transport::open`（trait 定义于 core）。
  - daemon 全部表主键与审计事件 id 取自 core `IdGen`；全部集合端点使用 core `PageQuery`/`Page<T>`。
- **时机**（对齐求值管线，详细设计 6.1 步骤映射）：
  - `[1]` 认证：`evaluate` 内部调用 `Authenticator::authenticate(..., now)`（注册表注入 `Evaluator`）。
  - `[2]` 语义归一化：kernel 先调 `Adapter::classify` 得 `ClassifiedIntent`（在 `evaluate` 之前）。
  - `[4]` 细则校验：kernel 先调 `Adapter::check_constraint` 物化为 `ConstraintCheck`（在 `evaluate` 之前，CONS-8）。
  - `[3][5][6]`：`evaluate` 内部完成 RBAC、条件谓词、动作分流，allow 时产出 `Allow{tier}`。
  - `[7]` 取连接：connpool 按 core 选定的 tier `acquire(resource, tier)`。
  - `[9]` 脱敏：内核出口调用 `Sanitizer::scrub` / `scrub_stream`。
  - `[7a]/[10]` 审计：内核调用 `AuditSink::record`。
  - 启动序列：daemon 注册各插件实现、构建 `Evaluator` 注册表。
- **失败语义（fail-closed）**：`evaluate` 一切错误路径解析为 `Deny`（公理二）；求值路径禁吞错（契约 `EVAL_NO_ERROR_SWALLOWING`，覆盖 `core::eval` 与 `daemon::kernel`）。`Adapter::classify`/`check_constraint` 返回 `Err`/`false` 时由 kernel 短路 deny，或传入 `passed=false` 的 `ConstraintCheck` 令 `evaluate` 据此 deny（二者等价）。`Authenticator::authenticate` 的任何 `Err` → `evaluate` 产出 deny。core 的判定确定性（`now` 显式传入、零随机、零隐式上下文）保证"相同输入 → 相同决策"，是审计可对账（`policy_rev`）的前提。

### 6.2 ← `postern-store`（存储层 · 8.11）

- **方向**：store → core（实现 core 定义的 trait，消费 core 的类型）。
- **内容**：
  - store 实现 `PolicyView`（`snapshot() -> Arc<PolicySnapshot>`）与 `AuditSink`（`JsonlAuditSink`）——二者 trait 定义在 core。
  - `PolicySnapshot` 的字段类型、`AuditEvent` 的类型、全部 policy.db 行映射的字段类型，引用 core 的领域词汇（`PrincipalId`/`ResourceCode`/`Capability`/`CredentialTier` 等）。
  - 全部表主键与审计事件 id 取自 core `IdGen`；全部集合查询函数接收 core `PageQuery`，返回 `Page<T>`。
- **时机**：
  - 控制面写入 / sweeper 回收 / import 协调 COMMIT 后，store 在同一写锁内重建 `PolicySnapshot` 并 `Arc` 原子替换；数据面 `evaluate` 经 `PolicyView::snapshot` 无锁读取（详细设计 6.2）。
  - 审计落地发生在求值管线 `[7a]/[10]`，由内核经 `AuditSink::record` 触达 store 载体。
- **失败语义**：core 只定义 trait 与类型，不参与 store 的 IO 失败处理；但 core 的契约约束 store——`IdGen` 时钟回拨拒绝生成（fail-closed，绝不产出可能重复 id）；`PageQuery::clamp` 钳制上限防无界查询（契约 `DB_PAGINATION_MANDATORY`）；`AuditSink::record` 返回 `Err` 时由内核执行"不可记 = 不放行"（处置归内核，8.2）。core 不向 store 暴露任何机密类型构造路径。

### 6.3 ← `postern-secrets`（机密面 · 8.8）

- **方向**：secrets → core（实现 core 定义的 trait，引用 core 的不透明机密类型声明）。
- **内容**：
  - secrets 实现 core 定义的 `CredentialProvider`（`credential_for(res, tier) -> ResourceCredential`）。
  - secrets 是 `ResolvedTarget` / `ResourceCredential` 类型的**唯一构造方**——core 中仅有这两个类型的不透明声明，secrets 持有其构造权（契约 `SEC_CONSTRUCTION_SITES`、`SEC_SECRET_TYPE_DISCIPLINE`）。
  - `credential_for` 的入参 `ResourceCode`、`CredentialTier` 是 core 词汇。
- **时机**：连接管理层在建立通路时一次性调用 `credential_for(res, tier)` 取得不透明 `ResourceCredential` 句柄（详细设计 6.3 步骤 `[7b]`）；tier 由 core 在 `evaluate` 的 allow 路径选定。
- **失败语义**：`credential_for` 返回 `Err`（配置缺失 / tier 无凭据）→ 连接无法建立 → deny（fail-closed，公理二）。core 通过类型设计保证机密零接触：`ResolvedTarget`/`ResourceCredential` 不可 Clone/Serialize、`Debug=REDACTED`，core 内不出现其构造点（公理四的编译期表达）。

### 6.4 ← `postern-transports`（传输 · 8.7）

- **方向**：transports → core（实现 core 定义的 `Transport` trait，消费 core 注入的不透明机密类型）。
- **内容**：
  - transports 实现 core 定义的 `Transport`（`kind` / `persistent` / `open(target, cred) -> Channel`）。
  - `open` 的入参 `ResolvedTarget` / `ResourceCredential` 是 core 中声明、secrets 构造的不透明类型——传输插件只在 `open` 调用生命周期内消费，不向上传递、不持久持有（凭据取用方向，详细设计 8.1/8.7）。
- **时机**：求值放行后步骤 `[7b]`，连接管理层在建立通路时把 `(target, cred)` 一次性传入 `Transport::open`。
- **失败语义**：`open` 返回 `Err(TransportError)` → 通路不可建立 → deny（fail-closed）。core 的类型纪律保证：机密生命周期不出 `open` 调用；通路死亡只上报、不自行重建（重建决策归连接管理，core 不参与）。

### 6.5 ← `postern-adapters`（适配器 · 8.6）

- **方向**：adapters → core（实现 core 定义的 `Adapter` trait，消费/产出 core 的请求与归类类型）。
- **内容**：
  - adapters 实现 core 定义的 `Adapter`（`protocol` / `capabilities` / `engine_enforced` / `classify` / `check_constraint` / `execute` / `discover`）。
  - `classify` 消费 core 的 `Intent`、产出 core 的 `ClassifiedIntent`（含 `Capability` 与 `Vec<ObjectRef>`）；`check_constraint` 产出布尔事实供 kernel 物化为 `ConstraintCheck`；`execute` 在 core 类型 `Channel` 上执行、返回 `RawResponse`。
  - 适配器是 `Intent` 负载格式的唯一解释者（8.0 速查表）；core 只定义 `Intent` 的装箱类型，不解释其内容。
- **时机**：步骤 `[2]`（`classify`，在 `evaluate` 之前）、`[4]`（`check_constraint`，在 `evaluate` 之前）、`[8]`（`execute`，在 allow 与取连接之后）；`discover` 仅由控制面触发（发现≠授权，8.6）。
- **失败语义**：`classify` 无法可靠归类 → `Err(ClassifyError)` → deny（白名单归类，宁可误拒，公理二）；`check_constraint` `false`/`Err` → deny；`execute` 的错误经内核出口脱敏后返回（已执行的请求绝不返回 deny，详细设计 6.1）。core 通过契约禁止 `adapters → secrets/transports/store` 依赖，保证适配器只见 `Channel`、不可达地址与凭据（`ARCH_FORBIDDEN_EDGES`）。

### 6.6 ← `postern-cli`（外壳客户端 · 8.12）

- **方向**：cli → core（仅消费 core 的共享类型，用于序列化/渲染控制面响应）。
- **内容**：cli 依赖 core 仅为**共享类型**——`PageQuery`/`Page<T>` 分页信封、`Decision`/`DenyResponse` 等需要在 CLI 侧反序列化与渲染的请求/响应/分页类型，以及雪花 id 的字符串序列化约定。cli 不依赖 `postern-store`/`postern-secrets`（契约 `ARCH_FORBIDDEN_EDGES` 禁止）。
- **时机**：每条管理命令 = 一次控制面 HTTP/JSON over UDS 调用 + 结果渲染；cli 用 core 类型解析 daemon 返回的 `Page<T>` 信封与结构化响应。
- **失败语义**：cli 是瘦客户端、零安全逻辑、零本地状态；它对 core 类型只做序列化与渲染，不参与任何求值或 fail-closed 判定（一切安全语义在内核，公理七）。core 不向 cli 暴露任何机密类型或求值入口。

---

## 7. 必守不变量

| 不变量 | 强制手段 |
|---|---|
| **零 IO、不依赖工作区任何 crate**（求值管线可被契约测试纯内存驱动） | 契约 `ARCH_FORBIDDEN_EDGES`（禁止 `core → 任何 IO crate`，扫描工作区依赖图）；`build.rs` 的 `FORBIDDEN_EDGES` 含 `postern-core → {store,secrets,transports,adapters,daemon}` |
| **`Capability` 枚举无 `Admin` 变体**（admin 在类型上不可表达，公理三） | 契约 `SEC_ADMIN_NOT_GRANTABLE`（扫描 `enum Capability` 体内不得出现 `Admin` 变体）+ 反例自检 `SEC_ADMIN_NOT_GRANTABLE_TEETH` |
| **机密类型在本域不可构造、不可 Clone/Serialize** | 契约 `SEC_SECRET_TYPE_DISCIPLINE`（`ResolvedTarget`/`ResourceCredential` 不得 derive 或手写 `Clone`/`Serialize`）；`SEC_CONSTRUCTION_SITES`（`ResolvedTarget`/`ResourceCredential` 只能在 `postern-secrets` 构造）——core 内不出现其构造点 |
| **求值路径禁吞错**（fail-closed 是代码规范，公理二） | 契约 `EVAL_NO_ERROR_SWALLOWING`（扫描 `core::eval` 路径禁 `.ok()` / `.unwrap_or(true)` / `.unwrap_or_default()` / `.unwrap_or_else(\|..\| true)` 等吞错放行写法）+ 反例自检 `EVAL_NO_ERROR_SWALLOWING_TEETH` |
| **`evaluate` 是纯函数、确定性**（`now` 显式传入；相同输入→相同决策；不持有可变状态） | 设计承诺（详细设计 8.3 必守不变量、技术设计 5.2 决策确定性）；`now`/`ConstraintCheck`/`ci` 全部经入参显式传入，无隐式上下文依赖 |
| **一切 `Err` 解析为 `Deny`**（公理二） | 设计承诺 + 上方 `EVAL_NO_ERROR_SWALLOWING`；错误→拒绝阶段为穷尽映射 |
| **`DenyResponse` 只含该 Principal 自身授权世界的事实或人亲笔预写内容**（公理六） | 设计承诺（契约 `DENY_RESPONSE_SCOPE_BOUNDED`，详细设计 8.3/6.4）；字段全部取自快照、受 Scope 约束 |
| **统一雪花 `IdGen` 是全工作区唯一 id 来源；时钟回拨拒绝生成** | 契约 `DB_UNIFIED_ID_GENERATOR`（封禁 uuid/ulid/nanoid 等替代 id 库）+ 反例自检 `DB_ID_GENERATOR_TEETH`；雪花 id JSON 序列化恒为字符串 |
| **`PageQuery` 上限钳制**（全部集合查询的唯一形态，禁无界查询） | 契约 `DB_PAGINATION_MANDATORY`（`PageQuery::MAX_SIZE = 200`，`clamp` 钳制；store 集合查询必接收 `PageQuery`）+ 反例自检 `DB_PAGINATION_TEETH` |
| **领域词汇与技术设计文档严格对齐** | 设计承诺（详细设计 8.1 必守不变量）；六动词集与正交语义、授权空间展开语义与技术设计第三部分一致 |
