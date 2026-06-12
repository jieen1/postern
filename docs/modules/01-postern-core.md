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

**建模要点（怎么做 + 为什么这样建类型）**：词汇表的实现是"把安全语义编码进类型形状，让违规在类型层不可表达"——而非仅声明一组 struct。几处非显然的建类型取舍：

- **`Capability` 是封闭的正交六动词枚举、且无 `Admin` 变体**：六动词（`Observe/Query/Mutate/Execute/Manage/Destroy`）按"读/写/管/毁"的正交语义切分，授权格是 `Resource × Capability` 的笛卡尔判定空间，使"该 Principal 对该资源能做哪类动作"由格子存在性单点判定（§3.3 `[3]`）。**`Admin` 不进枚举**是把"admin 不可授予"（公理三）落到类型层：枚举里没有这个变体，"授予 admin"在 Rust 里根本写不出来（契约 `SEC_ADMIN_NOT_GRANTABLE`），而非靠运行期检查拦截。
- **`ConnOrigin` 恰两态、且只取最小可信字段**：`UnixPeer` 只取 `{uid,gid}`、`Tcp` 只取 `remote`——**刻意不纳入 pid/exe 路径**，因 PID 可复用、`/proc` 读存在 TOCTOU 伪冒面，是可伪造特征；把不可信字段挡在类型之外，比"取进来再判断要不要信"更彻底（构造权仅属外壳 listener，§5.1）。
- **机密族类型在词汇表里只有不透明声明**：`PresentedCredential`/`ResolvedTarget`/`ResourceCredential` 在 core 仅是声明（不可 Clone/Serialize、`Debug=REDACTED`、无 `Display`、core 内无构造点），词汇表给出"它们存在且如何被引用"，但实体的产生在 secrets/外壳（公理四的编译期表达，详见 §5.2/§7）。
- **`Decision` 是三值而非布尔**：`Allow{grant,tier}`/`Deny`/`Escalate{fallback}` 把"放行还需带哪个授权格与凭据等级""拒绝带结构化响应""升级折叠为 fallback"三种结局编码进枚举，使下游不可能"只拿到 true/false 而丢失放行所需上下文或拒绝事实"。
- **雪花 id 系列类型 JSON 恒序列化为字符串**：`PrincipalId` 等承载 64bit 雪花 id 的类型，序列化约定为字符串而非数字（避开 JS 53bit 精度丢失，详见 §3.5），这是词汇表对全系统 id 表达的统一约束、非各端各自决定。

### 3.2 全部插件 trait 的定义

`Authenticator`、`Adapter`、`Transport`、`CredentialProvider`、`ConditionPredicate`、`AuditSink`、`Sanitizer`、`PolicyView`——**定义在 core，实现在各面 crate**（接口隔离点；与详细设计第四部分及附录 C 一一对应）。

**为什么 trait 全部定义在 core（依赖反转，怎么做）**：这是 core 得以"零 IO、不依赖工作区任何 crate"却仍被全系统消费的**关键结构手段**——core 只声明"求值/执行需要哪些能力的形状"（trait 签名），把"如何用 IO 实现这些能力"全部下沉到各面 crate；`Evaluator` 与各运行期组件持有的是 `&dyn Trait`（或泛型约束），而非任何具体 IO 类型。由此**依赖边方向恒为"各面 crate → core"**（它们 `impl core::Trait`），绝无"core → IO crate"的反向边（契约 `ARCH_FORBIDDEN_EDGES`）。两条非显然的设计取舍：

- **trait 上的 `Send + Sync` 是给实现方的约束、不是 core 的负担**：core 不 `spawn`、不 `await`，但因实现落在各 async 域、且要跨 tokio 任务共享，故 trait 在 core 处即标 `Send + Sync`（凭据/连接相关的 trait 还带 `#[async_trait]`）——这是"由 core 统一规定接口契约、各域照此实现"，把并发安全要求前移到接口定义处。
- **机密相关 trait 的入参/出参用 core 的不透明声明类型**（`Transport::open` 收 `ResolvedTarget`/`ResourceCredential`、`CredentialProvider` 产 `ResourceCredential`）：core 只声明这些类型不可 Clone/Serialize、`Debug=REDACTED`，**不持有其构造权**（构造在 secrets，契约 `SEC_CONSTRUCTION_SITES`）。这使"trait 定义在 core"与"机密零接触"两个目标在类型层同时成立——接口形状在 core，机密实体永不流经 core。

这一反转的直接收益是 §3.6 测试策略所述的"契约测试纯内存驱动"：任一 trait 都能被一个内存 Fake 实现并注入，求值/映射逻辑无需任何真实 IO 即可被完整驱动。

### 3.3 纯函数求值

`Evaluator::evaluate(req, ci, constraint_check, policy, now)` —— 编排求值步骤 `[1][3][5][6]`，产出 `(Decision, EvalTrace)`；allow 时依快照 tier 声明完成动词→凭据等级选择；DenyResponse 的事实组装（reason / your_grants / request_hint / operator_note）全部取自快照。

**内部编排时序（怎么做）**：`evaluate` 是一条**短路串行管线**——按 `[1]→[3]→[5]→[6]`（外加 allow 后的 tier 选择）顺序执行，**任一步判定拒绝即就地短路**，组装 `EvalTrace` 并以 `Deny` 返回，绝不继续往下跑。步骤之间不并行（条件谓词逐一求值、遇假即停，无需全跑），因为求值是 fail-closed 的"找到一个拒绝理由即足够"，并行既无收益又会模糊"在哪一步拒"的轨迹归因。各步落点：

- **`[1]` 认证**：从注入 `Evaluator` 的 `Authenticator` 注册表按 `presented` 的 kind 选认证器，调 `authenticate(presented, origin, creds, now)` 得 `PrincipalId`；`now` 由 `evaluate` 透传，使凭证 `expires_at`/`revoked_at`/可信域时效在求值时刻按墙钟二次校验（不依赖 sweeper 时序）。`Err` → 短路 deny，`stage=auth`。
- **`[3]` RBAC 展开查表**：以 `[1]` 得到的 `Principal` 在快照中展开 `(Role × Scope)`，查 `(req.resource, ci.capability)` 授权格是否存在。**`[2]` classify 的产物 `ci` 是入参**（kernel 先行物化），故 `evaluate` 直接消费 `ci.capability`/`ci.objects`，本步不调适配器。无命中格 → 短路 deny（公理一），`stage=rbac`。
- **`[4]` 细则**：不在 `evaluate` 内执行——`ConstraintCheck` 已由 kernel 先跑 `Adapter::check_constraint` 物化为布尔事实入参（CONS-8）。`evaluate` 仅读 `constraint.passed`，`false` → 短路 deny，`stage=constraint`。这使 `evaluate` 零 IO、不持有 Adapter，契约测试可纯内存构造该入参。
- **`[5]` 条件谓词**：取命中格附加的条件谓词清单，从 `ConditionPredicate` 注册表逐一 `eval(ctx, spec)`；`ctx` 由 `req`/`ci`/`now`/快照事实组装（含 mode、TTL、限流上下文等），全部来自入参与快照，无隐式来源。任一谓词返回 `false` 或 `Err`（无法判定）→ 视为不满足 → 短路 deny（公理二），`stage=condition`。
- **`[6]` 动作分流 + tier 选择**：读命中格的动作标注。`allow` → 进入 **tier 选择**：在快照对该资源的 tier 声明中查"承载本次动词（`ci.capability`）的等级"，命中 → `Allow{grant, tier}`；**无任何 tier 承载该动词 → deny（不退默认 tier、不 panic）**，`stage=tier`。`escalate` → 直接折叠：审批关闭即取该格 fallback（恒 deny），不挂起、不在 core 内引入任何等待状态（审批挂起属控制面运行期，core 只表达"折叠为 deny"的纯语义）。

**EvalTrace 如何累积**：`EvalTrace` 随管线推进**逐步追加**记录——每进入一步即登记"到达该步"，判定时登记"在该步、因何判定"（命中/未命中、谓词名与结论、tier 选择结果）。短路发生时轨迹截止于当前步，其最后一条 `stage` 即拒绝阶段，直接喂给审计 `stage` 字段与 `DenyResponse.reason` 组装。因 `evaluate` 全程确定性（输入相同→轨迹逐字相同），`EvalTrace` 是审计可对账（同一 `policy_rev` 下同一请求复算得同一轨迹）的载体。allow 路径的轨迹同样完整（记录"逐步皆通过 + 选定 tier"），供放行审计与解释。

**DenyResponse 组装（为什么只取快照）**：`reason`/`your_grants`/`request_hint`/`operator_note` 全部从快照与轨迹机械取值、不编造（公理六）。`your_grants` 仅枚举该 `Principal` 自身 Scope 内授权的资源代号（受 `DENY_RESPONSE_SCOPE_BOUNDED` 约束）——这使"Scope 外但存在的资源"与"根本不存在的资源"两次拒绝**逐字节相同、不可区分**（防拓扑探测）：关键设计取舍是拒绝响应**绝不**回答"该资源是否存在"，只回答"你的授权世界里有什么"。`request_hint` 由策略对可授予能力机械生成 `postern elevate` 命令、对不可授予能力恒为 `null`；`operator_note` 仅当资源所有者预写才出现（缺省不序列化）。

### 3.4 错误 → 拒绝阶段映射

各域错误枚举（`AuthError` / `ClassifyError` / `ConstraintError` / `PredicateError` / `TransportError` / `CredentialError` / `ExecError` / `DiscoverError` / `AuditError` …）及其到拒绝阶段（`stage`）的穷尽映射，供审计 `stage` 字段与拒绝响应组装使用。

**穷尽 match 的组织方式（怎么做）**：映射以**逐变体显式分支**的 `match` 表达——每个错误枚举的每个变体对应一个拒绝 `stage`，**禁用 `_ =>` 通配兜底**。这是关键设计取舍：通配会让"新增一个错误变体却忘了归类"无声落进某个默认阶段，掩盖归因错误；去掉通配后，给某枚举加新变体而不补映射**直接编译失败**——把"映射完备性"从测试义务变成编译期义务（与详细设计 7.1 错误模型一致）。`stage` 取一个封闭的阶段枚举（对齐求值管线步骤命名：`auth`/`classify`/`rbac`/`constraint`/`condition`/`tier`/`transport`/`exec`/`audit`/`discover` 等），与 `EvalTrace` 截止步语义一一对应，使"轨迹截止步"与"错误归类"两条来源对同一次拒绝得到同一 `stage`。

**为什么映射归 core**：错误枚举本身各域定义、实现各域，但"错误→阶段"的语义裁决是共享词汇表的一部分（拒绝阶段是全系统统一的审计维度），故穷尽映射的权威定义落 core，各域只产出错误、不各自决定阶段名——避免阶段命名在各 crate 漂移、审计 `stage` 字段失去跨域可对账性。映射是纯函数（错误值入、`stage` 出），无 IO、无副作用，与 `evaluate` 同属可纯内存驱动的判定逻辑。

### 3.5 统一 ID 与分页

`IdGen`（雪花规格，时钟回拨拒绝生成）、`PageQuery`（含 `clamp` 上限钳制）、`Page<T>`（统一分页信封）。

**IdGen 并发序列号生成（怎么做）**：位布局为 41 bit 毫秒时间戳（纪元 `2026-01-01T00:00:00Z`）+ 10 bit 节点号（config，默认 0）+ 12 bit 序列号。`IdGen` 持有"上次出号毫秒 + 该毫秒内序列计数"两项可变状态，对并发请求**串行化**取号：

- **关键设计取舍——状态需互斥**：`IdGen` 是 core 内**唯一**持有可变状态的设施（与 `evaluate` 的无状态形成对照），多线程共用一个 `IdGen` 取号时该状态须互斥更新（短临界区的轻量串行化即可：读时钟、比较毫秒、推进序列、组装 id），保证同一毫秒内序列号严格递增、不漏不重。
- **同毫秒序列推进与溢出**：同一毫秒内每出一号序列 +1；序列在 12 bit 内（单毫秒上限 4096 个）。**用满即等待下一毫秒**（自旋至墙钟跨入下一毫秒再从序列 0 重开），绝不在同毫秒内回绕复用——回绕会产出重复 id，违反"唯一来源"。跨毫秒时序列归零。
- **时钟回拨处理思路（为什么拒绝而非容忍）**：取号读到的当前毫秒 **< 上次出号毫秒**即判定时钟回拨，**拒绝生成并返回错误**（fail-closed，公理二），绝不"取上次毫秒继续发"或"等回拨追平"。理由：回拨期间若沿用旧毫秒续发，会与回拨前已发 id 落在同一(毫秒,序列)空间而碰撞；而 id 是全工作区表主键与审计事件 id 的唯一来源，一个重复 id 即破坏主键唯一性与审计可对账性，代价远高于"短暂拒绝出号"。把回拨上抛由调用方（store/daemon）按 fail-closed 处置，core 不擅自补偿时钟。
- **序列化约定**：雪花 id 为 64 bit，JSON 序列化**恒为字符串**（避开 JS 53 bit 精度丢失），反序列化亦按字符串解析。

**PageQuery clamp 边界（怎么做）**：`clamp` 是把任意外来分页参数收敛进合法区间的**纯函数钳制**，不报错（取舍：超限钳到边界而非拒绝，使集合查询对"页大小填超"这类常见输入鲁棒、不把可恢复输入升级为失败）：

- `page_no`：从 1 起，`< 1` 钳到 `1`（杜绝 0 或负页号导致后端 `OFFSET` 计算异常）。
- `page_size`：钳进 `[1, MAX_SIZE]`，`MAX_SIZE = 200`、`DEFAULT_SIZE = 20`；`> 200` 钳到 `200`（封顶防无界查询，契约 `DB_PAGINATION_MANDATORY` 的语义前提），`< 1` 钳到合法下界。
- **为什么钳制点在 core**：分页是全部集合查询的唯一形态，上限须有单一权威值；core 提供 `clamp` 后，store 后端在拼 `LIMIT ? OFFSET ?` 前统一过一次 `clamp`，使"页大小封顶"在全工作区只有一处定义、不被各调用方各自放宽。

### 3.6 实现要点与工程约束

本小节是上述功能落地时须共同遵守的横切约束。本 crate 的定位决定其工程画像与各 IO 域迥异：**纯类型 + 纯函数、零 IO、零工作区依赖**，故其约束以"确定性、可纯内存驱动、无任何副作用"为主轴；与全局工程规范（详细设计 7.x）重叠处一句话引用、不整段重抄。

**并发 / 线程模型**：本 crate **纯同步、无 async、不触碰 tokio**——`evaluate` 与"错误→阶段"映射是无状态纯函数，可被任意线程并发调用、无需任何同步（无共享可变状态即无数据竞争）。`Evaluator` 的注册表（`Authenticator` / `ConditionPredicate`）在装配后只读，求值期不改。唯一持有可变状态的设施是 `IdGen`（出号计数 + 上次毫秒），其取号临界区须互斥串行化（见 §3.5），是 core 内唯一需要内部同步的点。trait 上的 `Send + Sync` 是给**实现方**的契约（实现落在各 async 域），core 本身不产生任务边界、不 `spawn`、不 `await`。

**错误处理与传播**：错误模型遵循详细设计 7.1——每域一个 thiserror 枚举，core 维护"错误变体→拒绝阶段"的**穷尽 `match`、禁 `_ =>` 通配**（见 §3.4，新增变体不补映射即编译失败）。求值路径 fail-closed 是**被机器守护的不变量**而非仅代码规范：`core::eval` 路径禁 `.ok()` / `.unwrap_or(true)` / `.unwrap_or_default()` / `.unwrap_or_else(|_| true)` 等吞错放行写法（契约 `EVAL_NO_ERROR_SWALLOWING` + 反例自检）；一切 `Err`/无法判定**一律解析为 `Deny`**（公理二），由 `evaluate` 短路并在 `EvalTrace` 标注阶段。**panic 政策**：core 不依赖外壳的 `CatchPanic` 兜底，而是从源头消除 panic——lint 在 workspace 级 deny `unwrap_used`/`expect_used`/`panic`/`indexing_slicing`/`arithmetic_side_effects`（后者直接覆盖 `IdGen` 时间戳/序列运算与 TTL 比较的间接 panic 源），`unsafe_code = forbid`（机密类型的类型层保证以内存安全为前提）。`IdGen` 时钟回拨是 core 内唯一显式的 fail-closed 出错点（拒绝出号而非 panic、亦非续发）。

**性能 / 资源边界**：`evaluate` 是关键路径，须**微秒级、零库访问**——它只在已物化入参（`ci`/`ConstraintCheck`）与内存快照上查表，不发起任何 IO、不阻塞。复杂度随命中格的条件谓词数线性、且遇假即短路；RBAC 展开与 tier 选择是快照内查表（快照已由 store 预先物化展开，core 不在求值期做继承展开计算）。core 自身无连接、无缓冲、无超时概念（这些属连接管理/传输/脱敏的运行期约束，见边界表）；唯一的有界量是 `IdGen` 单毫秒 4096 序列上限与 `PageQuery::MAX_SIZE=200` 的分页封顶。

**测试策略**：core 零 IO 是测试的直接收益——**全部以纯内存 Fake 驱动，无需任何真实资源（无库、无容器、无网络）**。求值器测试以纯内存 `PolicySnapshot` 构造 + 内存假 `Authenticator`/`ConditionPredicate` 实现驱动，按 §8 的 F/L 用例方向覆盖：命中放行选 tier、未命中默认拒、各 `Err`/`false` 入参分别落到正确 `stage`、escalate 折叠、确定性（同输入多次/跨进程逐字相同）、拒绝响应 Scope 受限（两类不存在/越界资源不可区分）。`IdGen` 以可注入的 `now` 源测同毫秒递增唯一、序列溢出进位、时钟回拨拒绝出号；`PageQuery::clamp` 测三类边界（页号 `<1`、页大小 `<1` 与 `>200`）。运行期行为不变量（escalate 折叠、不可归类即拒、过期/吊销凭证拒、临时授权过期即不可见、无 tier 匹配即拒、拒绝响应 Scope 受限）以纯内存 `Evaluator` 数据驱动的 `(assert)` 值契约表达（详细设计 7.3-b），独立于 24 条静态结构契约。

**可观测性**：core **无任何可观测性副作用——不记日志、不发指标、不写审计**（零 IO 的直接推论）。它的"可观测产物"是**返回值里的 `EvalTrace`**：求值轨迹（到达哪步、因何判定、`stage`、选定 tier）作为数据交回 kernel，由 kernel 决定落审计事件与运行日志（字段白名单见详细设计 7.5）。机密红线在 core 由**类型层**而非运行期纪律保证：机密族类型（`PresentedCredential` / `ResolvedTarget` / `ResourceCredential`）不可 Clone/Serialize、`Debug=REDACTED`、无 `Display`，在类型层即无法被 tracing 字段或序列化直接记录；core 内不出现其构造点。因此"不记凭据、不记真实地址"在本域不是约定而是编译期事实，`EvalTrace`/`DenyResponse` 只承载代号与策略事实、绝无机密。

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

---

## 8. 验收标准

> 本节是 `postern-core` 的**验收基准**：拿这份清单可逐条判定开发实现的"功能写全没、逻辑对不对"。每条 = **要求 + 通过判定**，通过判定对当前代码只有"通过/不通过"一个答案，无歧义、可复现；判定方式按条目而定（行为观察 / 接口存在 / Stele 契约绿红 / 结构检查），不强求都是单元测试。
>
> 说明：第 7 节引用的 `DENY_RESPONSE_SCOPE_BOUNDED` 在现行 24 条 Stele 契约中暂无对应规则，其相关条目（L-5）以行为观察判定。

### 一、功能完整性（判断：该有的功能都写了吗、行为对吗）

| 编号 | 要求（必须实现） | 通过判定（满足即过，否则不过） |
|---|---|---|
| F-1 领域词汇表 | 定义这些类型且字段/变体与 §5.1 一致：`Capability`、`PrincipalId`/`ResourceCode`/`CredentialTier`、`NormalizedRequest`/`Intent`/`ClassifiedIntent`/`ConnOrigin`、`MatchedGrant`、`Decision`/`DenyResponse`/`EvalTrace` | 上述类型全部存在；`Capability` 恰六变体（`Observe/Query/Mutate/Execute/Manage/Destroy`）无 `Admin`；`ConnOrigin` 恰两态（`UnixPeer{uid,gid}`/`Tcp{remote}`）；`DenyResponse` 字段集恰为 `decision/denied/reason/your_grants/request_hint/operator_note` |
| F-2 八个插件接口 | 定义 8 个 trait：`Authenticator`/`Adapter`/`Transport`/`CredentialProvider`/`ConditionPredicate`/`AuditSink`/`Sanitizer`/`PolicyView`（§5.2） | 8 个 trait 签名全部存在、与 §5.2 一致；每个都能被一个内存假实现实现并编译通过 |
| F-3 求值器 | 提供 `Evaluator::evaluate(req, ci, constraint, policy, now) -> (Decision, EvalTrace)`（§5.3） | 该函数存在、签名一致；`now` 为入参、`&self` 不可变（无可变运行时状态） |
| F-4 求值编排 | `evaluate` 依次完成 认证[1]→RBAC[3]→条件[5]→分流[6]，allow 时选 tier | 给定"命中授权格、细则过、条件满足"的快照与请求 → 返回 `Allow{grant, tier}`，`grant` 为正确命中的格、`tier` 为该动词在快照声明的等级 |
| F-5 错误→阶段映射 | 每个域错误枚举映射到唯一拒绝 `stage`（§3.4） | 列出的每个错误枚举（`AuthError`/`ClassifyError`/`ConstraintError`/`PredicateError`/`TransportError`/`CredentialError`/`ExecError`/`DiscoverError`/`AuditError`）都有对应 `stage`，无遗漏、无 `_ =>` 通配兜底（缺一变体则编译失败） |
| F-6 雪花 IdGen | 提供全工作区唯一 id 生成（§5.4/§7） | 生成的 id 解析为 41bit 毫秒（纪元 `2026-01-01T00:00:00Z`）+ 10bit 节点 + 12bit 序列；JSON 序列化为字符串；时钟回拨时拒绝生成 |
| F-7 统一分页 | 提供 `PageQuery`/`Page<T>` 与上限钳制（§5.4） | `DEFAULT_SIZE=20`、`MAX_SIZE=200` 存在；`clamp` 把 `page_size` 超限钳到 200、`page_no` 钳到 ≥1 |

### 二、逻辑正确性（判断：关键逻辑、边界、失败处理对不对）

| 编号 | 要求（行为必须正确） | 通过判定 |
|---|---|---|
| L-1 默认拒绝 | 未命中授权格的请求一律拒（公理一） | 快照中无该 `(资源, 动词)` 格 → 返回 `Deny`（非放行） |
| L-2 tier 选择 | allow 后按动词选承载它的等级 | 有匹配 tier → `Allow{tier=该等级}`；无任何 tier 承载该动词 → `Deny`（**不 panic、不退默认 tier**） |
| L-3 一切 Err 即拒（fail-closed 核心，公理二） | 认证/条件/细则任一报错或无法判定 → 拒 | 注入假实现使 `Authenticator::authenticate` 返回 `Err`、`ConditionPredicate::eval` 返回 `Err`/无法判定、或传入 `ConstraintCheck{passed:false}` → 每种都返回 `Deny`，且 `EvalTrace.stage` 指向正确环节（auth/condition/constraint） |
| L-4 escalate 折叠 | 审批关闭时 escalate 立即变拒（5.3） | 命中 escalate 格且审批关 → 返回 `Deny`（取 fallback），不挂起 |
| L-5 拒绝只说自身世界 | 拒绝响应不泄露其他资源/存在性（公理六、防拓扑探测） | `your_grants` 只含该 Principal 自身授权的资源代号；分别请求"Scope 外但存在的资源"与"根本不存在的资源"，两次 `DenyResponse` 完全相同（不可区分） |
| L-6 拒绝只含事实 | 不编造话术（公理六） | `reason` 引用策略事实；`request_hint` 由策略机械生成、对不可授予的能力为 `null`；`operator_note` 仅当资源所有者预写才出现，缺省时该字段不序列化 |
| L-7 确定性 | 相同输入相同决策（审计可对账前提） | 同一 `(req, ci, constraint, policy, now)` 多次/跨进程调用 → `(Decision, EvalTrace)` 完全相同；求值路径不读系统时钟、不用随机 |
| L-8 id 不重复 | 并发/同毫秒不产重复 id | 同一毫秒连发至序列上限内全部唯一且递增；溢出则进位或拒绝，绝不重复 |

### 三、边界与不变量（机器强制，绿/红即答案）

| 编号 | 要求 | 通过判定（机器） |
|---|---|---|
| B-1 零 IO、零工作区依赖 | core 不依赖任何业务 crate、不碰 IO | 契约 `ARCH_FORBIDDEN_EDGES` 绿；`cargo tree -p postern-core -e normal` 无 store/secrets/transports/adapters/daemon/rusqlite/网络/文件 IO crate |
| B-2 无 Admin | admin 在类型层不可表达 | 契约 `SEC_ADMIN_NOT_GRANTABLE` + 反例自检 `SEC_ADMIN_NOT_GRANTABLE_TEETH` 均绿 |
| B-3 机密不可构造/复制 | core 内不构造机密类型、机密不可 Clone/Serialize | 契约 `SEC_CONSTRUCTION_SITES` + `SEC_SECRET_TYPE_DISCIPLINE`（各 + `_TEETH`）绿 |
| B-4 求值禁吞错 | 求值路径无 `.ok()`/`.unwrap_or(true)`/`.unwrap_or_default()`/`.unwrap_or_else(\|_\|true)` | 契约 `EVAL_NO_ERROR_SWALLOWING` + 反例自检 `EVAL_NO_ERROR_SWALLOWING_TEETH` 均绿 |
| B-5 id/分页统一 | 唯一 id 来源、分页强制 | 契约 `DB_UNIFIED_ID_GENERATOR` + `DB_PAGINATION_MANDATORY`（各 + `_TEETH`）绿 |
| B-6 lint 红线 | 无 unwrap/expect/panic/unsafe | `cargo clippy -p postern-core --all-features -- -D warnings` 退出码 0 |

### 通过定义（DoD）

`postern-core` **算完成** ⟺ 一、二、三三组**每一条都通过**。任一条不过 = 不通过，必须修。F/L 类靠"给定输入看行为是否符合通过判定"，B 类靠"跑契约/clippy 看绿红"。
