# postern-adapters · 模块详细设计

> 本篇是 `postern-adapters` 的模块级详细设计，在《详细设计文档》第八部分 **8.6 适配器** 的领域裁决之上展开本 crate 的定位、职责、功能、边界、与相邻模块的交互细节与必守不变量。与上层冲突时，以《技术设计文档》七公理与《详细设计文档》第八部分为准。纯设计，不含实现代码、阶段划分或进度状态。

---

## 1 · 定位（一句话）

`postern-adapters` 是各类资源协议的**唯一解释者**：它把某种协议的原始意图翻译为 `(Capability 动词, 操作对象)`，按资源声明的细则校验该归类，并在连接管理层递来的、不透明的 `Channel` 上执行已被求值放行的意图——它解释协议、不做授权，见通路、不见传输与地址。

本 crate 是库（无二进制），其 `Adapter` 实现以模块 + cargo feature 形态共存：`postgres` / `docker_logs` / `http`（《详细设计文档》3.2、4.4）。

---

## 2 · 承载领域与职责范围

**承载领域**：《详细设计文档》**8.6 适配器**（语义边界），载体为 `crates/postern-adapters/`（库 crate）。

**职责范围（封闭列举）**：

1. **`classify`——协议感知语义归一化**：把协议原始 `Intent` 翻译为 `ClassifiedIntent { capability, objects }`。SQL 类为语法树级归类，**按语句树内出现的最高危写节点定档**（见 §3.1）；无法可靠归类一律 `Err`（公理二）。
2. **细则语义的定义与 `check_constraint`**：定义本协议支持的各细则 `kind`（按资源类型：`table_allow`/`column_mask`、`container_prefix`、`http_route`、`command_template`/`script_template`/`path_whitelist`、`key_prefix`/`command_class`、`vhost_allow`/`queue_prefix`/`route_allow`、`mask_fields`——完整语义见 §3.2）的语义，并对一个已物化的 `ClassifiedIntent` 按 `ConstraintSpec` 判定通过/不通过。
3. **`execute`**：在连接管理层提供的 `Channel` 上执行**已被求值放行**的 `Intent`，产出未脱敏的 `RawResponse`。
4. **能力声明与 `engine_enforced` 声明**：经 `protocol()` / `capabilities()` 申明协议名与可承载动词集；经 `engine_enforced()` 如实申明该协议是否存在引擎级强制兜底（SQL 类 `true`，HTTP/容器类 `false`，见 §3.3）。
5. **Intent 负载格式的定义**（速查表"Intent 负载格式 → 适配器（唯一解释者）"）：定义本协议 `Intent` 的结构，**含 MCP 动词工具的参数 schema**——即 `postern_query` / `postern_mutate` 等工具向 Agent 暴露的 `(resource, request...)` 中 `request` 的形态。
6. **`discover`——能力面探测的执行**：真实连上资源探测其能力面，产出 `CapabilitySurface`。**仅接受控制面触发**（接入侧探测），发现≠授权。
7. **伪装攻击识别**：在 `classify` 内识别写 CTE 包裹、子查询藏写、多语句、注释混淆、`DO` 块、`SET` 会话语义篡改等伪装形态，统一收敛为正确归类或 `Err`。

---

## 3 · 支持的功能

按 `Adapter` trait 的对外方法组织。

### 3.1 `classify`：归类规约

`classify(intent) -> Result<ClassifiedIntent, ClassifyError>`，`Err → deny`。各内置实现的归类规约：

**postgres（SQL，语法树级）**（《详细设计文档》4.4 落点 D5）：

```
SQL 文本 → sqlparser(PostgreSqlDialect) → Vec<Statement>
  ├─ 解析失败 / 多语句 / 未知节点                 → ClassifyError → deny
  ├─ 按语句树内出现的「最高危写节点」定档（遍历整棵语句树：含 CTE、子查询、INTO 目标）：
  │    任意位置出现 Delete/Drop/Truncate          → Destroy
  │    任意位置出现 Insert/Update/Merge           → Mutate
  │    纯只读 Query（无任何写节点、无 INTO）        → Query
  │    Show / Explain（不带 ANALYZE）             → Observe
  │    其余一切语句类型（Set/DO/COPY/CALL/...）     → 不可靠归类或改变会话语义 → deny
  └─ 对象提取：遍历 AST 收集 schema.table 与列     → 供细则校验与审计 objects
```

定档原则与禁降级语义（与 4.4 严格一致）：

- **按整棵语句树内最高危写节点定档，而非顶层语句类型**。危险动词在整棵树范围内可见，CTE 或子查询里的 `DELETE`/`DROP`/`TRUNCATE` 一律提升为 `Destroy`，**绝不因被只读外壳包裹而降级**（如 `WITH x AS (DELETE ...) SELECT ...` 仍判 `Destroy`，绝不降为 `Mutate` 或 `Query`）。
- **`SET` 一律拒绝、不设白名单逃生口**：`SET` 改变会话语义（search_path、role、超时等），不属只读观测；放行的 `SET` 无归属动词将使 RBAC 步骤[3]无格可判（违 fail-closed）。会话语义的调整（如设定只读会话）由适配器在**建立连接时**统一施加，绝不接受 Agent 的 `SET` 请求。
- **`EXPLAIN` 仅允许不带 `ANALYZE` 的形态归为 `Observe`**：`EXPLAIN ANALYZE` 会真实执行被解释语句，按其内部最高危写节点定档（含写节点即非 `Observe`）。
- **白名单之外宁可误拒**（公理二）：归类是显式枚举的语句形态白名单，未知/歧义/多语句一律 `deny`；归类正确性的强制兜底是凭据分级（`Query` 恒走只读账号，引擎层拒绝一切写入，见 §3.3）。

**docker_logs（容器日志，只读取数）**：`Intent` 为容器日志请求（容器选择 + 取数范围），恒归类为 `Observe`；其取数形态与远端探针/直连差异见 §3.4。

**http（HTTP API）**：按声明的动词工具与路径将请求归类为相应 `Capability`；`engine_enforced=false`，归类 + 细则是唯一防线。

### 3.2 `check_constraint`：细则语义

`check_constraint(spec, ci) -> Result<bool, ConstraintError>`，`false / Err → deny`。本 crate 是各 `kind` 语义的属主：

每个 `kind` 由其属主 Adapter 定义语义（与《详细设计文档》5.2 `grant_constraints.kind` 枚举一一对应）：

| 细则 `kind` | 属主 adapter | 语义（适配器定义并判定） |
|---|---|---|
| `table_allow` | postgres/mysql | 归类提取的 `objects` 中的 schema.table 必须全部落在 `spec.tables` 白名单内 |
| `column_mask` | postgres/mysql | 声明 query/mutate 不得触及的敏感列（区别于 `mask_fields` 的响应擦除——本 kind 在求值期拒绝触达） |
| `container_prefix` | docker | 目标容器名必须匹配 `spec` 声明的前缀 |
| `http_route` | http | 请求 `(method, path)` 必须落在 `spec` 路由白名单内（即"接口维度限制"——可对读/写分别声明不同路径） |
| `command_template` | command | 请求必须匹配预声明的参数化命令模板，模板外的自由命令一律不通过 |
| `script_template` | command/deploy | `execute` 只能实例化 `spec` 声明的脚本模板集合（"某些脚本"） |
| `path_whitelist` | command/deploy | 模板中的目标路径参数必须落在 `spec` 目录白名单内（"某些目录"） |
| `key_prefix` | redis | 命令触及的 key 必须匹配 `spec` 前缀白名单 |
| `command_class` | redis | 命令必须落在 `spec` 声明的命令类白名单内（如仅 `@read`） |
| `vhost_allow` | rabbitmq | 操作的 vhost 必须落在 `spec` 白名单内 |
| `queue_prefix` | rabbitmq | 目标队列名必须匹配 `spec` 前缀 |
| `route_allow` | rabbitmq | 发布/绑定的 routing key 必须落在 `spec` 白名单内 |
| `mask_fields` | 任意（声明形态） | 声明需脱敏的列/字段名；**由内核出口的 `Sanitizer` 执行擦除**（适配器只定义声明形态，不执行脱敏，见 §4） |

合并语义遵循《详细设计文档》5.2"约束合并"：同 `kind` 多行默认取交集（全部须满足，fail-closed）；不同 `kind` 之间恒 `AND`；任一 `kind` 若采"白名单并集"语义须在其 `spec` 文档显式声明扩权特性（默认不取并集）。

### 3.3 能力声明与 `engine_enforced`

`protocol() -> &'static str`、`capabilities() -> &'static [Capability]`、`engine_enforced() -> bool`：

- **`engine_enforced = true`（SQL 类，如 postgres）**：存在**引擎级强制兜底**——`Decision::Allow{tier}` 选定的凭据等级在数据库引擎账号层受真实权限约束，即便 `classify` 把一条伪装写误归为 `Query`，它也只走只读账号、被引擎拒绝。归类是第一道线，引擎账号是强制兜底。
- **`engine_enforced = false`（HTTP/容器类，如 http、docker_logs）**：不存在引擎级账号分级，**归类 + 细则是唯一防线**。此差异须在该 `Adapter` 的文档与 `engine_enforced()` 返回值中**如实标注**（公理三、6 节失败语义对齐）。

### 3.4 `execute`

`async execute(ch, intent) -> Result<RawResponse, ExecError>`：在 `Channel` 上执行**已被求值放行**的 `Intent`，产出未脱敏 `RawResponse`（脱敏由内核出口完成，见 §4）。

- **只执行已放行意图**：`execute` 是管线步骤[8]，前置步骤[1]~[6] 求值放行、[7a] 意图审计、[7b] 取连接均已成立后才被调用（见 §6.1）。
- **错误经脱敏后返回，已执行请求绝不返回 deny**：`ExecError` 沿出口经 `Sanitizer` 返回；对有副作用动词，一旦底层已落库副作用，绝不再返回 deny（《详细设计文档》6.1 时序不变量，由内核守护）。
- **会话副作用形态**：对存在会话副作用、无法可靠净化的请求形态，连接管理层将禁用连接复用（即建即用即弃）——该决策属连接管理（8.5），适配器经 `Channel` 抽象消费，不参与池化判定。

### 3.5 `discover`：能力面探测

`async discover(ch) -> Result<CapabilitySurface, DiscoverError>`：真实连上资源探测其能力面。

- **仅控制面可触发**（速查表"能力面发现 → 适配器执行，仅控制面可触发"）：经 `daemon::control` 的 `POST /v1/resources/{code}/discover` 进入，是接入侧探测，供运维圈选授权。
- **发现≠授权**：`discover` 只产出**事实**（资源具备何种能力），绝不产生任何授权；授权化由人经控制面圈选。
- **与数据面 `postern_surface` 严格区分**（CONS-20）：数据面 `postern_surface` 是授权快照的纯事实投影，**不触达底层资源、不调用 `Adapter::discover`**；二者命名边界由命名规范固化，禁止互相借用。

### 3.6 Intent 负载格式

本 crate 定义各协议 `Intent` 的结构，并定义 MCP 动词工具的参数 schema（`postern_query` / `postern_observe` / `postern_mutate` / `postern_execute` / `postern_manage` / `postern_destroy` 的 `request` 形态）。外壳层只把该负载**忠实装箱搬运、不增不减、不解释**（8.12 范围外）；解释权唯一归适配器。

---

## 4 · 明确边界（不做什么）

| 不做的事 | 归属域 / crate |
|---|---|
| 授权判定（allow/deny/escalate） | **策略引擎** `postern-core::eval`（适配器只产出归类，不产出决策） |
| `CredentialTier` 选择（动词→tier） | **策略引擎**（`evaluate` 产出 `Allow{tier}`；适配器既不选 tier 也不感知 tier） |
| 通路的获取、池化、健康、重建、退避、回收 | **连接管理** `postern-daemon::connpool`（适配器只见 `Channel`，不知通路从何而来） |
| 通路如何建立、保活、关闭 | **传输** `postern-transports`（适配器不依赖、不感知 transport 种类） |
| 资源凭据与真实地址 | **机密面** `postern-secrets`（契约禁止 `adapters → secrets/transports/store`） |
| `(res, tier) → 凭据` 解析、`代号→ResolvedTarget` 解析 | **机密面**（适配器无任何取用路径） |
| 响应脱敏的执行（含 `mask_fields` 擦除、错误/拒绝脱敏） | **数据面内核出口** `postern-daemon::kernel` + `Sanitizer`（适配器只定义 `mask_fields` 声明形态） |
| `ScrubSet` 的构造/持有 | **机密面**（8.8） |
| 发现结果的授权化 | **人，经控制面圈选**（发现≠授权） |
| 触发 `discover` 的权限 | **控制面** `postern-daemon::control`（适配器只执行、不决定何时触发） |
| 意图审计、结果审计、审计时序 | **观测面/内核**（适配器不落审计；objects 供审计消费） |
| 策略状态的读写、快照构建 | **存储层** `postern-store` / **控制面**（适配器对策略无任何路径） |
| 接收/校验 Agent 自报来源、构造 `ConnOrigin` | **外壳层 listener**（8.12；适配器不接触来源采集） |

---

## 5 · 对外接口

本 crate 的**对外接口是对 `core` 中 `Adapter` trait 定义的实现**——trait 的**定义**归 `postern-core::plugin`（8.1），本 crate 提供**实现**（postgres / docker_logs / http，feature 门控）。设计承诺级签名（《详细设计文档》4.1，与第四部分一致）：

```rust
/// 适配器(步骤[2][4][8])。实现:postgres / docker_logs / http(本 crate 提供实现)
#[async_trait]
pub trait Adapter: Send + Sync {
    fn protocol(&self) -> &'static str;
    fn capabilities(&self) -> &'static [Capability];
    /// 引擎级强制可用性:SQL 类为 true(凭据分级兜底),HTTP/容器类为 false(归类+细则是唯一防线)
    fn engine_enforced(&self) -> bool;
    fn classify(&self, intent: &Intent) -> Result<ClassifiedIntent, ClassifyError>;     // Err → deny
    fn check_constraint(&self, spec: &ConstraintSpec, ci: &ClassifiedIntent)
        -> Result<bool, ConstraintError>;                                                // false/Err → deny
    async fn execute(&self, ch: &mut Channel, intent: &Intent) -> Result<RawResponse, ExecError>;
    async fn discover(&self, ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError>; // 发现≠授权
}
```

消费的 `core` 类型（**定义在 core**，本 crate 只读消费）：`Intent`、`ClassifiedIntent`、`Capability`、`ObjectRef`、`ConstraintSpec`、`Channel`、`RawResponse`、`CapabilitySurface`，以及各错误枚举 `ClassifyError`/`ConstraintError`/`ExecError`/`DiscoverError`。

本 crate **额外定义**（属适配器域、其他域只读消费）：各协议 `Intent` 负载的具体结构与 MCP 动词工具参数 schema、各细则 `kind` 的 `spec` 语义形态。

错误模型：本 crate 持一个 `thiserror` 错误枚举；其变体到"拒绝阶段"的穷尽映射在 `core::error` 维护（新增错误变体不写映射无法编译，《详细设计文档》7.1）。

---

## 6 · 与相邻模块的交互

依赖事实（权威依赖图 3.2）：本 crate **仅依赖 `postern-core`**（消费领域类型与 `Adapter` trait 定义）。本 crate **被 `postern-daemon` 依赖**（daemon 是唯一组装点）。本 crate **禁止依赖** `postern-secrets` / `postern-transports` / `postern-store`（契约 `ARCH_FORBIDDEN_EDGES` 强制；build.rs `FORBIDDEN_EDGES` 列 `("postern-adapters", &["postern-secrets","postern-transports","postern-store"])`）。

下文逐一展开每条交互边。**注意**：适配器与机密面、传输层、存储层之间**不存在任何依赖边**——下文不描述、也不允许任何被禁止的交互。

### 6.1 ← `postern-daemon::kernel`（被调用：classify / check_constraint / execute）

- **方向**：`daemon::kernel` 调用本 crate 的 `Adapter`。
- **内容与时机**（对齐《详细设计文档》6.1 求值管线 → 代码映射）：
  - **步骤[2] 语义归一化**：kernel 传入 `&Intent`（自 `NormalizedRequest` 取得），调 `classify`，收回 `ClassifiedIntent { capability, objects }`。此结果随后供 `Evaluator` 步骤[3] RBAC 查表与步骤[4] 细则使用。
  - **步骤[4] 细则**：kernel **先行**调 `check_constraint(spec, ci)`（细则属适配器、需 IO 上下文），把布尔结果物化为 `ConstraintCheck` **作为入参传入** `Evaluator::evaluate`（CONS-8）——如此 `evaluate` 保持纯逻辑、零 IO。`spec` 来自 `grant_constraints`（由 kernel 自快照取得并传入；适配器不读策略库）。
  - **步骤[8] 执行**：求值放行（`Decision::Allow{tier}`）、意图审计[7a] 与取连接[7b] 均成立后，kernel 传入 `&mut Channel`（连接管理层产出）与 `&Intent`，调 `execute`，收回未脱敏 `RawResponse`。
- **失败语义（fail-closed）**：
  - `classify` 返回 `Err` → kernel 据此 **deny**（步骤[2]，公理二；运行时不变量 `UNCLASSIFIABLE_INTENT_DENIED`）。
  - `check_constraint` 返回 `false` 或 `Err` → kernel 直接以 deny 短路，或传入 `passed=false` 的 `ConstraintCheck` 令 `evaluate` 据此 deny（二者等价，步骤[4]）。
  - `execute` 返回 `Err` → 错误经内核出口 `Sanitizer` 脱敏后返回；**有副作用动词一旦已执行，绝不返回 deny**（6.1 时序不变量，由内核守护）。
  - 适配器自身绝不吞错放行——返回 `Err` 是其唯一的失败表达，由内核翻译为 fail-closed 拒绝。

### 6.2 ← `postern-daemon::control`（被触发：discover）

- **方向**：`daemon::control` 触发本 crate 的 `Adapter::discover`。
- **内容与时机**：经 `POST /v1/resources/{code}/discover`（接入/发现）进入；控制面取得到资源的通路（`&mut Channel`）后调 `discover`，收回 `CapabilitySurface`（资源具备何种能力的事实）。
- **失败语义（fail-closed）**：`discover` 返回 `Err` → 接入侧探测失败（如版本不兼容、连接不可建立），控制面据此拒绝接入或报缺口，绝不据失败结果生成任何授权。
- **不变量**：`discover` 只由控制面触发，**数据面无任何路径触发它**；其产物只是事实，授权化由人经控制面圈选完成（发现≠授权）。

### 6.3 → 经 `postern-daemon::connpool` 提供的 `Channel`（只见 Channel）

- **方向**：本 crate 在 `execute` / `discover` 中**使用** kernel/control 递来的 `&mut Channel`（`Channel` 由连接管理层经 `connpool.acquire(resource, tier)` 取得、最终经 `Transport::open` 建立）。
- **内容**：适配器只见 `Channel` 这一"本地可用通路"抽象——**不知传输种类（ssh/ssm/direct）、不知真实地址、不知凭据、不知 tier**；长/非长连接差异不外溢到适配器（8.7 不变量）。
- **时机**：`execute` 在步骤[8]、`discover` 在控制面接入时；通路的获取/重建/退避/回收/会话净化全部由连接管理层在适配器视野之外完成。
- **失败语义（fail-closed）**：通路不可建立时，连接管理层在适配器被调用**之前**即 deny（步骤[7b]），`execute` 不会拿到坏通路；通路在执行中断开，适配器经 `Channel` 收到错误并以 `ExecError` 上报，由内核脱敏返回（绝不把 `connection refused to 10.0.3.17` 一类原始串外泄——该脱敏由传输/机密面在跨边界前完成，适配器侧本就拿不到真实地址，与机密类型 `Debug=REDACTED` 纪律一致）。
- **依赖纪律**：此交互**不构成对 `connpool` 或 `transports` 的依赖边**——`Channel` 类型定义在 `core`，本 crate 只消费该类型，物理依赖图上仍是 `adapters → core`。适配器通过 core 类型与连接管理"协作"，但不 import `daemon` 或 `transports`。

### 6.4 → `postern-core`（消费领域类型与 trait 定义）

- **方向**：本 crate 依赖并消费 `postern-core`（唯一允许的依赖边）。
- **内容**：`Adapter` trait 定义、`Intent`/`ClassifiedIntent`/`Capability`/`ObjectRef`/`ConstraintSpec`/`Channel`/`RawResponse`/`CapabilitySurface`、错误枚举与 `PageQuery`/`Page<T>`（若 `discover`/`classify` 产出集合则经统一分页）。
- **时机**：编译期（trait 实现）+ 运行时全程（类型流转）。
- **失败语义**：core 是纯数据/纯逻辑、零 IO，无运行期失败面引入本 crate；本 crate 不构造 core 中的机密占位类型（`ResolvedTarget`/`ResourceCredential` 在 core 仅不透明声明，构造权归机密面，契约 `SEC_CONSTRUCTION_SITES`）。

### 6.5 交互矩阵对齐

本篇与索引文档跨模块交互矩阵一致的条目：

| 调用方 | 被调方 | 经由 | 时机 |
|---|---|---|---|
| daemon::kernel | adapters `Adapter` | `classify` / `check_constraint` / `execute` | 步骤 [2][4][8] |
| daemon::control | adapters `Adapter::discover` | 触发能力面探测 | 资源接入/发现 |

`Channel` 由 `daemon::connpool` 在步骤[7] 经 `acquire(resource, tier)` 产出后传入本 crate；本 crate 是 `Channel` 的**使用者**而非获取者。

---

## 7 · 必守不变量

| 不变量 | 强制方式 |
|---|---|
| **无法可靠归类 → `Err` → deny**（白名单归类，宁可误拒） | 设计承诺（`classify` 签名 `Result`，未知/歧义/多语句一律 `Err`）；公理一、公理二；运行时行为契约 `UNCLASSIFIABLE_INTENT_DENIED` |
| **SQL 按语句树内最高危写节点定档，CTE/子查询里的 DELETE/DROP/TRUNCATE → Destroy，绝不降级** | 设计承诺（§3.1）；归类语料集成测试（"伪装攻击语料"，《详细设计文档》7.3）；`postern verify` 红队项 1、2（6.7） |
| **`SET` 一律拒绝、不设白名单逃生口** | 设计承诺（§3.1）；伪装攻击语料断言 deny |
| **`EXPLAIN` 仅非 `ANALYZE` 形态归 Observe**；`EXPLAIN ANALYZE` 按内部最高危写节点定档 | 设计承诺（§3.1） |
| **只见 `Channel`，不可达真实地址与凭据** | 契约 `ARCH_FORBIDDEN_EDGES`（禁 `adapters → secrets/transports/store`，build.rs `FORBIDDEN_EDGES` + 反例自检）；本 crate 无任何 `vault://` 解析、无 transport import 路径 |
| **不构造机密类型**（`ResolvedTarget`/`ResourceCredential`） | 契约 `SEC_CONSTRUCTION_SITES`（构造权仅 `postern-secrets`；build.rs `scan_construction_sites` 对非 secrets 路径构造计违规 + 反例自检） |
| **`discover` 只产出事实、不产生授权**（发现≠授权） | 设计承诺（§3.5）；仅控制面触发（CONS-20 命名边界，禁与数据面 `postern_surface` 互借） |
| **`execute` 只执行已被求值放行的意图** | 内核管线短路保证（步骤[1]~[6] 放行后才到步骤[8]）；有副作用动词两阶段审计时序由内核守护（6.1） |
| **`engine_enforced=false` 的协议须如实标注"归类+细则是唯一防线"** | 设计承诺（`engine_enforced()` 返回值 + 文档）；公理三 |
| **仅依赖 `core`，不依赖工作区任何 IO crate** | 契约 `ARCH_FORBIDDEN_EDGES` |
| **求值相关路径不吞错放行** | 适配器以 `Err` 表达失败、由内核 fail-closed 翻译；求值路径吞错由契约 `EVAL_NO_ERROR_SWALLOWING` 守护（该契约作用面为 `core::eval`/`daemon::kernel`，适配器经返回 `Err` 与之协同） |

---

## 8 · 验收标准

> 本节是 `postern-adapters` 的**验收基准**：每条都给「输入 → 可观察结果」的判据与**验证方式**，让实现者据此自检、审查者据此判定"本模块是否完成"。按 A~F 六维度组织（与本模块相关者；不相关者注明"不适用"）。维度 A 逐条对应 §3 功能，B 对应 §5 接口，C 对应 §4 边界，D 对应 §7 不变量，E 对应 §6 交互，F 收口关键 fail-closed 路径。验证方式词汇统一（见 00 §8 规约）。**纯设计：以下只描述"验证什么、怎么验证"，不含测试实现代码。**

### 8.A 功能完整性（对应 §3 每项功能）

| # | 功能（§3） | 输入 → 可观察结果（判据） | 验证方式 |
|---|---|---|---|
| A1 | `classify` 纯只读归类（§3.1） | `SELECT id,status FROM public.orders WHERE status='paid'` → `Ok(ClassifiedIntent{ capability=Query, objects=["public.orders"] })`（无 `INTO`、无写节点） | `单元测试`；`场景规格 docs/examples/04 §4.1 Trace ① [2]` |
| A2 | `classify` 按最高危写节点定档·CTE 不降级（§3.1） | `WITH x AS (DELETE FROM public.orders RETURNING *) SELECT * FROM x` → `Ok(capability=Destroy)`，**绝不**为 `Query`/`Mutate` | `集成测试（内存Fake）`（"伪装攻击语料"断言归档=Destroy）；`postern verify 项2`；`场景规格 docs/examples/04 §4.2 A`；`场景规格 docs/examples/07 §C`（红队九项·项1/2） |
| A3 | `classify` `Insert/Update/Merge` → `Mutate`，`Show/EXPLAIN(非 ANALYZE)` → `Observe`（§3.1） | `INSERT INTO t ...`→`Mutate`；`EXPLAIN SELECT ...`→`Observe`；`EXPLAIN ANALYZE INSERT ...`→`Mutate`（按内部最高危写节点） | `单元测试`；`集成测试（内存Fake）`（语料逐形态断言档位） |
| A4 | `classify` 不可靠归类一律 `Err`（§3.1） | 解析失败 / 多语句 / 未知节点 / `SET`/`DO`/`COPY`/`CALL` → `Err(ClassifyError)`（→ deny） | `单元测试`；`集成测试（内存Fake）`；`场景规格 docs/examples/04 §4.2 A`（归类不可达即 deny）；`场景规格 docs/examples/04 §4.1 Trace ①[2]`（归类失败即 deny、不进 [3]） |
| A5 | `check_constraint` 各 `kind` 语义（§3.2） | `table_allow` 白名单外表 → `Ok(false)`；`http_route` 白名单外 `(method,path)` → `Ok(false)`；`container_prefix` 前缀不匹配 → `Ok(false)`；命中则 `Ok(true)` | `单元测试`（逐 `kind`）；`场景规格 docs/examples/04 §4.1 Trace ①[4]/②[4]/③[4]` |
| A6 | `check_constraint` 同 `kind` 多行取交集、跨 `kind` 恒 `AND`（§3.2） | 同 `kind` 任一行不满足 → `Ok(false)`；不同 `kind` 任一不满足 → `Ok(false)`（fail-closed，默认不取并集） | `单元测试`（合并语义） |
| A7 | `engine_enforced()` 如实声明（§3.3） | postgres → `true`；http / docker_logs → `false` | `单元测试`；`构造签名审查`（返回值与文档 §3.3 一致） |
| A8 | `execute` 在 `Channel` 上执行放行意图、产出未脱敏 `RawResponse`（§3.4） | 给定已放行 `Intent` + `&mut Channel` → `Ok(RawResponse)`（原始未脱敏；脱敏归内核出口） | `集成测试（内存Fake）`（Fake `Channel`）；`场景规格 docs/examples/04 §4.1 Trace ①[8]/②[8]/③[8]` |
| A9 | Intent 负载格式 / MCP 动词工具参数 schema 定义（§3.6） | 各协议 `Intent` 结构与 `postern_query`/`postern_mutate`/… 的 `request` 形态由本 crate 定义；外壳忠实装箱不解释 | `构造签名审查`（schema 属主在 adapters）；`单元测试`（负载序列化往返） |
| A10 | `discover` 真实探测产出 `CapabilitySurface`（§3.5） | 给定 `&mut Channel` → `Ok(CapabilitySurface)`（资源能力事实，不含任何授权） | `集成测试（真实资源）`（接入侧探测）；`构造签名审查`（产物为事实型，无授权字段）；`场景规格 docs/examples/02 §4.1 步骤6`（控制面 discover 回报能力面事实） |
| A11 | 伪装攻击识别统一收敛为正确归类或 `Err`（§2 职责7、收敛于 §3.1 `classify`） | 写 CTE / 子查询藏写 / 多语句 / 注释混淆 / `DO` 块 / `SET` 会话篡改 → 各自归正确高危档或 `Err`，无一降级放行 | `集成测试（内存Fake）`（"伪装攻击语料"全集）；`postern verify 项1`；`postern verify 项2`；`场景规格 docs/examples/07 §C`（红队九项含项1/2） |

### 8.B 对外接口契约（对应 §5 `Adapter` trait）

| # | 接口（§5） | 判据（签名稳定 + 语义符合承诺 + 错误路径正确） | 验证方式 |
|---|---|---|---|
| B1 | `Adapter` trait 实现完整 | postgres / docker_logs / http 三实现均提供 `protocol`/`capabilities`/`engine_enforced`/`classify`/`check_constraint`/`execute`/`discover`，签名与 core 定义一致 | `构造签名审查`（trait 实现对齐 core 定义）；`单元测试` |
| B2 | `classify` 错误路径 | 失败唯一表达为 `Err(ClassifyError)`，**绝不**以 `Ok` 静默降档放行 | `单元测试`；`clippy（deny warnings）`（无 `#[allow]` 旁路）；与 §7「无法可靠归类→Err」同源 |
| B3 | `check_constraint` 错误路径 | 不通过返回 `Ok(false)`，无法判定返回 `Err(ConstraintError)`；二者皆经内核翻译为 deny | `单元测试` |
| B4 | `execute` / `discover` async 错误路径 | 失败为 `Err(ExecError)` / `Err(DiscoverError)`；不 panic、不吞错 | `集成测试（内存Fake）`（Fake `Channel` 注入断链/超时） |
| B5 | 错误枚举到"拒绝阶段"映射穷尽 | 本 crate 一个 `thiserror` 枚举；新增变体未在 `core::error` 写映射则**无法编译**（§5、详设 7.1） | `构造签名审查`（穷尽 `match`，编译期保证）；`单元测试` |
| B6 | 只消费 core 类型、不重定义 | `Intent`/`ClassifiedIntent`/`Capability`/`ObjectRef`/`ConstraintSpec`/`Channel`/`RawResponse`/`CapabilitySurface` 均来自 core | `构造签名审查`；`cargo tree`（依赖仅含 `postern-core`） |

### 8.C 边界·禁止项（对应 §4 每条"不做什么"；机器可验者优先标注）

| # | 不做的事（§4） | 判据（确实无此代码路径） | 验证方式 |
|---|---|---|---|
| C1 | 不做授权判定（allow/deny/escalate） | 本 crate 无 `Decision`/`Allow`/`Escalate` 产出路径；只产 `ClassifiedIntent`/`bool`/`RawResponse` | `构造签名审查`（无返回 `Decision` 的对外路径） |
| C2 | 不选 `CredentialTier`、不感知 tier | 全 crate 无 `CredentialTier` 入参/出参/分支 | `构造签名审查`（接口签名不含 tier） |
| C3 | 不获取/池化/重建/回收通路 | 无 `acquire`/连接池/退避/健康检查代码；只接收 `&mut Channel` | `构造签名审查`；`cargo tree`（无 connpool 依赖） |
| C4 | 不感知传输种类、不建/关通路 | 无 ssh/ssm/direct 分支，不依赖 `postern-transports` | **`Stele契约 ARCH_FORBIDDEN_EDGES`**（禁 `adapters→transports`）；`cargo deny`/`cargo tree` |
| C5 | 不取资源凭据与真实地址 | 无 `vault://` 解析、无 `(res,tier)→凭据` 路径、不依赖 `postern-secrets` | **`Stele契约 ARCH_FORBIDDEN_EDGES`**（禁 `adapters→secrets`）；与 §7「只见 Channel」同源；`场景规格 docs/examples/04 §4.2 G`（拿不到 app 账号） |
| C6 | 不执行响应脱敏（含 `mask_fields` 擦除、拒绝/错误脱敏） | `execute` 产出**未脱敏** `RawResponse`；`mask_fields` 仅定义声明形态，不持 `ScrubSet`、不擦字节 | `构造签名审查`（无 `Sanitizer`/`ScrubSet` 持有或调用）；`场景规格 docs/examples/04 §4.1 Trace ③[9]`（脱敏在 [9] 出口而非适配器） |
| C7 | 不构造 `ScrubSet` | 无 `ScrubSet` 构造点（属机密面 8.8） | **`Stele契约 ARCH_FORBIDDEN_EDGES`**（不依赖 secrets）；`构造签名审查` |
| C8 | 不读写策略、不构建快照 | 无 `PolicyRepo`/`PolicySnapshot` 路径，不依赖 `postern-store`；`spec` 由内核传入 | **`Stele契约 ARCH_FORBIDDEN_EDGES`**（禁 `adapters→store`）；`cargo tree` |
| C9 | `discover` 不产生授权、不决定何时触发 | `discover` 产物为纯事实 `CapabilitySurface`（无授权字段）；触发权在控制面 | `构造签名审查`；`场景规格 docs/examples/02 §4.1 步骤6`（接入探测经控制面、发现≠授权）；与 §7「发现≠授权」同源 |
| C10 | 不落审计、不采集来源、不构造 `ConnOrigin` | 无 `AuditSink` 调用；无 `ConnOrigin` 构造（属外壳 listener） | **`Stele契约 SEC_CONSTRUCTION_SITES`**（`ConnOrigin` 仅 daemon shells 构造）；`构造签名审查` |
| C11 | 不构造机密类型 | 无 `ResolvedTarget`/`ResourceCredential` 构造点（构造权仅 `postern-secrets`） | **`Stele契约 SEC_CONSTRUCTION_SITES`**；**`Stele契约 SEC_SECRET_TYPE_DISCIPLINE`**（机密类型禁 Clone/Serialize，本 crate 无从复制传递） |

### 8.D 必守不变量（对应 §7 每条；沿用 §7 已标强制手段）

| # | 不变量（§7） | 验证（判据 + 强制手段） | 验证方式 |
|---|---|---|---|
| D1 | 无法可靠归类 → `Err` → deny（白名单归类，宁可误拒） | 未知/歧义/多语句 `Intent` → `Err`；内核据此 deny（运行时契约 `UNCLASSIFIABLE_INTENT_DENIED`） | `单元测试`；`集成测试（内存Fake）`；`场景规格 docs/examples/04 §4.1 Trace ①[2]` |
| D2 | SQL 按最高危写节点定档，CTE/子查询里 DELETE/DROP/TRUNCATE → Destroy，绝不降级 | 伪装写语料逐条断言归 Destroy，无降级 | `集成测试（内存Fake）`（伪装攻击语料，详设 7.3）；`postern verify 项1`；`postern verify 项2` |
| D3 | `SET` 一律拒绝、不设白名单逃生口 | 任意 `SET ...` → `Err`（无白名单分支） | `单元测试`；`集成测试（内存Fake）`（语料断言 deny） |
| D4 | `EXPLAIN` 仅非 `ANALYZE` 归 Observe；`EXPLAIN ANALYZE` 按内部最高危写定档 | `EXPLAIN SELECT`→Observe；`EXPLAIN ANALYZE DELETE ...`→Destroy | `单元测试` |
| D5 | 只见 `Channel`，不可达真实地址与凭据 | 无 `vault://` 解析、无 transport import；依赖图仅 `adapters→core` | **`Stele契约 ARCH_FORBIDDEN_EDGES`**（build.rs `FORBIDDEN_EDGES` + 反例自检 `ARCH_FORBIDDEN_EDGES_TEETH`）；`cargo tree`/`cargo deny` |
| D6 | 不构造机密类型（`ResolvedTarget`/`ResourceCredential`） | 非 secrets 路径构造计违规 | **`Stele契约 SEC_CONSTRUCTION_SITES`**（`scan_construction_sites` + 反例自检 `SEC_CONSTRUCTION_SITES_TEETH`）；**`Stele契约 SEC_SECRET_TYPE_DISCIPLINE`** |
| D7 | `discover` 只产出事实、不产生授权（发现≠授权） | `discover` 仅控制面触发、产物无授权语义；与数据面 `postern_surface` 命名边界（CONS-20）不互借 | `构造签名审查`（产物为事实型）；`场景规格 docs/examples/02 §4.1 步骤6`；运行时行为契约 CONS-20（命名边界，非 24 条静态契约） |
| D8 | `execute` 只执行已被求值放行的意图 | 内核管线短路保证：步骤[1]~[6] 放行后才到 [8]；有副作用动词两阶段审计时序由内核守护（6.1） | `集成测试（内存Fake）`（管线放行后才达 execute）；`场景规格 docs/examples/05 §4.1 步骤6`（manage 两阶段审计） |
| D9 | `engine_enforced=false` 协议须如实标注"归类+细则是唯一防线" | http/docker_logs `engine_enforced()=false` 且文档标注；SQL 类 `true` | `单元测试`；`构造签名审查`；`postern verify 项1`（SQL 类引擎兜底为真则 PASS） |
| D10 | 仅依赖 `core`，不依赖工作区任何 IO crate | 依赖图无 `adapters→secrets/transports/store` | **`Stele契约 ARCH_FORBIDDEN_EDGES`**；`cargo deny`/`cargo tree` |
| D11 | 求值相关路径不吞错放行 | 适配器以 `Err` 表达失败；无 `.ok()`/`.unwrap_or(true)`/`.unwrap_or_default()` 旁路放行（与内核 `EVAL_NO_ERROR_SWALLOWING` 协同） | `clippy（deny warnings）`；`构造签名审查`（失败唯一表达为 `Err`）；契约作用面注：`EVAL_NO_ERROR_SWALLOWING` 主守 `core::eval`/`daemon::kernel`，适配器经返回 `Err` 协同 |

### 8.E 与相邻模块交互（对应 §6 每条交互边：方向/类型/时机/失败语义可验）

| # | 交互边（§6） | 判据（方向·类型·时机·fail-closed） | 验证方式 |
|---|---|---|---|
| E1 | ← `daemon::kernel`：`classify`（步骤[2]）（§6.1） | kernel 传 `&Intent` → 收 `ClassifiedIntent`；`classify` 返回 `Err` → kernel deny（不进 [3]） | `集成测试（内存Fake）`（Fake kernel 驱动）；`场景规格 docs/examples/04 §4.1 Trace ①[2]` |
| E2 | ← `daemon::kernel`：`check_constraint`（步骤[4]，先行物化为 `ConstraintCheck` 入参，CONS-8）（§6.1） | kernel 传 `(spec, ci)` → 收 `bool`；`false`/`Err` → kernel deny；`spec` 由内核自快照传入，适配器不读策略库 | `集成测试（内存Fake）`；`构造签名审查`（`check_constraint` 不持策略库句柄）；`场景规格 docs/examples/04 §4.1 Trace ②[4]` |
| E3 | ← `daemon::kernel`：`execute`（步骤[8]，放行+[7a]审计+[7b]取连接后）（§6.1） | kernel 传 `(&mut Channel, &Intent)` → 收未脱敏 `RawResponse`；有副作用动词一旦已执行**绝不再返回 deny**（错误经内核出口脱敏） | `集成测试（内存Fake）`；`场景规格 docs/examples/04 §4.1 Trace ②[8]/[10]`（已执行不返回 deny） |
| E4 | ← `daemon::control`：`discover`（接入/发现触发）（§6.2） | 经 `POST /v1/resources/{code}/discover` 进入；control 传 `&mut Channel` → 收 `CapabilitySurface`；`Err` → control 拒绝接入，绝不据失败生成授权；数据面无触发路径 | `集成测试（真实资源）`；`场景规格 docs/examples/02 §4.1 步骤6`；与 §7「仅控制面触发」同源 |
| E5 | → 经 `connpool` 提供的 `Channel`（只见 Channel）（§6.3） | `execute`/`discover` 仅消费 `&mut Channel`；不知 transport 种类/真实地址/凭据/tier；通路在调用**前**不可建则内核已 deny（步骤[7b]），适配器拿不到坏通路；执行中断链 → `ExecError` 上报，由内核脱敏 | `构造签名审查`（仅 `&mut Channel` 入参）；`集成测试（内存Fake）`（断链注入）；`场景规格 docs/examples/04 §4.2 D`（不可建→deny） |
| E6 | → `postern-core`：消费领域类型与 trait 定义（§6.4） | 编译期实现 trait + 运行时类型流转；core 零 IO 无运行期失败面引入；本 crate 不构造 core 机密占位类型 | `cargo tree`（唯一依赖边 `adapters→core`）；`构造签名审查`；**`Stele契约 SEC_CONSTRUCTION_SITES`** |

### 8.F 失败与边界行为（关键 fail-closed 路径逐条可验）

| # | fail-closed 路径 | 输入 → 可观察结果 | 验证方式 |
|---|---|---|---|
| F1 | 归类失败 → 拒绝 | 不可靠/歧义/多语句 `Intent` → `classify` 返回 `Err` → 内核 deny（确未执行） | `单元测试`；`集成测试（内存Fake）`；`场景规格 docs/examples/04 §4.2 A` |
| F2 | 伪装写不降级 → 拒绝（引擎兜底为最后防线） | ① 写 CTE 经**只读**授权 → 归类拦截或经只读账号被引擎拒（对应 verify 项1）；② `WITH x AS (DELETE ...) SELECT ...` 经 **mutate** 授权 → 按最高危写节点归 `Destroy`、mutate 授权不足以放行（对应 verify 项2）；两者均零行副作用 | `postern verify 项1`；`postern verify 项2`；`场景规格 docs/examples/04 §4.2 A` |
| F3 | 细则不通过 → 拒绝 | `table_allow`/`http_route`/`container_prefix` 不匹配 → `Ok(false)` 或 `Err` → 内核 deny | `单元测试`；`场景规格 docs/examples/04 §4.1 Trace ③[4]` |
| F4 | 通路不可建 → 拒绝（适配器视野外短路） | 通路建立失败 → 内核在 `execute` 调用前 deny（步骤[7b]）；适配器不被调用 | `集成测试（内存Fake）`；`场景规格 docs/examples/04 §4.2 D` |
| F5 | 执行中断链 → 经内核脱敏返回，不外泄真实地址 | `Channel` 执行中断 → `ExecError` 上报；`connection refused to 10.0.3.17` 类原始串不外泄 | `集成测试（内存Fake）`（断链注入）；`场景规格 docs/examples/04 §4.2 C`（脱敏出口） |
| F6 | 适配器绝不吞错放行 | 任一失败唯一表达为 `Err`，无静默 `Ok`/降级放行路径 | `clippy（deny warnings）`；`构造签名审查`；与 §7「不吞错」、契约 `EVAL_NO_ERROR_SWALLOWING`（内核侧）协同 |

### 完成定义（Definition of Done）

**当且仅当上述 A~F 全部判据成立——三个内置 `Adapter` 实现的归类按最高危写节点定档且伪装写无一降级（A2/A11/F2，`postern verify 项1/项2` PASS）、各细则 `kind` 语义与 `engine_enforced` 如实（A5/A7）、依赖图无任一禁止边（C4/C5/C7/C8/D10，`ARCH_FORBIDDEN_EDGES` 绿）、不构造任何机密类型与 `ConnOrigin`（C10/C11/D6，`SEC_CONSTRUCTION_SITES`/`SEC_SECRET_TYPE_DISCIPLINE` 绿）、所有失败沿 `Err` fail-closed 翻译为拒绝（B2/F1/F6）——`postern-adapters` 视为完成。**
