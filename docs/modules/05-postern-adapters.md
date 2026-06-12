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
