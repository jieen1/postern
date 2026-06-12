# postern-daemon 模块详细设计

> 本篇是 `postern-daemon` crate 的模块级详细设计，在《详细设计文档》第八部分（8.2 数据面内核、8.5 连接管理、8.9 观测面执行、8.10 控制面、8.12 外壳服务端）的领域裁决之上展开。与本篇冲突时，以《技术设计文档》七公理与《详细设计文档》第八部分为准。纯设计，不含实现代码、阶段划分或进度状态。

---

## 1. 定位（一句话）

`postern-daemon` 是整个 workspace 的**唯一组装点**，编译产出二进制 `posternd`：它把零 IO 的领域核心（core）与四个面 crate（store / secrets / transports / adapters）的实现装配成一个常驻进程，承载数据面请求内核、连接管理、控制面、观测面执行、脱敏执行与策略生命周期自动机——是七公理落到运行时的**承重体**。

---

## 2. 承载领域与职责范围

本 crate 是依赖图唯一可依赖全部下游库的节点（依赖图见索引文档与第三部分 3.2），因此被分派承载第八部分中需要"组装多个面"才能成立的五个领域：

| 领域（第八部分） | 本 crate 内部模块 | 职责一句话 |
|---|---|---|
| 8.2 数据面请求内核 | `kernel/` | 把任意外壳进入的请求按统一管线 [0]→[10] 串到底；出口统一脱敏的调用职责；审计相对执行的时序不变量 |
| 8.5 连接管理 | `connpool/` | 以 `(ResourceCode, CredentialTier)` 为键治理通路的获取、复用、健康、上限、回收、强制中断与归池前会话净化 |
| 8.9 观测面（执行侧） | `kernel/`（提交事件）+ `sweeper/`（回收审计） | 执行"不可记=不放行"的记录纪律与两阶段审计时序（schema 与纪律的**定义权**仍属观测面领域，载体属存储层） |
| 8.10 控制面 | `control/` + `sweeper/` | 策略状态的唯一变更入口域：全部管理端点 + 系统自动机（sweeper 回收、import 协调，actor=`system`） |
| 8.12 外壳（服务端） | `shells/http/`、`shells/mcp/` | 数据面接入形态的翻译与事实采集（ConnOrigin 采集、NormalizedRequest 装箱），自身不做安全决策 |

本 crate 不"持有"任何领域的权威定义（领域定义在 core 与各面 crate），它只**编排与执行**。两平面物理隔离是本 crate 的工程承诺：`shells/` 与 `control/` 是两个独立 axum router，绑定不同 socket、权限位不同，依赖注入集合不同。

内部模块全景（与第四部分 4.5 一致）：

```
src/
├── boot/        # 启动序列：开库 → 重建快照 → 解锁保险箱 → 注册插件 → 开放数据面（含 data.sock 可连 uid 自检）
├── kernel/      # 数据面请求内核：[0]→[10] 编排；两阶段审计时序；出口统一脱敏的调用点
├── shells/
│   ├── http/    # 数据面 HTTP 外壳（axum router，挂 data.sock）
│   └── mcp/     # MCP 外壳（rmcp streamable-http，挂 data.sock 的 /mcp；固定动词工具面含 postern_surface）
├── control/     # 控制面 API（axum router，挂 control.sock，0600 + 控制面认证；审批挂起端点）
├── connpool/    # 连接管理：池键 (ResourceCode, CredentialTier)；重连/退避/健康/上限/回收/中断/会话净化
├── sweeper/     # TTL 回收后台任务（temp_grants / credentials / mode / 审批超时 到期 → 事务回收 → 快照重建 → 审计）
└── sanitize/    # Sanitizer 执行：应用机密面签发的 ScrubSet 不透明句柄 + 声明级 MaskRule
```

---

## 3. 支持的功能

按对内部模块组织（每条功能均不越出第 4 节边界）：

### 3.1 boot — 启动序列

按固定时序拉起进程并 fail-closed 自检；任一前置条件不成立即拒绝启动（公理二）：

1. **开库**：打开 `policy.db`（WAL），校验 `PRAGMA user_version` 与迁移版本；校验 `settings` 表（出现未知 key 即拒绝加载，见 5.2）。
2. **重建快照**：经存储层在一次事务内全量加载并展开授权空间，物化首份 `Arc<PolicySnapshot>`。
3. **解锁保险箱**：依 `config.toml` 选定的 `MasterKeySource` 解锁 `vault.postern`，机密面在内存中以 `Zeroizing` 持有 payload 并据 `targets`/`secrets` 构造 ScrubSet；写 `lifecycle` 审计（保险箱解锁）。
4. **注册插件**：把 Authenticator / Adapter / ConditionPredicate / Transport 的实现编译期注册进内核与连接管理的注册表（trait 对象）。
5. **开放数据面**：先创建 `control.sock`（`0600`），再创建 `data.sock`（`0660` 或专用组）并挂载数据面 router——数据面最后开放，确保策略、机密、连接均已就绪。
6. **`data.sock` 可连 uid 自检（硬前置条件 ②）**：检查 `data.sock` 的可连 uid 集合；若该集合包含 daemon 自身 uid（Agent 与 daemon 同 uid），**fail-closed 拒绝启动**（或在明知风险的形态下强制告警），杜绝"同 uid 还能跑"的静默不安全状态。该项纳入 `postern verify` 红队项。

### 3.2 kernel — 数据面请求内核

对每个外壳提交的 `NormalizedRequest`，按管线 [0]→[10] 串到底并逐步短路（管线步骤与失败语义见第六部分 6.1）：

- **[0] 之后请求与外壳无关**（公理七）：内核只见 `NormalizedRequest`，不知其来自 HTTP 还是 MCP。
- **[4] 细则先行**（CONS-8）：在调用 `Evaluator::evaluate` **之前**先跑 `Adapter::check_constraint`，把结果物化为 `ConstraintCheck` 作入参传入 `evaluate`——保证 `evaluate` 仍是 core 零 IO 纯逻辑。
- **[6] 动作分流**：`Decision::Allow{tier}` 进执行路径；`Decision::Deny` 与 `Decision::Escalate`（审批关闭即取 fallback 恒 deny）走拒绝出口；二者**同等经出口脱敏**。
- **两阶段审计时序（安全不变量，见 6.1）**：只读动词（observe/query）执行后单次审计，写失败按 deny 返回；有副作用动词（mutate/execute/manage/destroy）执行前先落 **intent** 审计（[7a]），intent 写不进则执行前 deny（此时确未执行），执行后落 **outcome** 审计（[10]），**已执行的请求绝不返回 deny**，outcome 写失败返回"已执行但审计降级"的可识别错误码。
- **出口统一脱敏的调用职责**：内核是 Sanitizer 的**唯一调用点**——正常响应、错误信息、拒绝响应、审计事件正文，一切离开内核的字节都经 `sanitize/` 模块过同一 Sanitizer，再交外壳格式化。
- **CatchPanic**：数据面 router 挂 CatchPanic 层，任何 panic 一律转为脱敏后的 deny 响应 + 一条 `kind=anomaly` 审计，绝不让 panic 成为不留痕的失败路径。

### 3.3 shells/http + shells/mcp — 数据面外壳（服务端）

- **HTTP 外壳**：axum router 挂 `data.sock`；解析协议形态、不合法形态返回经同一 Sanitizer 的外壳层 4xx（常量安全文案）；构造 `NormalizedRequest` 提交内核。
- **MCP 外壳**：rmcp streamable-http 挂 `data.sock` 的 `/mcp`，并提供 stdio 桥的服务端侧；工具集是**固定动词工具**（不随授权动态增删，描述只含事实）：`postern_grants`、`postern_query/observe/mutate/execute/manage/destroy`、`postern_surface`。
- **`postern_surface`（CONS-20）**：返回该 Principal Scope 内**已授权能力面**——授权快照的投影，**禁止触达 `Adapter::discover`**、不触达任何底层资源。这与控制面接入侧的 `discover` 是两个术语，命名规范固化其边界、禁止互借。
- **ConnOrigin 仅 listener 构造**：`ConnOrigin`（`UnixPeer{uid,gid}` / `Tcp{remote}`）只能由本模块 listener 层构造，绝不采信请求自报字段（契约 `SEC_CONSTRUCTION_SITES`）。

### 3.4 control — 控制面 API + 系统自动机

- **全部端点**（见 6.5）：principals / credentials（签发/吊销/轮换/可信域）/ roles / bindings / resources（含 `POST /v1/resources/{code}/discover` 触发接入侧探测）/ constraints / conditions / deny-notes / settings / grants/temp（elevate/revoke）/ mode / grants 视图 / audit 查询 / denials/summary / **approvals（审批挂起队列查询与裁决）** / export / import / verify / health / shutdown。
- **写入三联动**：每个写端点 = 一次事务 + 快照重建 + 审计事件；全部集合端点强制分页（`page_no/page_size`，缺省 20，钳制上限 200，`Page<T>` 信封，分页在 SQL/扫描层执行）；雪花 id 一律字符串序列化。
- **乐观锁端到端**：读端点统一返回 `version`，更新/删除端点必须携带期望 `version`，不匹配返回 `409 Conflict` 并写 `policy_change` 审计；系统协调写不走乐观锁。
- **系统自动机**：sweeper 回收、import/apply 协调以 `actor=system`（`created_by/updated_by=system`）走同一事务路径、同等审计。

### 3.5 connpool — 连接管理

- **池键 `(ResourceCode, CredentialTier)`**：不同 tier 永不共享连接（账号隔离在连接粒度成立）。
- **建立流程**：收到 `Decision::Allow{tier}` 后 `acquire(resource, tier)`；向机密面一次性取 `(ResolvedTarget, ResourceCredential)` 不透明句柄并即时传入 `Transport::open`，调用边界外凭据引用即时释放。
- **重连/退避/健康/上限**：`persistent` 通路池化复用 + 健康检查 + 指数退避重连（基数 1s、上限 60s、带抖动）；非长连接即建即用即弃；每资源/全局并发上限，超限有界排队或 deny（fail-closed）。
- **回收与中断**：空闲回收（默认 10min）与优雅销毁（排空在途请求）；freeze/吊销时对相关 `(resource[, principal])` 在用连接**强制 abort/cancel**（取消底层查询、关闭隧道），而非仅优雅排空。
- **归池前会话净化（不变量）**：复用前强制重置会话态（如 PostgreSQL `DISCARD ALL`、重置 `search_path`、回滚未决事务、清临时表）；净化失败的连接销毁而不归池（fail-closed）。
- **连接审计（`connection_event`）**：通路建立/健康剔除/回收经 `AuditSink::record` 落 `connection_event`（字段为 resource、tier 名、transport 种类，不含真实地址/凭据）；该审计由连接管理层写入（传输层只如实上报健康事实、不写审计，见 6.4）。

### 3.6 sweeper — TTL 回收后台任务

事务性回收过期记录并写审计：`temp_grants`（写 `ended_at`/`end_reason=expired`）、`credentials.expires_at`、`mode_state.expires_at`、审批超时挂起项。回收后在同一写锁内重建快照。正确性不依赖 sweeper 时序——过期判定在求值时刻按墙钟二次校验（见 6.2），sweeper 只做"可见性回收 + 留痕打扫"。

### 3.7 sanitize — Sanitizer 执行

实现 `Sanitizer` trait：小响应/错误串/拒绝响应整体脱敏（`scrub`），流式大输出走滑动重叠窗口（`scrub_stream`，保留上块尾部 N 字节参与下块匹配，消除边界分块逃逸，N 取 ScrubSet 最长匹配模式上界，有界缓冲与背压）。脱敏材料两路：机密面签发的**系统级 ScrubSet 不透明句柄**（只能 match-and-erase）+ 来自 `grant_constraints.kind='mask_fields'` 的声明级 `MaskRule`。

---

## 4. 明确边界（不做什么）

| 排除项 | 归属域 / crate |
|---|---|
| 领域类型、求值步骤 [1][3][5][6] 纯逻辑、tier 选择（动词→tier）、DenyResponse 事实组装、IdGen/PageQuery | 策略引擎 / 领域核心模型（`postern-core`） |
| policy.db schema 与迁移、统一基础仓储、PolicyRepo 事务读写、**PolicySnapshot 的构建**、JSONL 审计载体与扫描 | 存储层（`postern-store`） |
| 资源凭据与代号↔真实地址映射的持有、`ResolvedTarget`/`ResourceCredential` 的构造、`(res,tier)→凭据` 解析、**ScrubSet 的构造/更新/持有**、保险箱与 MasterKeySource | 机密面（`postern-secrets`） |
| 单条通路的建立与通路内保活（心跳/续约）、关闭的物理执行 | 传输（`postern-transports`） |
| 协议意图的 `classify`/`check_constraint`/`execute`/`discover`、Intent 负载格式定义、伪装攻击识别 | 适配器（`postern-adapters`） |
| 审计事件 schema 与记录纪律的**定义**、聚合分析逻辑、`AuditExporter` 接口语义 | 观测面领域（schema 在 core/store，纪律执行在本 crate） |
| 网关凭证语义规则（过期/吊销/可信域判定规则）的**定义** | 身份与凭证域（认证器族，定义在 core，本 crate 仅注册并由 Evaluator 调用） |
| 控制面瘦客户端 / SPA / 桌面壳（客户端侧、stdio↔UDS 桥的客户端入口） | `postern-cli` + 桌面外壳进程 |

要点裁决（与第八部分速查表一致）：

- **tier 选择归策略引擎**：本 crate 的连接管理只接收 `Allow{tier}` 并据已选 tier 取连接，**绝不做动词→tier 的选择**。
- **ScrubSet 归机密面**：本 crate `sanitize/` 只持不透明 match-and-erase 句柄，不可枚举、不可序列化 ScrubSet。
- **重连/退避/回收归连接管理（本 crate）**，但单条通路的建立、保活与关闭的物理执行归传输——决策者（连接管理）与执行者（传输）分离。
- **TTL 过期判定归策略引擎（求值时刻墙钟）**，sweeper 只做可见性回收。
- **PolicySnapshot 的构建归存储层**，本 crate 只**消费**只读快照（`PolicyView::snapshot`）。

---

## 5. 对外接口

`postern-daemon` 是二进制 crate，**不向其他 workspace crate 暴露库接口**（`postern-cli` 仅依赖 core + HTTP/UDS 客户端，不依赖本 crate）。其"对外接口"是**进程对外形态**与**内部模块对内契约**：

### 5.1 进程对外形态

- **数据面**：MCP（rmcp streamable-http，`/mcp`）与 HTTP 端点，挂 `data.sock`（`0660` 或专用组，Agent 可连的唯一入口）。
- **控制面**：HTTP/JSON over `control.sock`（`0600`，仅属主，叠加同 uid 也成立的控制面认证：`SO_PEERCRED` uid 比对 + 控制面专用本地凭证）。放大时同一 router 加挂 mTLS TCP listener。

### 5.2 内部模块对内契约（设计承诺，非实现）

内核对外壳暴露唯一入口（与 8.2 一致）：

```rust
// 设计承诺：外壳提交归一化请求，得到已脱敏的结果或结构化拒绝；步骤[0]之后与外壳无关。
// 入口的依赖注入集合仅含：PolicyView（只读快照）、Adapter/Authenticator/Predicate 注册表、
// 连接管理句柄、Sanitizer、AuditSink —— 不存在 PolicyRepo 与 vault 句柄（构造函数签名保证）。
submit(req: NormalizedRequest) -> Result<SanitizedResponse, DenyResponse>
```

连接管理对内核与适配器暴露"一个可用通路"（与 8.5 一致）：

```rust
acquire(resource: &ResourceCode, tier: &CredentialTier) -> Result<Channel /* 或通路租约 */, AcquireError>
```

脱敏执行实现 core 定义的 `Sanitizer`（与 8.2/6.4 一致）：`scrub(payload, declared)` / `scrub_stream(declared)`。控制面对外是 HTTP 端点表（见 6.5），不是 Rust 接口。

---

## 6. 与相邻模块的交互

本 crate 是组装点，与**全部下游 crate**交互。下列每条均与索引文档依赖图、交互矩阵一致；本 crate 对它们的依赖**全部是被允许的边**（daemon → core/store/secrets/transports/adapters），不存在被禁止的依赖边。

### 6.1 ← → `postern-core`（消费领域类型与插件 trait 定义）

- **方向**：daemon 依赖 core。`kernel` 调用 `Evaluator`；`shells` 构造 `NormalizedRequest`/`ConnOrigin`；全模块共享领域类型与 `IdGen`/`PageQuery`。
- **内容**：传入 `&NormalizedRequest`、`&ClassifiedIntent`、`&ConstraintCheck`、`&PolicySnapshot`、`now: Timestamp`；得回 `(Decision, EvalTrace)`。`Decision::Allow{grant, tier}` 中的 `tier` 即连接管理的取连接依据。
- **时机**：求值步骤 [1][3][5][6]——`kernel` 在跑完 `check_constraint`（[4]）后调用 `Evaluator::evaluate`。
- **失败语义**：`evaluate` 一切 Err 解析为 deny（公理二，契约 `EVAL_NO_ERROR_SWALLOWING`，本 crate `kernel` 路径在该契约扫描范围内）；`Decision::Deny`/`Escalate(fallback)` 经出口脱敏返回。

### 6.2 ← → `postern-store`（PolicyView / PolicyRepo / AuditSink）

三条独立交互，按平面隔离严格区分注入位置：

- **`PolicyView::snapshot()`（数据面读路径）**
  - 方向：`kernel` 调 store 的 `PolicyView` 实现，得 `Arc<PolicySnapshot>`。
  - 内容：只读原子快照（含展开授权空间、凭证元数据含 secret_hash、tier 声明、constraints/conditions、mode、deny_notes、approval 设置；**不含任何 vault 内容**）。
  - 时机：每请求步骤 [1]~[6] 读快照（无锁、微秒级），TTL 在求值时刻按墙钟二次校验。
  - 失败语义：快照是 `Arc` 原子投影，读路径无失败点；快照不可得（极端启动失败）→ boot fail-closed 拒绝启动。
- **`PolicyRepo`（控制面写路径，仅 `control` 与 `sweeper` 可达）**
  - 方向：`control`/`sweeper` 调 store 的 `PolicyRepo` 做事务读写，COMMIT 后在同一写锁内触发快照重建（`Arc` 原子替换）。
  - 内容：策略状态写入（携带期望 `version` 走乐观锁；系统协调写幂等谓词驱动不走乐观锁）；级联逻辑删除在同一事务内施加。
  - 时机：每次策略变更（启动序列重建首份快照、控制面写端点、sweeper 回收、import 协调）。
  - 失败语义：事务失败/版本冲突 → 不 COMMIT、不重建快照、返回 `409`/错误并写审计（公理三：无双源）；**`PolicyRepo` 与 vault 句柄绝不注入数据面 router**（红线 7.2-2，构造函数签名保证）。
- **`AuditSink::record(event)`（观测面执行落地）**
  - 方向：`kernel`（请求事件）、`connpool`（`connection_event`：通路建立/健康剔除/回收，字段为 resource、tier 名、transport 种类，不含真实地址/凭据）与 `control`/`sweeper`（policy_change/credential_event/mode_change/lifecycle）调 store 的 `JsonlAuditSink`。
  - 内容：`AuditEvent`（已脱敏；雪花 id 字符串；含 stage/reason 等）。
  - 时机：步骤 [7a]（有副作用动词 intent）/ [10]（outcome 或只读单次）；连接管理在通路建立/健康剔除/回收时；控制面每次写入后。
  - 失败语义（安全不变量，见 6.1）：只读动词审计写失败 → 该请求按 deny 返回；有副作用动词 intent 写失败 → 执行前 deny（确未执行）；outcome 写失败 → 返回"已执行但审计降级"错误码，**绝不返回 deny**。审计内容写入前必经同一 Sanitizer。

### 6.3 ← → `postern-secrets`（CredentialProvider / resolve / ScrubSet / MasterKeySource）

- **`MasterKeySource::obtain()`（启动序列解锁）**
  - 方向：`boot` 调机密面解锁保险箱。
  - 内容：得 `Zeroizing<[u8;32]>`（包裹槽解出的 data-key 路径），机密面据此在内存解锁 payload。
  - 时机：启动序列第 3 步——**开放数据面之前**（交互矩阵：`daemon::boot → secrets vault MasterKeySource`）。
  - 失败语义：解锁失败/格式版本不识别 → fail-closed 拒绝启动（绝不按旧假设解析）；写 `lifecycle` 审计。
- **`CredentialProvider::credential_for(res, tier)` + `resolve(code)`（建连时取句柄）**
  - 方向：`connpool` 调机密面，一次性取 `(ResolvedTarget, ResourceCredential)` 不透明句柄。
  - 内容：传入 `&ResourceCode` + `&CredentialTier`，得回不可 Clone/Serialize、`Debug=REDACTED` 的机密句柄（生命周期不出本次建连调用）。
  - 时机：步骤 [7b] 建立通路时（交互矩阵：`daemon::connpool → secrets CredentialProvider + 映射解析`）。
  - 失败语义：取不到凭据/解析失败 → `acquire` 返回错误 → 步骤 [7b] deny（fail-closed）；句柄即时传入 `Transport::open` 后释放，本 crate **不可读取明文**（契约 `ARCH_FORBIDDEN_EDGES` 不禁 daemon→secrets，但句柄类型本身不可读 + `SEC_CONSTRUCTION_SITES` 禁本 crate 构造机密类型）。
- **ScrubSet 不透明句柄（脱敏材料）**
  - 方向：机密面在保险箱解锁后构造 ScrubSet，签发给 `sanitize/`；`sanitize/` 只持该 match-and-erase 句柄。
  - 内容：单向匹配视图，覆盖 `targets` 真实地址/IP、`secrets` 凭据值及常见编码形态、私网段、连接串模式；句柄不可枚举、不可序列化。
  - 时机：启动解锁后获取，保险箱写入更新后刷新；步骤 [9] 出口脱敏时应用。
  - 失败语义：ScrubSet 是黑名单兜底，真正保证来自上游匿名化与凭据零接触（见 6.4 脱敏诚实度）；句柄内容永不出现在任何输出路径。

### 6.4 ← → `postern-transports`（open / 健康 / 关闭）

- **方向**：`connpool` 调 transports 的 `Transport::open`，并经 `Channel` 行使健康查询与关闭指令。
- **内容**：把机密面取出的 `ResolvedTarget`/`ResourceCredential` 一次性传入 `open`，得回 `Channel`（对上层呈现一致的"本地 socket"抽象）；关闭/中断时下达指令由传输物理执行。
- **时机**：步骤 [7b] 建立通路；freeze/吊销时的强制 abort；空闲回收/优雅销毁时的关闭。
- **失败语义**：`open` 失败 → `acquire` 失败 → deny（fail-closed）；**重建决策与退避节奏归本 crate**，传输只上报通路死亡、绝不自行重建；传输层错误在跨 `postern-secrets`/`postern-transports` 调用边界外抛前已脱敏为不含真实地址的错误码（红线 7.2-1），本 crate 不让原始地址串外泄。

### 6.5 ← → `postern-adapters`（classify / check_constraint / execute / discover）

- **`classify`（步骤 [2]）**
  - 方向：`kernel` 调适配器把 `Intent` 归一化为 `ClassifiedIntent`（capability + objects）。
  - 时机：求值前的语义归一化（交互矩阵：`daemon::kernel → adapters`）。
  - 失败语义：无法可靠归类 → `ClassifyError` → deny（白名单归类，宁可误拒，公理二）。
- **`check_constraint`（步骤 [4]，CONS-8）**
  - 方向：`kernel` 在调 `evaluate` **前**先跑 `check_constraint`，把结果物化为 `ConstraintCheck` 入参传入 `evaluate`。
  - 失败语义：`false`/`Err` → kernel 以 deny 短路（或传 `passed=false` 令 evaluate 据此 deny，二者等价）。
- **`execute`（步骤 [8]）**
  - 方向：`kernel` 在求值放行且取得 `Channel` 后调 `Adapter::execute(&mut ch, intent)`，得 `RawResponse`。
  - 时机：[7a] intent 审计成功（有副作用动词）+ [7b] 取连接成功之后。
  - 失败语义：`execute` 错误经出口脱敏后返回；**已执行的请求绝不返回 deny**（见两阶段审计时序）。
- **`discover`（接入侧探测，仅 `control` 触发，CONS-20）**
  - 方向：`control` 的 `POST /v1/resources/{code}/discover` 调 `Adapter::discover` 真实连上资源探测能力面。
  - 时机：资源接入/发现（交互矩阵：`daemon::control → adapters Adapter::discover`）。
  - 失败语义：发现≠授权——结果供运维经控制面圈选，**绝不暴露给数据面 Agent**；数据面 `postern_surface` 是快照投影，与 `discover` 无关。

> 注：适配器只见 `Channel`，不可达地址与凭据——`adapters ↛ secrets/transports/store` 是被禁止的依赖边（契约 `ARCH_FORBIDDEN_EDGES`）。本 crate 作为组装点负责把"已脱去真实地址的可用通路"交给适配器，由此该禁止边在运行时同样成立。

### 6.6 ← `postern-cli`（被调方，经控制面 API）

- **方向**：`postern-cli`（瘦客户端）调本 crate 的 `control` 端点；本 crate 是被调方。
- **内容**：HTTP/JSON over `control.sock`，每条管理命令 = 一次控制面调用 + 结果渲染。
- **时机**：每条 `postern ...` 管理命令（交互矩阵：`cli → daemon::control`）。
- **失败语义**：控制面认证不通过/同 uid 自检语义 → 拒绝服务该连接；统一 `{error:{code,message}}` 信封，`message` 只用常量化安全文案。`cli ↛ store/secrets` 是被禁止的依赖边——客户端无任何安全逻辑，一切经本 crate 控制面 API。

---

## 7. 必守不变量

| 不变量 | 强制手段 |
|---|---|
| 数据面 handler 的依赖注入集合中**不存在** PolicyRepo 与 vault 句柄，只有 `PolicyView`（只读快照）、`AuditSink`、连接池、Sanitizer、注册表 | 构造函数签名审查（红线 7.2-2）；跨 crate 禁止边由契约 `ARCH_FORBIDDEN_EDGES` |
| 两平面 socket 权限隔离：`control.sock=0600`（仅属主）、`data.sock=0660`/专用组；二者为独立 axum router，注入集合不同 | 启动序列创建顺序 + 权限位（5.5）；构造函数签名 |
| **同 uid 前置条件**：`data.sock` 可连 uid 集合若含 daemon 自身 uid → fail-closed 拒绝启动；`control.sock` 叠加同 uid 也成立的控制面认证 | boot 启动自检（硬前置条件 ②）；`postern verify` 红队项 |
| `ConnOrigin` 只能由 `shells` listener 层构造，绝不采信请求自报字段 | 契约 `SEC_CONSTRUCTION_SITES`（`ConnOrigin` 只在 daemon shells 构造）+ 反例自检 |
| **审计相对执行的时序**：只读动词审计写失败按 deny；有副作用动词 intent 写失败执行前 deny、已执行绝不返回 deny、outcome 失败返回"已执行但审计降级" | 设计级安全不变量（6.1 / 红线 7.2-5）；运行时行为契约 `AUDIT_FAILURE_DENIES` |
| **一切离开内核的字节必经 Sanitizer**——正常响应、错误、拒绝响应、外壳层 4xx、框架级错误、panic 响应体，套统一 `{error:{code,message}}` 信封，`message` 仅常量安全文案 | 红线 7.2-3；内核为唯一出口；CatchPanic 层兜底 |
| 求值路径（本 crate `kernel`）禁吞错放行（`.ok()`/`.unwrap_or(true)`/`.unwrap_or_else(\|_\| true)`/`.unwrap_or_default()`） | 契约 `EVAL_NO_ERROR_SWALLOWING`（扫描 `crates/postern-daemon/src/kernel/`）+ 反例自检 + clippy deny 清单 |
| 不同 tier 不共享连接（账号隔离在连接粒度成立）；无法建立通路 → deny；超限绝不静默放行 | 连接管理设计（8.5 / 6.3）；池键含 `CredentialTier` |
| 归池前会话净化为不变量——净化失败的连接销毁而不归池 | 连接管理设计（6.3） |
| 内核对策略状态只读，绝不产生写入；策略写入唯一入口是 `control`/`sweeper` 经 `PolicyRepo`，写入 = 一次事务 + 快照重建 + 审计 三联动 | 平面隔离注入（数据面无 `PolicyRepo`）；控制面写端点设计（6.5）；公理三 |
| 系统自动机（sweeper/import）与人类写入走同一事务路径、同等审计（`actor=system`）；正确性不依赖 sweeper 时序（过期判定在求值时刻墙钟二次校验） | 控制面/ sweeper 设计（8.10 / 6.2）；fail-closed 一律解析为拒绝（公理二） |
| `on_timeout` 恒 `deny`，导入校验与设置写入两处都拒绝 allow；daemon 重启时一切挂起审批恒 deny | 控制面审批端点 + settings 校验（6.10）；fail-closed |
| `unsafe_code = forbid`（全 crate）；`SO_PEERCRED` 走安全 API（`tokio UnixStream::peer_cred`） | workspace lints（7.1）；CI `-D warnings` |

---

*结语：`postern-daemon` 不持有任何领域的权威定义，只把它们装配为一个 fail-closed 的常驻进程。两平面的物理隔离、出口脱敏的单一调用点、审计相对执行的时序、`(资源, tier)` 连接隔离与同 uid 启动自检，是本 crate 把七公理变成运行时事实的五块承重墙。*

---

## 8. 验收标准

> 本节是 `postern-daemon` 的**验收基准**：拿这份清单可逐条判定开发实现的"功能写全没、逻辑对不对"。每条 = **要求 + 通过判定**，通过判定对当前代码只有"通过/不通过"一个答案，无歧义、可复现；判定方式按条目而定（行为观察 / 接口存在 / Stele 契约绿红 / 构造签名审查 / `postern verify` 项），不强求都是单元测试。
>
> 说明：本 crate 多条运行时安全语义（两阶段审计时序、TTL 求值时刻失效、escalate 折叠、归类拒绝、拒绝响应 Scope 有界、不做动词→tier 选择）在现行 24 条 Stele 契约（`contract/proposals/agent-additions.stele`：`DB_*`/`SEC_*`/`ARCH_*`/`EVAL_*` 各含 `_TEETH` 反例自检，全 24 条逐字列于第三组）中**无对应静态规则**——它们靠纯内存 `Evaluator` 驱动的行为用例（`cargo test` 内）与 `postern verify` 九项守住，故下文以**【行为观察】**标注、给出确切二元判定，不冒充 Stele 契约绿（§6/§7 提及的 `AUDIT_FAILURE_DENIES`/`TEMP_GRANT_EXPIRY_EFFECTIVE`/`ESCALATE_FOLDS_TO_DENY`/`DENY_RESPONSE_SCOPE_BOUNDED`/`NO_TIER_MATCH_DENIED` 是运行时行为契约语义，不在这 24 条静态规则内，故不写"绿"）。凡标**【人工】**者，是"crate 内职责越界 / 注入集合或类型签名细粒度"类命题——`stele check` 只覆盖跨 crate 依赖边与构造点，判不了 crate 内调用集与注入集合，须由构造函数签名 / 调用集审查逐项 yes/no（全 yes 才过）。`postern verify` 九项见详细设计 6.7，本 crate 是其运行载体。

### 一、功能完整性（判断：该有的功能都写了吗、行为对吗）

| 编号 | 要求（必须实现） | 通过判定（满足即过，否则不过） |
|---|---|---|
| F-1 boot 启动序列（§3.1·1-5） | 按固定时序拉起并 fail-closed 自检 | 启动 `posternd` 时序恰为「开库+校验 `user_version`/迁移版本/`settings` 表 → 重建首份 `Arc<PolicySnapshot>` → 解锁 `vault.postern` 并写 `lifecycle` 审计 → 注册插件 → 先建 `control.sock(0600)` 后建 `data.sock` 挂数据面 router」；任一前置不成立 → 进程非零退出、`data.sock` 未创建（场景 `docs/examples/02 §4.2 E9`） |
| F-2 `data.sock` 可连 uid 自检（§3.1·6） | 同 uid 即拒启动（硬前置条件②） | 构造 `data.sock` 可连 uid 集合**含 daemon 自身 uid** 的环境 → 启动被拒（非零退出、数据面未开放）；可连 uid 不含自身 uid → 正常开放。判据：`postern verify` 项「`data.sock` 可连 uid 不含 daemon 自身 uid」该条 PASS（详细设计 5.5/6.7） |
| F-3 kernel 管线 [0]→[10] 编排（§3.2） | 内核按管线逐步短路、[4] 先于 evaluate、写动词执行前先落 intent | 提交一条放行 query 请求 → 观测到的调用序恰为 `Adapter::classify`[2] → `Adapter::check_constraint`[4] → `Evaluator::evaluate`[1][3][5][6] → `acquire`[7b] → `Adapter::execute`[8] → `Sanitizer::scrub`[9] → `AuditSink::record`[10] 单条；同一请求若为有副作用动词（mutate）→ 在 `Adapter::execute` 之前多一条 [7a] intent `AuditSink::record`，只读动词（query/observe）则无 [7a]；`check_constraint` 的调用栈位置严格早于 `evaluate`（场景 `docs/examples/04 §4.1 Trace ①②③`）【行为观察】 |
| F-4 数据面外壳 HTTP+MCP（§3.3） | 两形态归一化为同一 `NormalizedRequest`、同管线 | 同一逻辑请求分别经 HTTP 与 MCP 入口 → [0] 后内核所见 `NormalizedRequest` 等价、决策/匿名化/脱敏/审计四项结果逐字段相同；MCP 工具集恰为固定动词工具 `postern_grants`/`postern_query`/`observe`/`mutate`/`execute`/`manage`/`destroy`/`postern_surface`，不随授权增删（场景 `docs/examples/04 §4.1`、`docs/examples/05 §2.1`）【行为观察】 |
| F-5 `postern_surface`（§3.3·CONS-20） | 返回授权快照投影，不触 discover | 调 `postern_surface()` → 返回该 Principal Scope 内已授权能力面（快照投影），且返回对象集 ⊆ 当前快照已授权对象（不含任何未圈选/未授权对象）；surface 代码路径**无 `Adapter::discover` 调用、无底层资源触达**（场景 `docs/examples/02 §4.2 E8`、`docs/examples/06 §3.3`，详细设计 6.8）【人工：surface 路径调用集审查 + 行为观察】 |
| F-6 control 控制面端点全集 + 审批（§3.4） | §6.5 端点全集可达、写端点三联动、集合分页、乐观锁 | 启动后经 `control.sock` 列出端点，恰覆盖 §6.5 全集（principals/credentials/roles/bindings/resources〔含 `POST /v1/resources/{code}/discover`〕/constraints/conditions/deny-notes/settings/grants·temp/mode/grants 视图/audit/denials·summary/**approvals**/export/import/verify/health/shutdown）；任一写端点 = 事务+快照重建+审计三事件齐发；任一集合端点缺 `page_no/page_size` → 用缺省 20、传 `page_size=300` → 实际钳到 200、返回 `Page<T>` 信封；更新带过期 `version` → `409 Conflict` 并落 `policy_change`（场景 `docs/examples/07 §4.2 E10`） |
| F-7 connpool 连接管理（§3.5） | 池键 `(资源,tier)`、取句柄即释、池化/退避/上限/中断/净化/审计 | 收 `Allow{tier}` → `acquire(resource, tier)` 用池键 `(ResourceCode, CredentialTier)`；建连时一次性取不透明句柄即时传入 `Transport::open`、调用边界外句柄不可再读；persistent 通路复用、断连按指数退避（基数 1s/上限 60s/带抖动）重连、非长连接即用即弃；落 `connection_event`（字段恰为 resource/tier 名/transport 种类，无真实地址/凭据）（场景 `docs/examples/04 §4.1 Trace ①[7b]`、`docs/examples/06 §4.2`）【行为观察】 |
| F-8 sweeper TTL 回收（§3.6） | 事务性回收过期记录、`actor=system`、回收后重建快照 | 构造一条 `expires_at < now` 的 `temp_grants`（及一条到期 `credentials.expires_at`、一条到期 `mode_state.expires_at`、一条超时审批项）后驱动一轮 sweeper → 该 `temp_grants` 行 `ended_at` 非空且 `end_reason=='expired'`、四类到期项均经事务回收、其 `created_by==updated_by=='system'`、对应落一条 `policy_change`/`mode_change` 审计、且快照在同一写锁临界区内被重建（重建后快照不含该过期项）；任一项不满足即不过（场景 `docs/examples/06 §4.1 A`）【行为观察】 |
| F-9 sanitize·Sanitizer 执行（§3.7） | 实现 core `Sanitizer`，小响应整体脱敏、流式滑窗；ScrubSet 仅持不透明句柄 | `sanitize/` 实现 `scrub(payload, declared)`/`scrub_stream(declared)`，签名与 core `Sanitizer` 一致；构造一段敏感串**跨 chunk 边界**的语料喂 `scrub_stream` → 输出该敏感串被擦除（保留上块尾部 N 字节参与下块匹配，N=ScrubSet 最长模式上界）；脱敏材料恰为机密面 ScrubSet 不透明句柄 + 声明级 `MaskRule`，且 `sanitize/` 持有的 ScrubSet 句柄类型**只暴露 match-and-erase**、无枚举/序列化方法（不可列出模式、不可 `Serialize`），ScrubSet 构造点不在本 crate（场景 `docs/examples/04 §4.1 Trace ③[9]`）【行为观察 + 人工：ScrubSet 句柄类型签名审查】 |
| F-10 出口唯一脱敏调用点（§3.2·§5.2） | `submit` 唯一入口、CatchPanic 兜底 | 内核唯一入口签名为 `submit(req: NormalizedRequest) -> Result<SanitizedResponse, DenyResponse>`；遍历正常响应/`execute` 错误/拒绝响应/外壳层 4xx/框架错误/panic 五类出口 → 每一类字节都经 `sanitize/` 同一 Sanitizer 后才离开内核，套 `{error:{code,message}}` 信封、`message` 仅常量安全文案（场景 `docs/examples/04 §4.2 C`、`docs/examples/07 §4.2 E4`）【人工：出口穷尽性 + 行为观察】 |

### 二、逻辑正确性（判断：关键逻辑、边界、失败处理对不对）

| 编号 | 要求（行为必须正确） | 通过判定 |
|---|---|---|
| L-1 两平面 socket 权限隔离 | control 仅属主、data 专用组；注入集合不同 | `stat control.sock` 的模式位**恰为** `0600`（仅属主可读写）；`stat data.sock` 的模式位**属于** {`0660` + 专用组属主} 这一允许集（Agent 与 daemon 不同 uid、经专用组放行）；`control.sock` 与 `data.sock` 是绑不同路径的两个独立 axum router；从 daemon 自身 uid 裸连 `control.sock` 不带控制面凭证 → 连接被拒/认证失败，须过控制面认证（`SO_PEERCRED` uid 比对 + 控制面专用本地凭证）方放行（场景 `docs/examples/06 §4.2-17`）【人工：socket stat + 连接尝试】 |
| L-2 数据面注入集合无 PolicyRepo/vault | `submit` 与数据面 router 构造点注入集合不含写路径与机密句柄 | `submit` 构造点注入集合**仅含** `PolicyView`（只读快照）/Adapter·Authenticator·Predicate 注册表/连接管理句柄/Sanitizer/AuditSink；其中**不出现** `PolicyRepo` 与 vault 句柄类型（红线 7.2-2）【人工：构造函数签名审查】 |
| L-3 两阶段审计时序：已执行绝不返 deny | 只读 deny / intent 写不进执行前 deny / outcome 失败=已执行但审计降级 | 注入 `AuditSink::record` 失败：①只读动词 record 失败 → 该请求返回 `Deny`；②有副作用动词 [7a] intent 写不进 → 执行前 `Deny`（`Adapter::execute` 未被调用）；③有副作用动词已 `execute` 后 [10] outcome 写失败 → 返回"已执行但审计降级"可识别错误码、**绝不返回 deny**（场景 `docs/examples/07 §4.2 E1/E2`）【行为观察】 |
| L-4 一切离开内核的字节经 Sanitizer（含 panic） | 无绕过出口；panic→脱敏 deny + `kind=anomaly` | 数据面 handler 内注入 panic → CatchPanic 层转为脱敏后的 `DenyResponse` + 落一条 `kind=anomaly` 审计，进程不崩、无不留痕失败路径（场景 `docs/examples/07 §4.2 E4`）【行为观察】 |
| L-5 求值任一步判拒 → 短路 deny 且 stage 正确 | 认证/归类/RBAC/细则/条件/建连失败各短路 | 分别注入：认证 `Err` → `Deny{stage=auth}`；不可归类 `ClassifyError` → `Deny{stage=classify}`；无授权格 → `Deny{stage=rbac}`；`ConstraintCheck{passed:false}` → `Deny{stage=constraint}`；条件谓词/模式不过 → `Deny{stage=condition}`；`acquire` 不可建 → `Deny{stage=connect}`；六种均不执行 [8]（场景 `docs/examples/05 §2.1`、`docs/examples/04 §4.2 A`）【行为观察】 |
| L-6 连接不可建 → deny（不降级、不改路） | `acquire` 失败即 deny，脱敏不含真实地址 | 注入 `Transport::open` 失败 → `acquire` 返回 `Err` → 上游返回 `decision=deny, stage=connect`，错误经脱敏、不含真实地址串；绝不静默重试到他路或降级放行（场景 `docs/examples/04 §4.2 D`）【行为观察】 |
| L-7 超并发上限 → 有界排队或 deny | 超限走有界排队或 deny，容量常量封顶 | 制造超每资源/全局上限 N 的并发 → 第 N+1 个请求的结果属于「在有界队列内等待至有空位后服务」或「立即 `decision=deny, stage=connect`」二者之一（fail-closed），不出现第三种；持续灌注远超 N 的 observe 大流时，排队/缓冲占用的观测峰值 `≤` 该资源/全局的常量队列上限 `Q`（与灌注量无关、触顶即背压或 `deny, stage=connect`）：占用超过 `Q` 即不过（场景 `docs/examples/04 §4.2 E`）【行为观察】 |
| L-8 不同 tier 不共享连接 | 账号隔离在连接粒度成立 | 同一资源、两个不同 `CredentialTier` 的请求 → 落在两个不同池槽（池键含 tier），永不复用同一底层连接（场景 `docs/examples/04 §4.1`）【行为观察】 |
| L-9 归池前会话净化（净化失败销毁不归池） | 复用前强制重置、失败即销毁 | 连接归池前强制重置会话态（如 PostgreSQL `DISCARD ALL`、重置 `search_path`、回滚未决事务、清临时表）；注入净化失败 → 该连接被销毁、**不归池**（fail-closed），下个请求拿到的是干净连接（场景 `docs/examples/04 §4.2 F`）【行为观察】 |
| L-10 freeze/吊销在飞强制 abort | 收紧时在用连接强制中断，非优雅排空 | 一条底层查询在飞时下达 freeze 或吊销 → 连接管理对相关 `(resource[, principal])` 在用连接强制 abort/cancel（取消查询、关隧道），写 `connection_event`；不等其优雅跑完（场景 `docs/examples/06 §4.2-6`）【行为观察】 |
| L-11 TTL 求值时刻即刻失效 | 失效不依赖 sweeper 时序 | 临时格 `expires_at` 已过但 sweeper 尚未回收时发起请求 → 求值在该时刻按墙钟二次校验 `expires_at >= now` 判定 → `Deny`；daemon 重启后用原临时格请求，TTL 按绝对墙钟续计、不重置不延长（场景 `docs/examples/06 §4.2-5`）【行为观察】 |
| L-12 escalate / 超时 / 重启挂起审批 → 恒 deny | 审批关或超时即 deny，绝不悬挂 | 命中 escalate 格且审批关闭 → 立即返回 `decision=escalate_denied`（与普通 deny 可区分），不挂起；`on_timeout` 固定 `deny`：设置/导入写入 `on_timeout=allow` 被拒；daemon 重启后一切挂起审批一律 deny（场景 `docs/examples/06 §4.2-9`、`docs/examples/05 §2.5`）【行为观察】 |
| L-13 拒绝只说自身世界、不泄露存在性 | Scope 外与不存在资源不可区分 | 分别请求"Scope 外但存在的资源"与"根本不存在的资源" → 两次 `DenyResponse` 完全相同（`stage=rbac`，`your_grants` 只含自身授权世界），不可区分（场景 `docs/examples/04 §4.2 I`、`docs/examples/07 §4.2 E5`，`postern verify` 项 4）【行为观察 + verify 项 4】 |
| L-14 内核对策略只读、写入唯一入口三联动 | 数据面无写路径；写=事务+快照重建+审计 | 数据面任何路径不持有 `PolicyRepo`、不产生策略写入；策略写入唯一经 `control`/`sweeper` 的 `PolicyRepo`，每次写入 = 事务 COMMIT + 快照重建 + 审计三者齐发，任一失败 → 不 COMMIT、不重建快照、整体失败（场景 `docs/examples/06 §4.2-10/14`）【行为观察 + 人工注入集合审查】 |
| L-15 系统自动机与人写同路同审计 | sweeper/import 走同一事务路径、`actor=system` | 分别驱动一次 sweeper 回收与一次 import 协调写 → 二者落库的写调用栈均经与控制面人写**同一** `PolicyRepo` 事务路径（写路径唯一性由 `DB_WRITE_PATH_CENTRALIZED` 绿背书）、各落与人写同形态的 `policy_change` 审计、写入行 `created_by==updated_by=='system'`（区别于人写的操作者标识）；正确性不依赖 sweeper 时序（过期判定在求值时刻墙钟，见 L-11）：任一项不满足即不过（场景 `docs/examples/06 §4.2-1`）【行为观察 + 人工：写路径调用集审查】 |
| L-16 red-team 自检载体（verify 九项运行） | 本 crate 自发九类应被拒请求走完整管线 | `POST /v1/verify` → daemon 以临时低权 Principal 自发详细设计 6.7 九类请求走完整 [0]→[10]：项 1-7、9 判据为 deny（含 `stage`+`reason`），项 8 脱敏探测判据为放行且响应擦净；九项均进审计；任一项判据不满足 → verify FAIL 并指出缺口防线（场景 `docs/examples/07 §4.1 C`、`docs/examples/02 §4.1`）【`postern verify` 九项】 |
| L-17 connpool 不做动词→tier 选择（§4 边界） | tier 选择归策略引擎，连接层只据已选 tier 取连接 | `acquire` 入参为已选定的 `(ResourceCode, CredentialTier)`，其调用栈内无任何「动词→tier」映射逻辑（不读 `Capability` 决定 tier）；构造一条 `Allow{tier=ro}` 与一条 `Allow{tier=op}` 仅 tier 不同的放行请求 → connpool 直接按入参 tier 取连接、不二次裁决 tier；tier 的产出点唯一在 `Evaluator::evaluate` 的 allow 路径（场景 `docs/examples/04 §4.1 Trace ①[6] tier 选择落策略引擎`）【人工：connpool 调用集审查 + 行为观察】 |

### 三、边界与不变量（机器强制，绿/红即答案）

| 编号 | 要求 | 通过判定（机器） |
|---|---|---|
| B-1 依赖图无禁止边 | daemon 可依赖全部下游，但 adapters/transports/cli/core 禁止边成立 | 契约 `ARCH_FORBIDDEN_EDGES`（+ `_TEETH`）绿；`cargo tree -p postern-cli -e normal` 无 `daemon`/`store`/`secrets` 边（cli 仅依赖 core + HTTP/UDS 客户端） |
| B-2 `ConnOrigin` 只在 shells listener 构造 | 来源采集不采信自报，构造点唯一 | 契约 `SEC_CONSTRUCTION_SITES`（+ `_TEETH`）绿（`ConnOrigin` 只在 daemon shells 构造）；场景 `docs/examples/06 §4.2-16`（自报 origin 不被采信） |
| B-3 不构造机密类型、机密不可 Clone/Serialize | 机密类型只在 secrets 构造、不可复制不可序列化 | 契约 `SEC_CONSTRUCTION_SITES` + `SEC_SECRET_TYPE_DISCIPLINE`（各 + `_TEETH`）绿（本 crate 无 `ResolvedTarget`/`ResourceCredential` 构造点；二者不 derive `Clone`/`Serialize`） |
| B-4 求值路径（kernel）禁吞错 | `kernel/` 无吞错放行写法 | 契约 `EVAL_NO_ERROR_SWALLOWING`（+ `_TEETH`）绿（扫描 `crates/postern-daemon/src/kernel/` 无 `.ok()`/`.unwrap_or(true)`/`.unwrap_or_default()`/`.unwrap_or_else(\|_\| true)`） |
| B-5 控制面写入纪律静态成立 | 写路径集中、逻辑删除、默认作用域、分页强制 | 契约 `DB_WRITE_PATH_CENTRALIZED`（+ `DB_WRITE_PATH_TEETH`）、`DB_LOGICAL_DELETE_ONLY`（+ `DB_LOGICAL_DELETE_TEETH`）、`DB_DEFAULT_SCOPE_EXCLUDES_DELETED`（+ `DB_DEFAULT_SCOPE_TEETH`）、`DB_NO_RAW_SQL_OUTSIDE_STORE`（+ `DB_RAW_SQL_TEETH`）、`DB_PAGINATION_MANDATORY`（+ `DB_PAGINATION_TEETH`）全绿；本 crate `control`/`sweeper` 不含 store 之外的裸 SQL、集合端点必带 `PageQuery` |
| B-6 id/基础字段统一 | 唯一雪花 id 来源、表带统一基础字段 | 契约 `DB_UNIFIED_ID_GENERATOR`（+ `DB_ID_GENERATOR_TEETH`）、`DB_BASE_FIELDS_REQUIRED`（+ `DB_BASE_FIELDS_TEETH`）全绿；`cargo deny check` 退出码 0（`uuid`/`ulid`/`nanoid` 连同传递依赖被 `[bans]` 拒入） |
| B-7 admin 不可授予 | admin 在类型/schema 层不可表达 | 契约 `SEC_ADMIN_NOT_GRANTABLE`（+ `_TEETH`）绿 |
| B-8 lint 红线 | 全 crate 无 unsafe / unwrap / expect / panic | `cargo clippy -p postern-daemon --all-features -- -D warnings` 退出码 0；`unsafe_code = forbid` 生效（`SO_PEERCRED` 经 `tokio UnixStream::peer_cred` 安全 API） |

### 通过定义（DoD）

`postern-daemon` **算完成** ⟺ 一、二、三三组**每一条都通过**。任一条不过 = 不通过，必须修。F 类靠"启动/管线/端点给定输入看行为是否符合通过判定"；L 类靠"触发某 fail-closed 条件→行为恰为某可观察结果"（含纯内存 `Evaluator` 驱动的行为用例与 `postern verify` 九项，标【行为观察】/【人工】者拆成逐项 yes/no、全 yes 才过）；B 类靠"跑契约/`cargo tree`/`cargo deny`/clippy 看绿红/退出码"。本 crate 同时是 `postern verify` 九项与场景 02~07 的运行载体，上述各条逐项通过即该模块完成。
