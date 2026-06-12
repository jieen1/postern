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

**怎么做 — 依赖顺序即安全顺序**：上述六步是一条**单线程顺序启动链**，顺序本身是安全不变量而非便利。每步产出下一步的输入：开库产出 `Connection` 与 schema 版本判定 → 重建快照需要已迁移的库 → 解锁保险箱在快照之后（机密面据 `targets`/`secrets` 构造 ScrubSet，与策略状态无依赖但须在数据面开放前就绪）→ 注册插件把 trait 对象装入注册表（注册表是内核与连接管理构造的入参）→ 最后才**开放数据面**。设计取舍：**数据面 socket 必须最后创建**——若提前 bind `data.sock`，则在快照/保险箱/连接池任一未就绪的窗口内，落到 handler 的请求会撞上半装配状态，fail-closed 在此退化为"先开门再装锁"。因此 boot 把"创建 `data.sock` 并 `serve`"作为整链的**唯一收尾动作**，此前任一步返回 `Err` 都在 socket 创建前短路（进程非零退出、`data.sock` 不存在），对外表现为"要么完整可用、要么根本不接客"。
- **socket 创建次序**：先 `control.sock`（`0600`）后 `data.sock`，二者各自 `bind` 后**立即 `chmod`/设属组再 `listen`**，消除"默认 umask 下短暂可连"的竞态窗口。
- **可连 uid 自检的判定形态**：自检是对"`data.sock` 在当前 umask/属组/ACL 下哪些 uid 能 `connect`"的有效集合判定，**不是**读请求自报——它在 socket 创建后、`serve` 前执行，集合含自身 uid 即在开放前 fail-closed。
- **首份快照的语义**：boot 通过 `PolicyRepo` 在一次事务内物化首份 `Arc<PolicySnapshot>`，此后数据面只经 `PolicyView::snapshot` 消费只读投影；`PolicyRepo` 句柄随即只交给 `control`/`sweeper`，**绝不进入数据面 router 的注入集合**（红线 7.2-2 在装配处即落地）。

### 3.2 kernel — 数据面请求内核

对每个外壳提交的 `NormalizedRequest`，按管线 [0]→[10] 串到底并逐步短路（管线步骤与失败语义见第六部分 6.1）：

- **[0] 之后请求与外壳无关**（公理七）：内核只见 `NormalizedRequest`，不知其来自 HTTP 还是 MCP。
- **[4] 细则先行**（CONS-8）：在调用 `Evaluator::evaluate` **之前**先跑 `Adapter::check_constraint`，把结果物化为 `ConstraintCheck` 作入参传入 `evaluate`——保证 `evaluate` 仍是 core 零 IO 纯逻辑。
- **[6] 动作分流**：`Decision::Allow{tier}` 进执行路径；`Decision::Deny` 与 `Decision::Escalate`（审批关闭即取 fallback 恒 deny）走拒绝出口；二者**同等经出口脱敏**。
- **两阶段审计时序（安全不变量，见 6.1）**：只读动词（observe/query）执行后单次审计，写失败按 deny 返回；有副作用动词（mutate/execute/manage/destroy）执行前先落 **intent** 审计（[7a]），intent 写不进则执行前 deny（此时确未执行），执行后落 **outcome** 审计（[10]），**已执行的请求绝不返回 deny**，outcome 写失败返回"已执行但审计降级"的可识别错误码。
- **出口统一脱敏的调用职责**：内核是 Sanitizer 的**唯一调用点**——正常响应、错误信息、拒绝响应、审计事件正文，一切离开内核的字节都经 `sanitize/` 模块过同一 Sanitizer，再交外壳格式化。
- **CatchPanic**：数据面 router 挂 CatchPanic 层，任何 panic 一律转为脱敏后的 deny 响应 + 一条 `kind=anomaly` 审计，绝不让 panic 成为不留痕的失败路径。

**怎么做 — 管线编排的数据流与短路形态**：内核把 [0]→[10] 实现为一条**线性短路链**，每步要么产出下一步入参、要么以确定 `stage` 的 `DenyResponse` 提前返回。关键数据沿链单向流动：`NormalizedRequest`（含出示物、`ConnOrigin`、资源代号、原始 Intent）→ `classify` 得 `ClassifiedIntent` →（先行）`check_constraint` 得 `ConstraintCheck` → `evaluate(req, ci, constraint_check, snapshot, now)` 得 `(Decision, EvalTrace)` → `Allow{tier}` 时 `acquire(resource, tier)` 得 `Channel` → `execute(&mut ch, intent)` 得 `RawResponse` → `scrub` → 审计。短路用 Rust 的 `?`/early-return 表达，**每个失败分支都显式带上该步的 `stage`**（auth/classify/rbac/constraint/condition/connect），杜绝"统一兜底 deny 但丢失 stage"——`stage` 是 deny 审计可还原"卡在哪一步"的承重字段（公理六）。
- **为何 [4] 细则先行（CONS-8 的工程理由）**：`check_constraint` 是适配器的 IO/语义判断（可能要解析 SQL 语法树），若把它放进 `evaluate` 内部，`evaluate` 就不再是 core 的零 IO 纯函数、无法用纯内存快照单测。故内核把这步**外提**到 `evaluate` 之前跑完，只把布尔化的 `ConstraintCheck` 作为**已物化结果**传入——`evaluate` 据此 deny 与 kernel 直接短路 deny 二者语义等价，取后者更早短路、省一次求值。
- **两阶段审计如何编排（intent 先于 execute 的时序锁）**：动词的副作用性决定审计编排形态。只读动词（observe/query）走"执行后单次"：先 `execute` 再 `scrub` 再单次 `record`，`record` 失败按 deny 返回（此时尚无对外暴露的已落库副作用，deny 诚实且安全）。有副作用动词（mutate/execute/manage/destroy）走**两阶段**：[7a] 先以同一 `request_id` 落 **intent** 事件，**intent 写不进则在 `execute` 之前 deny**（确未执行）；[7b]/[8] 执行后 [10] 落 **outcome** 事件，`outcome` 写失败时返回"已执行但审计降级"的可识别错误码、**绝不返回 deny**。设计取舍：这里刻意把"intent 必须先落盘成功"作为执行的**硬门**——宁可在确未执行时 deny（可重试且安全），也不容许"已执行却无痕"或"已执行却谎报 deny 诱导重试导致重复执行"（违公理六）。intent 与 outcome 由同一 `request_id` 关联，供事后对账"发起即有痕、结果可追溯"。
- **出口统一脱敏的单点如何成立**：内核把"离开内核的字节"收敛到**唯一一处** `sanitize` 调用——正常 `RawResponse`、`execute` 错误、各 `stage` 的 `DenyResponse`、审计事件正文，在交还外壳格式化**之前**都流经 `sanitize/`。实现上以"内核出口函数是唯一返回点"保证穷尽性：handler 不在内核内部各分支各自 format-and-return，而是各分支汇聚到出口、统一过 `scrub`/`scrub_stream` 再套 `{error:{code,message}}` 信封。外壳层语法 4xx 与 CatchPanic 响应体同样回灌这一出口，杜绝绕过路径（红线 7.2-3）。

### 3.3 shells/http + shells/mcp — 数据面外壳（服务端）

- **HTTP 外壳**：axum router 挂 `data.sock`；解析协议形态、不合法形态返回经同一 Sanitizer 的外壳层 4xx（常量安全文案）；构造 `NormalizedRequest` 提交内核。
- **MCP 外壳**：rmcp streamable-http 挂 `data.sock` 的 `/mcp`，并提供 stdio 桥的服务端侧；工具集是**固定动词工具**（不随授权动态增删，描述只含事实）：`postern_grants`、`postern_query/observe/mutate/execute/manage/destroy`、`postern_surface`。
- **`postern_surface`（CONS-20）**：返回该 Principal Scope 内**已授权能力面**——授权快照的投影，**禁止触达 `Adapter::discover`**、不触达任何底层资源。这与控制面接入侧的 `discover` 是两个术语，命名规范固化其边界、禁止互借。
- **ConnOrigin 仅 listener 构造**：`ConnOrigin`（`UnixPeer{uid,gid}` / `Tcp{remote}`）只能由本模块 listener 层构造，绝不采信请求自报字段（契约 `SEC_CONSTRUCTION_SITES`）。

**怎么做 — UDS 上挂 axum 与请求装箱**：数据面 router 是一个绑在 `data.sock`（`UnixListener`）上的 axum `Router`，经 tower 的 `serve_connection` 形态接受连接；**HTTP 与 MCP 两个 router 共挂同一 `data.sock`**（MCP 占 `/mcp` 子路径、HTTP 占其余动词端点），二者注入集合相同、出口同一 `submit`。装箱（[0]）是外壳唯一的语义动作：listener 接受连接的瞬间，由 listener 层经 `SO_PEERCRED`（`tokio UnixStream::peer_cred`）取 `(uid,gid)` 构造 `ConnOrigin::UnixPeer`——**这是 `ConnOrigin` 的唯一构造点**，请求体里任何自报来源字段一律不读（契约 `SEC_CONSTRUCTION_SITES`）。随后外壳把"出示物 + `ConnOrigin` + 资源代号 + 原始 Intent 负载"组装为 `NormalizedRequest` 提交 `submit`。设计取舍：装箱**只搬运不解释**——原始 Intent 原封传递给适配器的 `classify`，外壳绝不预解析 SQL/协议语义，确保 HTTP 与 MCP 经 [0] 后内核所见请求逐字段等价（公理七）。
- **不合法形态的 4xx**：协议语法层不合法（解析失败、缺字段）由外壳直接返回 4xx，但**仍过同一 Sanitizer** 输出常量安全文案——语法判断不是安全决策，不进管线，但其响应字节同样不许绕过出口。
- **MCP 工具面为何固定**：`rmcp` streamable-http 暴露的工具集是**编译期固定的动词工具**，不随某 Principal 的授权动态增删——动态增删会把"有哪些工具"变成授权事实的侧信道（泄露 Scope 边界）。工具描述只含协议事实，授权判定一律延后到管线内。
- **`postern_surface` 为何不碰 discover**：`postern_surface` 直接读**当前 `Arc<PolicySnapshot>` 的已授权投影**返回，路径上**没有 `Adapter::discover` 调用、不触底层资源**——它回答"我已被授权能看到什么"，是快照内存查表；控制面 `discover` 回答"这资源实际有什么能力"，要真实连库探测。命名规范固化二者边界、禁止互借，杜绝把接入侧探测误接到数据面成为发现即授权。

### 3.4 control — 控制面 API + 系统自动机

- **全部端点**（见 6.5）：principals / credentials（签发/吊销/轮换/可信域）/ roles / bindings / resources（含 `POST /v1/resources/{code}/discover` 触发接入侧探测）/ constraints / conditions / deny-notes / settings / grants/temp（elevate/revoke）/ mode / grants 视图 / audit 查询 / denials/summary / **approvals（审批挂起队列查询与裁决）** / export / import / verify / health / shutdown。
- **写入三联动**：每个写端点 = 一次事务 + 快照重建 + 审计事件；全部集合端点强制分页（`page_no/page_size`，缺省 20，钳制上限 200，`Page<T>` 信封，分页在 SQL/扫描层执行）；雪花 id 一律字符串序列化。
- **乐观锁端到端**：读端点统一返回 `version`，更新/删除端点必须携带期望 `version`，不匹配返回 `409 Conflict` 并写 `policy_change` 审计；系统协调写不走乐观锁。
- **系统自动机**：sweeper 回收、import/apply 协调以 `actor=system`（`created_by/updated_by=system`）走同一事务路径、同等审计。

**怎么做 — 控制面 router 与认证如何挂 UDS**：控制面是**独立于数据面**的第二个 axum `Router`，绑 `control.sock`（`0600`）；它与数据面 router **不共享注入集合**——这里注入的是 `PolicyRepo`（事务写）、机密面录入接口、`AuditSink`，而**没有**连接池/Sanitizer 出口（控制面不服务 Agent）。认证以 axum 中间件层形态前置在全部端点之上：先 `SO_PEERCRED` 取对端 uid 比对（即便同 uid 也要过），再校验控制面专用本地凭证——二者皆过才进 handler。设计取舍：`0600` 只对不同 uid 设防，故**必须叠加同 uid 也成立的控制面认证**，使"恰好同 uid"不等于"自动 admin"（硬前置条件②的运行时一半，另一半是 boot 的可连 uid 自检）。
- **写入三联动的编排次序**：每个写 handler 是一条固定序列——`PolicyRepo` 开事务 → 写（经 `base`，乐观锁/逻辑删除/级联由 store 层落地）→ COMMIT → **同一写锁临界区内**触发快照重建（`Arc` 原子替换）→ 落 `policy_change`/`credential_event`/`mode_change` 审计。设计取舍：快照重建与事务写**同临界区**，避免"已 COMMIT 但快照未换"的可见性裂缝；三者任一失败即整体不生效（事务不 COMMIT/不重建/返回错误并审计），不留半截状态（公理三：无双源）。带机密的写（资源接入、凭证签发轮换）先调机密面原子写 vault，机密写失败即整体回退、策略变更不提交。
- **乐观锁端到端如何贯通**：读端点在响应里回带 `version`；更新/删除 handler 从请求体/header 取**期望 `version`** 作为 UPDATE 的 `WHERE ... AND version = ?` 条件，影响行数为 0 即版本冲突 → 映射 `409 Conflict` 并落一条 `policy_change`（记录冲突）。期望 version 的唯一来源是调用方先前读取值，**不在 handler 内自读自比**（自读自比使乐观锁恒成立=失效）；系统协调写（sweeper 幂等谓词回收）无"读后写"竞态，**不走乐观锁**。
- **审批挂起如何编排（escalate 不立即折叠的分支）**：审批关闭时 `escalate ≡ deny`（取 fallback 恒 deny），管线内立即返回 `escalate_denied`、不进队列。审批开启时 `escalate` 进**内存挂起队列**（持久化必要恢复元数据），挂请求 `request_id` 与超时定时器；`GET /v1/approvals` 查询、`POST /v1/approvals/{id}/approve|deny` 裁决（裁决写审计）。两条 fail-closed 取舍：①超时即 deny（`on_timeout` 固定 `deny`，settings 写入与 import 校验都拒绝 `allow`），sweeper 兜底回收超时仍未裁决项并写审计；②**daemon 重启时一切挂起审批恒 deny**——不跨重启"复活"一个待批的危险操作，重启后内存队列为空即等同全部超时拒绝。

### 3.5 connpool — 连接管理

- **池键 `(ResourceCode, CredentialTier)`**：不同 tier 永不共享连接（账号隔离在连接粒度成立）。
- **建立流程**：收到 `Decision::Allow{tier}` 后 `acquire(resource, tier)`；向机密面一次性取 `(ResolvedTarget, ResourceCredential)` 不透明句柄并即时传入 `Transport::open`，调用边界外凭据引用即时释放。
- **重连/退避/健康/上限**：`persistent` 通路池化复用 + 健康检查 + 指数退避重连（基数 1s、上限 60s、带抖动）；非长连接即建即用即弃；每资源/全局并发上限，超限有界排队或 deny（fail-closed）。
- **回收与中断**：空闲回收（默认 10min）与优雅销毁（排空在途请求）；freeze/吊销时对相关 `(resource[, principal])` 在用连接**强制 abort/cancel**（取消底层查询、关闭隧道），而非仅优雅排空。
- **归池前会话净化（不变量）**：复用前强制重置会话态（如 PostgreSQL `DISCARD ALL`、重置 `search_path`、回滚未决事务、清临时表）；净化失败的连接销毁而不归池（fail-closed）。
- **连接审计（`connection_event`）**：通路建立/健康剔除/回收经 `AuditSink::record` 落 `connection_event`（字段为 resource、tier 名、transport 种类，不含真实地址/凭据）；该审计由连接管理层写入（传输层只如实上报健康事实、不写审计，见 6.4）。

**怎么做 — 池数据结构与获取/复用/健康/回收/中断的形态**：池的核心是一张以 `(ResourceCode, CredentialTier)` 为键的并发表（`DashMap` 类形态或 `Mutex`/`RwLock` 守护的 `HashMap`），每个键映射一个**池槽**，池槽内含：空闲 `Channel` 队列、在用计数、该键的并发上限、退避状态机、等待者队列。键里含 `tier` 即在结构层面实现"不同 tier 永不共享连接"——账号隔离落到数据结构而非运行时判断，从根上杜绝一条只读连接被升格执行写。
- **acquire 数据流**：收到 `Decision::Allow{tier}` → `acquire(resource, tier)` 定位池槽 → 命中空闲且健康的 `Channel` 即出借（复用，不重建）；无空闲且未达上限 → 向机密面**一次性**取 `(ResolvedTarget, ResourceCredential)` 不透明句柄、**即时**传入 `Transport::open` 得新 `Channel`，**句柄不出本次调用边界**（调用一返回即释放，不入池、不缓存——凭据零接触的运行点）；达上限 → 进有界等待队列或 `deny`（fail-closed）。`acquire` 返回的是**租约**形态（如 RAII guard），析构时把健康连接归还池槽、把损坏连接销毁，归还前强制会话净化。
- **复用与会话净化的次序**：复用一定发生在**净化成功之后**——租约归还（或下次借出前）跑 `DISCARD ALL`/重置 `search_path`/回滚未决事务/清临时表；净化是不变量不是优化，**净化失败的连接直接销毁不归池**（fail-closed），下个请求只会拿到干净连接。对存在会话副作用且无法可靠净化的形态，整类禁用复用（即建即用即弃）。
- **健康与退避状态机**：`persistent` 通路池化复用并周期健康检查，传输层**只上报通路死亡、绝不自行重建**——重建决策与节奏归本层。退避是每键一个状态机：死亡 → 指数退避（基数 1s、上限 60s、带抖动）择时重建，退避期内对该键的 `acquire` 走 deny 或有界等待而非风暴重连。非长连接 transport 不入池、不进退避，即建即用即弃。
- **上限与背压如何量化**：每键并发上限与全局上限均为**常量封顶**；超限的请求落入**有界**等待队列（容量上限 `Q`，与灌注量无关），队列触顶即背压（对上游施压）或 `deny, stage=connect`——绝不无界缓冲、绝不静默放行第三种结果。observe 类大流的缓冲占用峰值受 `Q` 钳制。
- **强制中断 vs 优雅回收的分流**：两条回收路径语义不同。空闲回收（默认 10min）与优雅销毁**排空在途请求**后关闭。freeze/吊销则对相关 `(resource[, principal])` 在用连接**强制 abort/cancel**——下达取消底层查询、关闭隧道的指令（物理执行归传输，决策归本层），不等其优雅跑完；这是 6.2"对已进入执行阶段的在飞操作"唯一真正的中断手段，优雅排空只是"不再接新"。
- **连接审计的写入点**：通路建立/健康剔除/回收/强制中断各落一条 `connection_event`，字段恰为 resource、tier 名、transport 种类——**绝不含真实地址/凭据**（地址/凭据从未进入本层可读形态，传输层错误跨边界前已脱敏为不含地址的错误码）。

### 3.6 sweeper — TTL 回收后台任务

事务性回收过期记录并写审计：`temp_grants`（写 `ended_at`/`end_reason=expired`）、`credentials.expires_at`、`mode_state.expires_at`、审批超时挂起项。回收后在同一写锁内重建快照。正确性不依赖 sweeper 时序——过期判定在求值时刻按墙钟二次校验（见 6.2），sweeper 只做"可见性回收 + 留痕打扫"。

**怎么做 — 定时任务的形态与"打扫"语义**：sweeper 是一个 tokio 周期任务（`tokio::time::interval` 节拍），每拍以 `actor=system` 走与人写**同一** `PolicyRepo` 事务路径：在一个事务里用谓词扫出 `expires_at < now` 的过期行、按表写终态（`temp_grants` 写 `ended_at` + `end_reason='expired'`、`credentials`/`mode_state` 按各自终态字段回收、审批超时项写裁决），COMMIT 后**与控制面写共用同一写锁临界区重建快照**、落 `policy_change`/`mode_change` 审计。设计取舍：sweeper 的写是**幂等谓词驱动**（"凡过期则回收"），无"读后写"竞态，故**不走乐观锁**；这也是它能与人写共路却不冲突的原因。
- **为何正确性不依赖 sweeper 时序**：过期的安全判定**不在** sweeper——`Evaluator`/`Authenticator` 在求值时刻就按墙钟 `expires_at >= now` 二次校验，过期格/凭证即刻不可见（见 6.2）。sweeper 只做两件**非安全承重**的事：把过期记录从库里"可见性回收"（让 `grants` 视图与重建后的快照不再带它）+ 留下 `expired` 审计痕迹。因此即便 sweeper 长时间未跑（或刚崩溃重启），安全语义不破——这是把"打扫"与"判定"解耦的关键取舍，避免把安全正确性押在后台任务的活性上。

### 3.7 sanitize — Sanitizer 执行

实现 `Sanitizer` trait：小响应/错误串/拒绝响应整体脱敏（`scrub`），流式大输出走滑动重叠窗口（`scrub_stream`，保留上块尾部 N 字节参与下块匹配，消除边界分块逃逸，N 取 ScrubSet 最长匹配模式上界，有界缓冲与背压）。脱敏材料两路：机密面签发的**系统级 ScrubSet 不透明句柄**（只能 match-and-erase）+ 来自 `grant_constraints.kind='mask_fields'` 的声明级 `MaskRule`。

**怎么做 — 句柄如何应用与滑窗状态**：本模块**只持有**机密面签发的 ScrubSet 不透明句柄，对它只调 match-and-erase——不可枚举、不可序列化、读不出内容（即便本模块代码亦然）。`scrub(payload, declared)` 对完整小 payload 一次性套两路材料：先过系统级 ScrubSet 句柄擦真实地址/凭据/私网段/连接串，再按 `declared` 的 `MaskRule` 擦声明字段。`scrub_stream(declared)` 是带状态的流式适配器：维护一个**重叠尾缓冲**（`carry`，长度 `N-1`，`N`=ScrubSet 最长匹配模式上界），每块输入与上块 `carry` 拼接后匹配擦除，再把本块尾部 `N-1` 字节存入 `carry` 留给下块——这样敏感串即便恰好跨 chunk 边界被切开也仍落在某次匹配窗口内，消除分块逃逸。缓冲**有界**，下游消费慢即对上游背压（与连接层有界排队同 fail-closed 取舍），不无界堆积。
- **脱敏无"放行"分支的取舍**：擦除是**单向**操作——匹配即擦、不匹配即原样过，不存在"匹配失败就直出明文"的语义分支。系统级 ScrubSet 本质是黑名单兜底，真正的机密不外泄保证来自上游匿名化（Agent 只见代号）与凭据零接触（凭据从不进响应路径），本模块是其上的尽力而为兜底，而非绝对识别保证。

### 3.8 实现要点与工程约束

本子小节汇总各内部模块共担的工程级实现约束（不重复 §3.1~§3.7 的逐模块"怎么做"，只给跨模块一致的承重事实）；与全局工程规范一致处一句话引用《详细设计文档》7.x，不整段重抄。

- **并发与线程模型（tokio 多线程）**：进程跑 tokio 多线程运行时；数据面每连接一个 task，管线内 `classify`/`acquire`/`execute` 等 IO 点 `await`，无阻塞独占。**唯一被串行化的是策略写**——控制面与 sweeper 经一把写互斥锁进入 `PolicyRepo`（rusqlite 同步 API），COMMIT 与快照重建在同一临界区完成，故写路径单线程、读路径（快照）无锁高并发。数据面**只读** `Arc<PolicySnapshot>`（`Arc::clone` 即取，微秒级、零锁），写侧 `Arc` 原子替换不阻塞读侧。连接池每键状态用细粒度锁/并发表守护，跨键无争用。`SO_PEERCRED` 走 `tokio UnixStream::peer_cred` 安全 API（无 `unsafe`）。
- **同步存储调用的异步边界（本 crate 承接 store 的同步签名）**：`postern-store` 只暴露同步 API、不替调用方决定并发模型（见 02-store §3.6），故"在 tokio 上下文里调同步 rusqlite/`JsonlAuditSink`"的承接责任落在本 crate——本 crate 是装配点也是那个异步调用方。承接形态：`control`/`sweeper`/`kernel` 对 `PolicyRepo` 事务、快照重建全量读、`AuditSink::record` 这类**会阻塞 OS 线程**的同步调用，一律置于 `spawn_blocking` 边界（或专用阻塞线程池）执行，**绝不在异步 worker 线程上直接同步阻塞**——否则一笔慢事务/慢 fsync 会拖住整个 runtime 的 reactor、间接拖慢全部数据面请求。设计取舍：写本就经一把写锁串行，把它放进阻塞线程池**不削弱串行性**（锁仍在），只把"同步阻塞"与"异步 reactor"两类执行体隔离开；审计 `record` 是高频点，其阻塞同样隔离在该边界外，避免 fsync 抖动反压到数据面 `await` 链上。
- **错误处理与传播**：每 crate 一个 thiserror 错误枚举；core 维护"错误变体→拒绝阶段"穷尽 match（新增变体不写映射即编译失败）。本 crate 的映射纪律：求值链任一 `Err`→对应 `stage` 的 deny（auth/classify/rbac/constraint/condition）；`acquire`/`Transport::open`/机密解析失败→`stage=connect` 的 deny（fail-closed，不降级、不改路）；`AuditSink::record` 失败按两阶段时序处置（只读 deny / intent 写不进执行前 deny / outcome 失败返"已执行但审计降级"码，绝不返 deny）。**kernel 路径禁吞错放行**（`.ok()`/`.unwrap_or(true)`/`.unwrap_or_default()`/`.unwrap_or_else(|_| true)`），由契约 `EVAL_NO_ERROR_SWALLOWING`（扫 `src/kernel/`）强制。panic 政策：数据面 router 挂 CatchPanic 层，任何 panic→脱敏 deny + `kind=anomaly` 审计；`anyhow` 仅出现在 `main`，全 crate `unsafe_code = forbid`、clippy `unwrap_used`/`expect_used`/`panic` 等 deny（详见 7.1/7.2）。
- **性能与资源边界**：求值读路径为快照内存查表，复杂度与库大小无关、微秒级、零库访问。连接池每键并发上限与全局上限为**常量封顶**，超限走容量上限 `Q` 的**有界**等待队列，触顶即背压或 `deny, stage=connect`——缓冲峰值 `≤ Q`、与灌注量无关，杜绝无界缓冲耗内存。退避有上界（基数 1s、上限 60s、带抖动），退避期不风暴重连。流式脱敏滑窗缓冲有界并对上游背压。长操作设可被控制面信号打断的检查点/超时，使在飞危险操作有可控中止路径。
- **测试策略**：kernel/管线逻辑用**内存 Fake 全插件驱动**——以纯内存 `PolicySnapshot` + Fake `Authenticator`/`Adapter`/`ConditionPredicate`/`Transport`/`AuditSink`/`CredentialProvider`/`Sanitizer` 注入，断言"给定输入→管线调用序与决策/审计恰为某可观察结果"（管线短路点、[4] 先于 evaluate、两阶段审计时序、各 `stage` deny、连接不可建即 deny、净化失败销毁、tier 不共享——对齐 §8 F-3/L-3/L-5~L-9/L-17 等，靠注入 Fake 失败触发 fail-closed 分支再观察行为）。控制面 API 走**集成测试**：起一个挂临时 `control.sock` 的 router，断言端点全集可达、写端点三联动齐发、集合分页钳制、乐观锁 `409`、审批裁决与重启恒 deny。boot 的 fail-closed 分支靠**前置条件可注入**测到——分别注入"开库/迁移/首快照/保险箱解锁失败""socket 创建早于装配完成""`data.sock` 可连 uid 含自身 uid"，断言进程非零退出、`data.sock` 未创建、数据面未开放（对齐 §8 F-1/F-2/L-1，与 02-store §3.6 的启动 fail-closed 注入同源）。connpool 的真实净化/退避/中断可对真实资源（容器）做端到端验证，但管线编排判定不依赖真实库。`postern verify` 九项与场景 02~07 以本 crate 为运行载体。
- **可观测性（tracing 字段白名单，机密红线）**：运行日志（tracing）与审计事件流是两套东西，二者都不得泄漏机密。tracing 字段限白名单：`request_id`（与审计事件 id 同源、便于对账）、`principal`、`resource`（恒为代号）、`capability`、`stage`、`decision`、`duration_ms`；逐请求细节只进审计，tracing 限生命周期/连接/异常事件。**红线**：`Intent` 原文（SQL 可含业务敏感数据）、`PresentedCredential`、vault payload、`secret_ref` 解引用结果、真实地址一律禁止入日志——这些类型在类型层即 `Debug=REDACTED`/无 `Display`/不可 `Serialize`，tracing 字段无法直接记录它们。本 crate 产出的事件：数据面请求事件（intent/outcome，带 `stage`/`reason`）、`connection_event`（resource/tier 名/transport 种类，无地址/凭据）、控制面 `policy_change`/`credential_event`/`mode_change`/`lifecycle`、panic 的 `kind=anomaly`；一切审计正文写入前必经同一 Sanitizer（详见 7.5/7.2-3）。

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
