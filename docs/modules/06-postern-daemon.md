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

> 本节是 `postern-daemon` 的**验收基准**——每条给「输入→可观察的预期结果」判据与**验证方式**，让实现者据此自检、审查者据此判定该模块是否完成。按 A~F 六维度组织（与本模块相关者）；A 逐条对应 §3 功能、C 逐条对应 §4 边界、D 逐条对应 §7 不变量、E 逐条对应 §6 交互。本 crate 是二进制组装点且是 `postern verify` 红队项与全部场景 02~07 的**运行载体**，故许多条目同时挂场景规格与 verify 项。
>
> **验证方式用词约定**：`Stele契约 <名>` 一律指 `contract/proposals/agent-additions.stele` 里**静态扫描**的契约（`ARCH_*`/`SEC_*`/`DB_*`/`EVAL_*`，由 `stele check` 在编译期/依赖图/源文本上判定）；运行时语义（审计时序、TTL 求值生效、escalate 折叠、归类拒绝、拒绝响应 Scope 有界等）由**行为契约 `<名>`** 守住——其名见详细设计文档与场景规格（如 `AUDIT_FAILURE_DENIES`/`TEMP_GRANT_EXPIRY_EFFECTIVE`/`ESCALATE_FOLDS_TO_DENY`/`UNCLASSIFIABLE_INTENT_DENIED`/`DENY_RESPONSE_SCOPE_BOUNDED`，与 §7 标注一致），落地为 `cargo test` 内的 `test_contract` 行为用例，**不在 agent-additions.stele 静态契约集内**，故本节不以 `Stele契约` 标之。

### A. 功能完整性（对应 §3 各内部模块功能）

| # | 功能（§3） | 输入 → 可观察预期结果 | 验证方式 |
|---|---|---|---|
| A-1 | boot 启动序列（§3.1·1-5） | 启动 `posternd`：按「开库→校验 `user_version`/迁移版本/`settings` 表→重建首份 `Arc<PolicySnapshot>`→解锁 `vault.postern`（写 `lifecycle` 审计）→注册插件→先建 `control.sock(0600)` 后建 `data.sock` 并挂数据面 router」固定时序拉起；任一前置不成立 → 进程拒绝启动（非零退出）、数据面 socket 未开放 | 集成测试（真实资源：临时 `policy.db`+`vault.postern`）；场景规格 `docs/examples/02 §4.2 E9`（启动序列恢复） |
| A-2 | `data.sock` 可连 uid 自检（§3.1·6，硬前置条件②） | 构造 `data.sock` 可连 uid 集合**含 daemon 自身 uid** 的环境 → fail-closed 拒绝启动（或明知风险形态下强制告警）；可连 uid 不含自身 uid → 正常开放 | `postern verify`（`data.sock` 可连 uid 不含 daemon 自身 uid 项，详细设计 5.5）；集成测试（真实资源：socket 权限/可连 uid 自检）；场景规格 `docs/examples/02 §4.1`（接入后 verify 含同 uid 校验项） |
| A-3 | kernel 管线 [0]→[10] 编排（§3.2） | 外壳提交 `NormalizedRequest` → 内核按 [0]→[10] 逐步短路：[4] `check_constraint` 先于 `evaluate`、[6] 动作分流、出口脱敏；query/mutate/observe 三类入口骨架一致（公理七） | 集成测试（内存 Fake：Evaluator/Adapter/connpool/Sanitizer/AuditSink）；场景规格 `docs/examples/04 §4.1 Trace ①②③` |
| A-4 | 两阶段审计时序（§3.2） | 只读动词执行后单次审计，写失败按 deny；有副作用动词 [7a] intent 写不进→执行前 deny（确未执行），[10] outcome 写失败→返回"已执行但审计降级"错误码、**绝不返回 deny** | `行为契约 AUDIT_FAILURE_DENIES`（纯内存 Evaluator 驱动的 `test_contract` 用例）；场景规格 `docs/examples/07 §4.2 E1/E2`、`docs/examples/04 §4.1 Trace ②[7a][10]` |
| A-5 | 出口统一脱敏唯一调用点（§3.2） | 正常响应/错误/拒绝响应/外壳层 4xx/框架错误/panic 响应体——一切离开内核的字节经 `sanitize/` 同一 Sanitizer；数据面 router 挂 CatchPanic，panic→脱敏 deny + `kind=anomaly` 审计 | 集成测试（内存 Fake：注入 panic/错误路径）；场景规格 `docs/examples/04 §4.2 C`、`docs/examples/07 §4.2 E4` |
| A-6 | shells/http + shells/mcp 外壳（§3.3） | HTTP 与 MCP 两形态请求 [0] 归一化为同一 `NormalizedRequest` 后鉴权/匿名化/脱敏/审计结果完全一致；MCP 工具集为固定动词工具 `postern_query/observe/mutate/execute/manage/destroy/grants/surface`，描述只含事实、不随授权增删 | 集成测试（内存 Fake）；场景规格 `docs/examples/04 §3 数据面`（入口对称）、`docs/examples/05 §2.1` |
| A-7 | `postern_surface`（§3.3·CONS-20） | 调 `postern_surface()` → 返回该 Principal Scope 内已授权能力面（授权快照投影）；**不触达 `Adapter::discover`**、不触底层资源 | 构造签名审查（surface 路径无 `Adapter::discover` 调用）；场景规格 `docs/examples/04 §4.2 G`、`docs/examples/06 §3.3` |
| A-8 | control 控制面端点全集 + 审批挂起（§3.4） | 启动后经 `control.sock` 可达 §6.5 端点全集（principals/credentials/roles/bindings/resources(含 `discover`)/constraints/conditions/deny-notes/settings/grants·temp/mode/audit/denials·summary/**approvals**/export/import/verify/health/shutdown）；每写端点=一次事务+快照重建+审计三联动；集合端点强制分页（缺省 20、上限钳 200、`Page<T>` 信封）；读返 `version`、更新/删除携期望 `version`，不匹配 → `409 Conflict` 并写 `policy_change` 审计 | `Stele契约 DB_PAGINATION_MANDATORY`；集成测试（真实资源：`policy.db`）；场景规格 `docs/examples/06 §4.2-10`、`docs/examples/07 §4.2 E10`（乐观锁 409） |
| A-9 | connpool 连接管理（§3.5） | 收 `Allow{tier}` → `acquire(resource, tier)`：池键 `(ResourceCode, CredentialTier)`、不同 tier 不共享连接；向机密面一次性取不透明句柄即时传入 `Transport::open`、调用边界外即时释放；persistent 池化复用+指数退避（基数 1s/上限 60s/抖动）；超限有界排队或 deny；freeze/吊销时在用连接强制 abort/cancel；归池前会话净化（净化失败销毁不归池）；落 `connection_event`（resource/tier 名/transport 种类，不含真实地址/凭据） | 集成测试（内存 Fake：Transport/CredentialProvider）；场景规格 `docs/examples/04 §4.1 Trace ①[7b]、§4.2 E/F`、`docs/examples/06 §4.2-6` |
| A-10 | sweeper TTL 回收（§3.6） | 后台事务性回收 `temp_grants`（写 `ended_at`/`end_reason=expired`）、`credentials.expires_at`、`mode_state.expires_at`、审批超时项，`actor=system`，写 `policy_change`/`mode_change`，回收后同写锁内重建快照；正确性不依赖 sweeper 时序——过期判定在求值时刻墙钟二次校验 | `行为契约 TEMP_GRANT_EXPIRY_EFFECTIVE`；集成测试（真实资源：`policy.db`）；场景规格 `docs/examples/06 §4.1 A、§4.2-1` |
| A-11 | sanitize·Sanitizer 执行（§3.7） | `scrub` 整体脱敏小响应/错误/拒绝响应；`scrub_stream` 滑动重叠窗口（保留上块尾部 N 字节，N=ScrubSet 最长模式上界）防跨 chunk 逃逸、有界缓冲+背压；脱敏材料=机密面 ScrubSet 不透明句柄 + 声明级 `MaskRule` | 集成测试（内存 Fake：ScrubSet 句柄；构造跨 chunk 边界的敏感串语料）；场景规格 `docs/examples/04 §4.1 Trace ③[9]、§4.2 C` |

### B. 对外接口契约（对应 §5）

> 本 crate 是二进制，不向其他 workspace crate 暴露库接口——`postern-cli` 只依赖 `core` + HTTP/UDS 客户端、**不依赖本 crate**（依赖图，由 `cargo tree` 核验无 `cli → daemon` 库依赖边）；`ARCH_FORBIDDEN_EDGES` 另行兜底 `cli ↛ store/secrets`。其"对外接口"是**进程对外形态**与**内部模块对内契约**。

| # | 接口（§5） | 输入 → 可观察预期结果 | 验证方式 |
|---|---|---|---|
| B-1 | 进程对外形态·两平面 socket（§5.1） | `data.sock`=`0660`/专用组（Agent 可连的唯一入口）；`control.sock`=`0600`（仅属主）+ 控制面认证（`SO_PEERCRED` uid 比对 + 控制面专用本地凭证）；二者为独立 axum router | 构造签名审查（两 router 绑不同 socket、权限位）；集成测试（真实资源：socket stat/连接尝试）；场景规格 `docs/examples/06 §4.2-17`、`docs/examples/07 §3 数据面（不适用）` |
| B-2 | kernel 唯一入口 `submit`（§5.2） | `submit(req: NormalizedRequest) -> Result<SanitizedResponse, DenyResponse>`：步骤 [0] 之后与外壳无关；返回**已脱敏**结果或结构化拒绝；依赖注入集合**仅含** `PolicyView`/Adapter·Authenticator·Predicate 注册表/连接管理句柄/Sanitizer/AuditSink——**不含 PolicyRepo 与 vault 句柄** | 构造签名审查（`submit` 构造点注入集合）；集成测试（内存 Fake）；与 D-1 同源 |
| B-3 | connpool `acquire`（§5.2） | `acquire(resource: &ResourceCode, tier: &CredentialTier) -> Result<Channel, AcquireError>`：返回一个可用通路或结构化错误；取不到/不可建 → `Err` → 上游 deny | 集成测试（内存 Fake：Transport open 成功/失败）；场景规格 `docs/examples/04 §4.2 D` |
| B-4 | sanitize 实现 core `Sanitizer`（§5.2） | `scrub(payload, declared)` / `scrub_stream(declared)` 签名与 core 定义一致；语义符合 §3.7 承诺 | 构造签名审查（impl 与 trait 签名一致）；单元测试（脱敏行为） |

### C. 边界（禁止项，对应 §4「不做什么」）

> 每条给「确实没做 X / 无 X 代码路径」的可验判据；能用契约/依赖图/构造签名机器验证者优先标注。

| # | 边界（§4）「不做什么」 | 可验判据 | 验证方式 |
|---|---|---|---|
| C-1 | 不做 tier 选择（动词→tier 归策略引擎） | connpool 只接收已选 `Allow{tier}` 取连接，无任何"动词→tier"映射代码路径 | 构造签名审查（`acquire` 入参已含 `tier`，无 verb 入口）；场景规格 `docs/examples/04 §4.1 Trace ①[6]` |
| C-2 | 不构造 ScrubSet（构造/持有归机密面） | `sanitize/` 只持不透明 match-and-erase 句柄，不可枚举、不可序列化 ScrubSet；本 crate 无 ScrubSet 构造点 | 构造签名审查（`sanitize/` 无 ScrubSet 构造点，只接收机密面签发的不透明句柄；ScrubSet 不可枚举/序列化由机密面侧类型纪律保证，daemon→secrets 是允许边，非本契约可判，故此处靠签名审查兜底） |
| C-3 | 不构造机密类型 `ResolvedTarget`/`ResourceCredential`（归机密面） | 本 crate 无 `ResolvedTarget`/`ResourceCredential` 构造点；句柄即时传入 `Transport::open` 后释放，无明文读取路径 | `Stele契约 SEC_CONSTRUCTION_SITES`（机密类型只在 postern-secrets 构造）；构造签名审查 |
| C-4 | 不持有 `PolicyRepo`/PolicySnapshot 构建（写路径/快照构建归存储层） | 数据面注入集合无 `PolicyRepo`；本 crate 只消费 `PolicyView::snapshot()` 只读快照，不构建快照 | 构造签名审查（数据面 router 注入集合）；`Stele契约 ARCH_FORBIDDEN_EDGES` |
| C-5 | 不做单条通路的物理建立/保活/关闭执行（归传输） | connpool 只下达 open/close/abort 指令，物理执行落在 `Transport`/`Channel`；本 crate 不实现心跳/续约 | 构造签名审查（connpool 经 `Transport`/`Channel` 行使，无物理 IO）；场景规格 `docs/examples/06 §2.4`（决策者/执行者分离） |
| C-6 | 不做 `classify`/`check_constraint`/`execute`/`discover` 协议逻辑（归适配器） | kernel/control 只调 `Adapter` 方法，不内含协议意图解析；适配器只见 `Channel`、不可达地址/凭据 | `Stele契约 ARCH_FORBIDDEN_EDGES`（`adapters ↛ secrets/transports/store`，组装点把已脱地址通路交适配器使该禁止边运行时成立）；构造签名审查（kernel/control 仅调 `Adapter::classify/check_constraint/execute/discover`，无协议解析代码）；场景规格 `docs/examples/04 §4.1`（适配器经获取的 `Channel` 执行、不可达地址/凭据） |
| C-7 | 不定义审计 schema / 记录纪律定义 / 网关凭证语义规则（归 core/store 与身份域） | 本 crate 只执行纪律（落 `AuditEvent`）、注册认证器，无 schema/规则定义代码 | 构造签名审查（无 schema 定义、纪律执行经 `AuditSink`）；`Stele契约 ARCH_FORBIDDEN_EDGES` |
| C-8 | 不做 TTL 过期判定（归策略引擎求值时刻墙钟），sweeper 只做可见性回收 | sweeper 无"是否过期"的求值判据，仅按 `expires_at < now` 回收+留痕；求值时刻墙钟二次校验在 core | `行为契约 TEMP_GRANT_EXPIRY_EFFECTIVE`（墙钟判定在求值）；场景规格 `docs/examples/06 §2.1` |
| C-9 | 不承载控制面瘦客户端/SPA/桌面壳/stdio 桥客户端入口（归 cli + 桌面外壳） | 本 crate 是被调方（control 端点），无客户端渲染/桥客户端入口代码 | `Stele契约 ARCH_FORBIDDEN_EDGES`（`cli ↛ store/secrets`；cli 不依赖 daemon）；场景规格 `docs/examples/04 §3 CLI`（数据面发起非 CLI 职责） |

### D. 必守不变量（对应 §7，沿用 §7 已标强制手段）

| # | 不变量（§7） | 验证判据 | 验证方式 |
|---|---|---|---|
| D-1 | 数据面 handler 注入集合**不含** PolicyRepo 与 vault 句柄（只有 `PolicyView`/`AuditSink`/连接池/Sanitizer/注册表） | `submit` 与数据面 router 构造点注入集合不出现 `PolicyRepo`/vault 类型 | `构造签名审查`（红线 7.2-2）；`Stele契约 ARCH_FORBIDDEN_EDGES`（跨 crate 禁止边） |
| D-2 | 两平面 socket 权限隔离：`control.sock=0600`、`data.sock=0660`/专用组；二独立 router、注入集合不同 | socket 创建顺序与权限位符合 §5.1；两 router 注入集合机器可区分 | `构造签名审查`；集成测试（真实资源：socket 权限位） |
| D-3 | 同 uid 前置条件：`data.sock` 可连 uid 含 daemon 自身 uid → 拒绝启动；`control.sock` 叠加同 uid 也成立的控制面认证 | 同 uid 环境启动被拒；同 uid 直连 `control.sock` 须过控制面认证才放行 | `postern verify`（从模拟 Agent uid 连 `control.sock` 必须失败项，详细设计 5.5）；集成测试（真实资源：`control.sock` 连接尝试）；场景规格 `docs/examples/06 §4.2-17` | |
| D-4 | `ConnOrigin` 只由 `shells` listener 层构造，绝不采信请求自报字段 | listener 外无 `ConnOrigin` 构造点；自报 origin 字段不被采信 | `Stele契约 SEC_CONSTRUCTION_SITES`（`ConnOrigin` 只在 daemon shells 构造）+ 反例自检；场景规格 `docs/examples/06 §4.2-16` |
| D-5 | 审计相对执行时序：只读 deny / intent 写不进执行前 deny / 已执行绝不返回 deny / outcome 失败返回"已执行但审计降级" | 三类写失败路径行为符合 §3.2 时序 | `行为契约 AUDIT_FAILURE_DENIES`（纯内存 Evaluator 驱动的 `test_contract` 用例）；场景规格 `docs/examples/07 §4.2 E1/E2` |
| D-6 | 一切离开内核的字节必经 Sanitizer，套 `{error:{code,message}}` 信封、`message` 仅常量安全文案 | 不存在绕过 Sanitizer 的输出路径；CatchPanic 兜底 panic 响应体 | `构造签名审查`（内核唯一出口，红线 7.2-3）；集成测试（内存 Fake：遍历正常/错误/拒绝/panic 出口）；场景规格 `docs/examples/07 §4.2 E4` |
| D-7 | 求值路径（`kernel/`）禁吞错放行（`.ok()`/`.unwrap_or(true)`/`.unwrap_or_else(\|_\| true)`/`.unwrap_or_default()`） | `crates/postern-daemon/src/kernel/` 无吞错放行写法 | `Stele契约 EVAL_NO_ERROR_SWALLOWING`（扫描 kernel 路径）+ 反例自检；`clippy unwrap_used/expect_used`（-D warnings） |
| D-8 | 不同 tier 不共享连接；无法建通路 → deny；超限绝不静默放行 | 池键含 `CredentialTier`；建连失败/超限均解析为 deny 或有界排队 | 集成测试（内存 Fake：Transport open 失败、并发超限）；场景规格 `docs/examples/04 §4.2 D/E` |
| D-9 | 归池前会话净化为不变量——净化失败的连接销毁而不归池 | 复用前重置会话态；净化失败 → 销毁不归池（fail-closed） | 集成测试（内存 Fake：注入净化失败）；场景规格 `docs/examples/04 §4.2 F` |
| D-10 | 内核对策略状态只读；策略写入唯一入口 `control`/`sweeper` 经 `PolicyRepo`，写入=事务+快照重建+审计三联动 | 数据面无 `PolicyRepo`；写端点三联动缺一即整体失败 | `构造签名审查`（平面隔离注入）；集成测试（真实资源：`policy.db`）；场景规格 `docs/examples/06 §4.2-14`（三联动任一失败整体失败） |
| D-11 | 系统自动机（sweeper/import）与人写走同一事务路径、同等审计（`actor=system`）；正确性不依赖 sweeper 时序 | 系统协调写经同一 `PolicyRepo` 路径、落同等审计；过期判定在求值时刻墙钟 | `行为契约 TEMP_GRANT_EXPIRY_EFFECTIVE`；集成测试（真实资源）；场景规格 `docs/examples/06 §4.2-1` |
| D-12 | `on_timeout` 恒 `deny`（导入校验与设置写入两处都拒 allow）；daemon 重启时一切挂起审批恒 deny | 设置/导入写入 `on_timeout=allow` 被拒；重启后挂起审批一律 deny | `行为契约 ESCALATE_FOLDS_TO_DENY`；场景规格 `docs/examples/06 §4.2-9`、`docs/examples/05 §2.5` |
| D-13 | `unsafe_code=forbid`（全 crate）；`SO_PEERCRED` 走安全 API（`tokio UnixStream::peer_cred`） | 本 crate 无 `unsafe`；peer_cred 经安全 API | `clippy`（workspace lints，`unsafe_code=forbid`，CI `-D warnings`）；`构造签名审查`（peer_cred 调用点） |

### E. 与相邻模块交互（对应 §6，每条按 方向/类型/时机/失败语义=fail-closed 可验）

| # | 交互（§6） | 方向·类型·时机 → 失败语义可验判据 | 验证方式 |
|---|---|---|---|
| E-1 | ← core `Evaluator`（§6.1） | daemon→core；`kernel` 在 [4] `check_constraint` 后调 `evaluate(&NormalizedRequest,&ClassifiedIntent,&ConstraintCheck,&PolicySnapshot,now)`（步骤 [1][3][5][6]）→ 一切 `Err` 解析为 deny、`Deny`/`Escalate(fallback)` 经出口脱敏 | `Stele契约 EVAL_NO_ERROR_SWALLOWING`（kernel 路径）；集成测试（内存 Fake：Evaluator 返 Err/Deny/Escalate）；场景规格 `docs/examples/05 §2、§4.1` |
| E-2 | ← store `PolicyView`（§6.2 数据面读） | `kernel`→store；每请求 [1]~[6] 读 `snapshot()->Arc<PolicySnapshot>`（无锁、不含 vault）→ 读路径无失败点；快照不可得（极端启动）→ boot fail-closed 拒绝启动 | 集成测试（内存 Fake：`PolicyView`）；与 A-1 同源（boot 快照不可得→拒启） |
| E-3 | ← store `PolicyRepo`（§6.2 控制面写，仅 control/sweeper 可达） | `control`/`sweeper`→store；每次策略变更事务读写，COMMIT 后同写锁内重建快照 → 事务失败/版本冲突 → 不 COMMIT、不重建、返 `409`/错误并写审计；**`PolicyRepo`/vault 绝不注入数据面 router** | `构造签名审查`（红线 7.2-2，数据面无 `PolicyRepo`）；集成测试（真实资源：`policy.db`）；场景规格 `docs/examples/07 §4.2 E10`（409） |
| E-4 | ← store `AuditSink`（§6.2 观测面落地） | `kernel`/`connpool`/`control`/`sweeper`→store；[7a]/[10]/建连·剔除·回收/写入后调 `record(AuditEvent)`（写入前过 Sanitizer）→ 只读 deny / intent 写不进执行前 deny / outcome 失败"已执行但审计降级"、绝不 deny | `行为契约 AUDIT_FAILURE_DENIES`；集成测试（内存 Fake：注入 record 失败）；场景规格 `docs/examples/07 §4.2 E1/E2` |
| E-5 | ← secrets `MasterKeySource`（§6.3 启动解锁） | `boot`→secrets；启动序列第 3 步（开放数据面前）调 `obtain()` 得 `Zeroizing<[u8;32]>` → 解锁失败/格式版本不识别 → fail-closed 拒绝启动，写 `lifecycle` 审计 | 集成测试（真实资源：`vault.postern`，注入坏 payload/版本）；场景规格 `docs/examples/02 §4.2 E9`、与 A-1 同源 |
| E-6 | ← secrets `CredentialProvider`+resolve（§6.3 建连取句柄） | `connpool`→secrets；步骤 [7b] 调 `credential_for(res,tier)`/`resolve(code)` 得不可 Clone/Serialize、`Debug=REDACTED` 句柄（生命周期不出本次建连）→ 取不到/解析失败 → `acquire` 错误 → [7b] deny；本 crate 不可读明文 | `Stele契约 SEC_SECRET_TYPE_DISCIPLINE`（机密类型禁 derive Clone/Serialize）+ `SEC_CONSTRUCTION_SITES`（禁本 crate 构造）；集成测试（内存 Fake）；场景规格 `docs/examples/04 §4.2 D/G` |
| E-7 | ← secrets ScrubSet 句柄（§6.3 脱敏材料） | secrets→`sanitize/`；解锁后获取、保险箱写入更新后刷新；步骤 [9] 应用 → 句柄不可枚举/序列化，内容永不出现在任何输出路径 | 构造签名审查（`sanitize/` 只持不透明句柄）；集成测试（内存 Fake：ScrubSet）；场景规格 `docs/examples/04 §4.2 C`、`docs/examples/07 §4.2 E4` |
| E-8 | ← transports `open`/健康/关闭（§6.4） | `connpool`→transports；步骤 [7b] 传 `ResolvedTarget`/`ResourceCredential` 入 `open` 得 `Channel`；freeze/吊销时 abort、回收时关闭 → `open` 失败→`acquire` 失败→deny；重建/退避归本 crate、传输只报通路死亡；跨边界错误已脱敏为不含真实地址（红线 7.2-1） | 集成测试（内存 Fake：Transport open 成功/失败/上报死亡）；场景规格 `docs/examples/04 §4.2 D`、`docs/examples/06 §4.2-6` |
| E-9 | ← adapters `classify`/`check_constraint`/`execute`/`discover`（§6.5） | `kernel`→adapters：[2] `classify`（无法归类→`ClassifyError`→deny，白名单宁误拒）、[4] `check_constraint`（`false`/`Err`→deny 短路）、[8] `execute`（错误经出口脱敏返回、已执行绝不 deny）；`control`→adapters：`discover`（接入侧探测，发现≠授权，绝不暴露给数据面） | `行为契约 UNCLASSIFIABLE_INTENT_DENIED`（classify Err 恒 deny）+ `Stele契约 ARCH_FORBIDDEN_EDGES`（适配器只见 Channel）；集成测试（内存 Fake）；场景规格 `docs/examples/04 §4.2 A`、`docs/examples/02 §4.1`（discover） |
| E-10 | ← cli（§6.6 被调方，经控制面 API） | cli→`control`（HTTP/JSON over `control.sock`）；每条 `postern ...` 管理命令=一次控制面调用 → 认证不通过/同 uid 自检语义→拒绝服务该连接；统一 `{error:{code,message}}` 常量安全文案；`cli ↛ store/secrets` | `Stele契约 ARCH_FORBIDDEN_EDGES`（`cli ↛ store/secrets`）；集成测试（真实资源：control.sock 连接尝试）；场景规格 `docs/examples/06 §4.2-12/17` |

### F. 失败与边界行为（关键 fail-closed 路径逐条可验）

| # | fail-closed 路径 | 触发 → 可观察预期结果 | 验证方式 |
|---|---|---|---|
| F-1 | 启动前置不成立 → 拒绝启动 | 开库失败/迁移版本不符/`settings` 未知 key/快照不可得/解锁失败/同 uid 自检失败 → 进程拒绝启动，数据面 socket 未开放 | 集成测试（真实资源）；`postern verify`（`data.sock` 可连 uid 不含 daemon 自身 uid 项，详细设计 5.5）；场景规格 `docs/examples/02 §4.2 E9` |
| F-2 | 求值任一步判拒 → 短路 deny | [1] 认证失败(`stage=auth`)/[2] 不可归类(`stage=classify`)/[3] 无格(`stage=rbac`)/[4] 细则不过(`stage=constraint`)/[5] 条件/模式不过(`stage=condition`)/[7b] 不可建(`stage=connect`) → 结构化 `DenyResponse`、不执行 [8] | 集成测试（内存 Fake）；场景规格 `docs/examples/05 §2.1 stage 分流`、`docs/examples/04 §4.2 A/D` |
| F-3 | 连接不可建 → deny（不降级/不静默重试到他路） | `acquire` 无法建到 `(resource,tier)` 通路 → `decision=deny, stage=connect`（脱敏不含真实地址） | 集成测试（内存 Fake：open 失败）；场景规格 `docs/examples/04 §4.2 D` |
| F-4 | 超并发上限 → 有界排队或 deny | 超每资源/全局上限 → 有界排队后服务或确定 deny；observe 大流有界缓冲+背压；绝不无界堆积 | 集成测试（内存 Fake：制造超限/慢消费）；场景规格 `docs/examples/04 §4.2 E` |
| F-5 | 审计不可记 → 不放行（两阶段时序） | 只读 record 失败→deny；intent 写不进→执行前 deny（确未执行）；outcome 失败→"已执行但审计降级"错误码非 deny | `行为契约 AUDIT_FAILURE_DENIES`；场景规格 `docs/examples/07 §4.2 E1/E2` |
| F-6 | 模式/吊销/TTL 收紧 → 在飞强制中断 | freeze/吊销时对相关 `(resource[,principal])` 在用连接强制 abort/cancel（非优雅排空），写 `connection_event` | 集成测试（内存 Fake：在飞连接 + freeze 信号）；场景规格 `docs/examples/06 §4.1 C、§4.2-6` |
| F-7 | escalate / 超时 / 重启挂起审批 → 恒 deny | 审批关闭/超时/重启挂起 → `decision=escalate_denied` 或 deny，绝不悬挂等待 | `行为契约 ESCALATE_FOLDS_TO_DENY`；场景规格 `docs/examples/05 §2.5/§4.2 C`、`docs/examples/06 §4.2-9` |
| F-8 | panic → 脱敏 deny + `kind=anomaly` 审计 | 数据面 handler panic → CatchPanic 转脱敏 deny 响应 + 一条 `kind=anomaly` 审计，绝不成为不留痕失败路径 | 集成测试（内存 Fake：注入 panic）；`clippy panic/unwrap_used/indexing_slicing`（收窄 panic 源）；场景规格 `docs/examples/07 §4.2`（审计连续性） |
| F-9 | Scope 外/不存在资源探测 → deny 且不泄露存在性 | 调 Scope 外资源代号 → [3] deny，拒绝响应 `your_grants` 只含自身世界，不区分"不存在/无权" | `行为契约 DENY_RESPONSE_SCOPE_BOUNDED`；`postern verify`（Scope 外访问项 4）；场景规格 `docs/examples/04 §4.2 I`、`docs/examples/07 §4.2 E5` |
| F-10 | red-team 自检载体（本 crate 承载 verify 全九项运行） | `POST /v1/verify` → daemon 以临时低权 Principal 自发九类应被拒请求走完整 [0]→[10]，八项 deny+1 项脱敏放行均进审计；任一项未被拒 → verify FAIL 指出缺口 | `postern verify`（九项全集）；场景规格 `docs/examples/07 §4.1 C、§4.2 E6`、`docs/examples/02 §4.1`（接入后 verify） |

### 供应链与构建门禁（贯穿全 crate）

| # | 判据 | 验证方式 |
|---|---|---|
| G-1 | 工作区不引入 `uuid`/`ulid`/`nanoid` 等替代 id 库（连同传递依赖）；id 一律雪花字符串序列化 | `cargo deny`（`[bans]` 列 id 库含传递依赖）；`Stele契约 DB_UNIFIED_ID_GENERATOR`（直接依赖文本扫描） |
| G-2 | 依赖图无禁止边（daemon 可依赖全部下游，但数据面注入集合不含 PolicyRepo/vault；adapters/transports/cli/core 禁止边成立） | `Stele契约 ARCH_FORBIDDEN_EDGES` + 反例自检；`cargo tree`（冗余校验） |
| G-3 | CI 门禁全绿：`cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo hack check --each-feature`、`cargo test --workspace`（含 `test_contract`）、`cargo deny check`、`stele check` | `clippy`/`cargo tree|deny`/`postern verify`（发布前必跑） |

### 需人工审查（无法完全机器验证）项

以下条目部分判据**只能靠构造签名审查或人工审查**，标出以便审查者重点核验（其余可由契约/lint/测试机器验证）：

- **B-2 / D-1 / E-3**：数据面注入集合"不含 PolicyRepo/vault 句柄"由**构造签名审查**判定（`ARCH_FORBIDDEN_EDGES` 仅保证 crate 级依赖不被禁，无法判注入集合的细粒度——daemon 合法依赖 store/secrets，注入边界靠签名审查守）。
- **C-1 / C-5 / C-7**：tier 选择缺位、通路物理执行外移、无 schema/纪律定义——属"无此代码路径"的**构造签名审查**判据，契约只覆盖跨 crate 依赖边、不覆盖 crate 内职责越界。
- **A-5 / D-6**："不存在任何绕过 Sanitizer 的输出路径"是全路径穷尽性命题：集成测试遍历已知出口可验，但"无遗漏出口"终需**构造签名审查**（内核为唯一出口 + CatchPanic 兜底）佐证。
- **D-9 / A-9（会话净化、在飞 abort）**：运行期行为，依赖**集成测试（内存 Fake）**注入净化失败/在飞中断场景；非静态契约可判。

### 完成定义（Definition of Done）

**当 A（11 项功能全部实现且各按「输入→预期可观察结果」可验）、B（4 项对外/对内契约签名稳定且语义符合）、C（9 项边界确无越界代码路径）、D（13 项不变量各由其标注的 Stele契约/clippy/构造签名/行为契约守住）、E（10 项相邻交互按约定方向/类型/时机调用且失败语义均 fail-closed）、F（10 项关键 fail-closed 路径逐条可观察可验）六维度全部满足，且供应链与构建门禁全绿、本 crate 作为 `postern verify` 九项与场景 02~07 运行载体逐条通过——即视为 `postern-daemon` 模块完成。**
