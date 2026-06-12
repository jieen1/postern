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

**怎么做（遍历策略与对象提取）**：

- **分级定档而非逐节点判定**：把"动词危险度"建模为一条全序——`Destroy(Delete/Drop/Truncate) > Mutate(Insert/Update/Merge) > Observe(Show/Explain非ANALYZE) > Query(纯只读)`。`classify` 对一条语句的归类 = 遍历整棵语句树所见全部写/观测节点取**最高危者**（一次自顶向下的 AST 走查累积一个"当前最高危档"，遇到更高危节点即提升、永不下调）。这把"是否降级"从需要逐分支推理的判断，收敛为"取最大值"这一不可被外壳包裹绕过的单调运算——这是 L-1/L-6 不降级在算法层的根因。
- **遍历必须穿透只读外壳**：sqlparser 把 CTE（`Query.with`）、子查询（`SetExpr`/`TableFactor::Derived` 内嵌 `Query`）、`INSERT ... SELECT` 的源、`SELECT ... INTO` 的目标都建模为语句树的子节点；遍历**递归进入所有这些子树**，不在顶层 `Statement` 变体上止步。`WITH x AS (DELETE ... RETURNING *) SELECT ...` 顶层是 `Query`，但子树里的 `Statement::Delete` 被走查命中，最高危档被提升到 `Destroy`。判据是"语句树内是否**出现过**写节点"，与该写节点处于哪一层无关。
- **白名单是"语句形态枚举"而非"危险动词黑名单"**：归类的入口是对顶层 `Statement` 变体做显式 `match`——只有落在 `Query`/`Insert`/`Update`/`Delete`/`Merge`/`Truncate`/`Drop`/`Show`/`Explain` 这组**已枚举形态**的语句才继续走查定档；`Set`/`DO`(`DECLARE`-less 匿名块)/`Copy`/`Call`/`Prepare`/`Deallocate` 等一切其余变体在 `match` 上即落入 `Err` 分支。**为什么用白名单而非黑名单**：黑名单要求穷举所有危险形态，漏一个即 fail-open；白名单只放行能可靠归档的少数形态，新语法默认落 `Err`（公理二），是 fail-closed 的唯一正确方向。
- **对象提取与定档同遍历完成**：同一趟 AST 走查在累积最高危档的同时，收集所触达的 `schema.table`（`ObjectName` 规范化为 `ObjectRef`）与列名，去重后随 `ClassifiedIntent.objects` 返回。**为何同遍历**：对象集既供 §3.2 `table_allow`/`column_mask` 判定，又供内核审计消费（见 §3.x 可观测性），二者须看到与定档**完全一致**的对象视图，分两趟遍历会引入"判定看到的表与审计记录的表不一致"的漏洞。对象未能可靠提取（如动态拼接的对象名、`information_schema` 反射式引用）按不可靠归类处理，宁可 `Err`。

定档原则与禁降级语义（与 4.4 严格一致）：

- **按整棵语句树内最高危写节点定档，而非顶层语句类型**。危险动词在整棵树范围内可见，CTE 或子查询里的 `DELETE`/`DROP`/`TRUNCATE` 一律提升为 `Destroy`，**绝不因被只读外壳包裹而降级**（如 `WITH x AS (DELETE ...) SELECT ...` 仍判 `Destroy`，绝不降为 `Mutate` 或 `Query`）。
- **`SET` 一律拒绝、不设白名单逃生口**：`SET` 改变会话语义（search_path、role、超时等），不属只读观测；放行的 `SET` 无归属动词将使 RBAC 步骤[3]无格可判（违 fail-closed）。会话语义的调整（如设定只读会话）由适配器在**建立连接时**统一施加，绝不接受 Agent 的 `SET` 请求。
- **`EXPLAIN` 仅允许不带 `ANALYZE` 的形态归为 `Observe`**：`EXPLAIN ANALYZE` 会真实执行被解释语句，按其内部最高危写节点定档（含写节点即非 `Observe`）。
- **白名单之外宁可误拒**（公理二）：归类是显式枚举的语句形态白名单，未知/歧义/多语句一律 `deny`；归类正确性的强制兜底是凭据分级（`Query` 恒走只读账号，引擎层拒绝一切写入，见 §3.3）。

**docker_logs（容器日志，只读取数）**：`Intent` 为容器日志请求（容器选择 + 取数范围），恒归类为 `Observe`；其取数形态与远端探针/直连差异见 §3.4 与详设 6.12。

- **怎么做**：docker_logs 的 `Intent` 负载是一个**封闭枚举的取数请求**（容器选择符 + `since`/`tail`/`follow` 等只读取数参数），其形态本身**不含任何写表达**——没有"执行命令""重启容器"这类变体可被构造。因此 `classify` 不需要语法树遍历，只做"负载结构合法即归 `Observe`、否则 `Err`"的形态校验；`objects` 取容器选择符规范化后的 `container:<名>`。**为什么恒 Observe 不靠运行期判别**：只读性下沉到了 `Intent` schema 层（无写变体可表达）与远端只读端点/探针（见 §3.4），而非靠 `classify` 在运行期识别一条请求是否危险——这与 SQL 的"白名单形态"同源，都是把安全性建立在"危险无从表达"而非"危险被识别"上。

**http（HTTP API）**：按声明的动词工具与路径将请求归类为相应 `Capability`；`engine_enforced=false`，归类 + 细则是唯一防线。

- **怎么做**：HTTP 没有 SQL 那样的协议级语义可解析，`classify` 依据**该资源声明的动词工具映射**——即接入时为该 HTTP 资源声明的 `(MCP 动词工具 → 方法×路径形态)` 表，把进来的 `(method, path)` 反查到声明的 `Capability`。命中声明形态归相应动词，未落任何声明形态 → `Err`（白名单，未声明即不可归类）。**为什么 http 的归类必须更保守**：`engine_enforced=false` 意味着没有引擎账号兜底，一条被误归低危的写请求**不会**在下游被第二道防线拦下；故 http 的归类档位完全由声明决定、不做任何启发式推断（如"GET 即只读"这类假设在反代/RPC-over-GET 场景会 fail-open，禁止采用），`objects` 取规范化后的 `route:<path>` 供 `http_route` 细则与审计消费。

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

**怎么做（各 kind 判定的实现思路）**：`check_constraint` 是**纯函数**——只读 `spec` 与已物化的 `ci.objects`（§3.1 提取的对象集），不触底层资源、不发 IO（IO 上下文已在 `classify` 阶段消化进 `objects`，故本步可在 `evaluate` 前由内核先行物化为 `ConstraintCheck` 入参，见 §6.1/CONS-8）。各 kind 按"判定对象的提取方式"分两类：

- **集合包含类**（`table_allow`/`http_route`/`container_prefix`/`key_prefix`/`command_class`/`vhost_allow`/`queue_prefix`/`route_allow`/`path_whitelist`）：从 `ci.objects` 取出该 kind 关心的对象维度（表集 / `(method,path)` / 容器名 / key / 命令类 / vhost / 队列 / routing key / 路径），逐一对照 `spec` 的白名单（精确集合或前缀集合）。判据是**全称量化**——请求触达的**每一个**对象都须落在白名单内，**任一**对象越界即 `Ok(false)`。为什么是全称而非存在量化：存在量化（"有一个命中即放行"）会让一条触达 N 张表的语句只要有 1 张在白名单就整体放行，是典型 fail-open。
- **触达禁止类**（`column_mask` 在求值期的形态）：`spec` 声明的是**禁止触达**的敏感列集；判定是 `ci.objects` 列集与禁止集的交集**必须为空**，非空即 `Ok(false)`。它与 `mask_fields` 的差别是**生效阶段不同**——`column_mask` 在求值期拒绝**触达**（请求根本不许碰这些列），`mask_fields` 在出口期擦除**响应**（列可读但值被脱敏），二者非同一防线、不可相互替代。
- **声明形态类**（`command_template`/`script_template`/`mask_fields`）：`check_constraint` 对 `command_template`/`script_template` 判"请求是否为某预声明模板的合法实例化"（模板外的自由命令一律不命中 → `Ok(false)`）；`mask_fields` 在本步**不参与求值期判定**（恒视作通过），它只是声明形态，真正的擦除由内核出口 `Sanitizer` 执行（见 §4）。
- **判定所需信息缺失即 `Err`**：当 `ci.objects` 不足以判定（如某 kind 需要的对象维度在 `classify` 阶段未能可靠提取）时返回 `ConstraintError` 而非 `Ok(true)`——"判不了"必须等价于"不通过"，绝不放行（L-7）。

合并语义遵循《详细设计文档》5.2"约束合并"：同 `kind` 多行默认取交集（全部须满足，fail-closed）；不同 `kind` 之间恒 `AND`；任一 `kind` 若采"白名单并集"语义须在其 `spec` 文档显式声明扩权特性（默认不取并集）。

### 3.3 能力声明与 `engine_enforced`

`protocol() -> &'static str`、`capabilities() -> &'static [Capability]`、`engine_enforced() -> bool`：

- **`engine_enforced = true`（SQL 类，如 postgres）**：存在**引擎级强制兜底**——`Decision::Allow{tier}` 选定的凭据等级在数据库引擎账号层受真实权限约束，即便 `classify` 把一条伪装写误归为 `Query`，它也只走只读账号、被引擎拒绝。归类是第一道线，引擎账号是强制兜底。
- **`engine_enforced = false`（HTTP/容器类，如 http、docker_logs）**：不存在引擎级账号分级，**归类 + 细则是唯一防线**。此差异须在该 `Adapter` 的文档与 `engine_enforced()` 返回值中**如实标注**（公理三、6 节失败语义对齐）。

**怎么做（为什么由适配器声明、声明为真凭什么成立）**：

- **`engine_enforced()` 是编译期常量声明，不是运行期推断**：每个 `Adapter` 实现把自己协议是否具备"引擎级账号权限模型"硬编码为返回值——SQL 类返回 `true`、HTTP/容器类返回 `false`。**为什么由适配器声明而非内核统一判定**：是否存在引擎级账号分级是**协议固有事实**，只有该协议的解释者（适配器）知道；内核不解释协议、无从推断，故这条事实的属主只能是适配器（与 §3.6 Intent 负载、§3.2 细则语义同属"协议解释者唯一持有"）。
- **`engine_enforced=true` 凭什么"真的"成立**：引擎兜底真实有效的前提是"tier 声明的动词集 ⊆ 底层账号真实权限"（详设 6.3）——这不是自动成立的，须经接入时校验取证。适配器在此的贡献是 §3.5 `discover` 探出账号的**真实权限**（如 `has_table_privilege`），供运维/`postern verify` 常规项比对"声明 `readonly` 的账号是否实测可写"。即：`engine_enforced=true` 是适配器的**声明**，其**为真的取证**由同一适配器的 `discover` 供给，二者合起来才让"伪装写恒被只读账号拦下"这条兜底落地。适配器本身**不选 tier、不感知 tier**（§4 边界），只声明"本协议存在兜底"这一布尔事实。

### 3.4 `execute`

`async execute(ch, intent) -> Result<RawResponse, ExecError>`：在 `Channel` 上执行**已被求值放行**的 `Intent`，产出未脱敏 `RawResponse`（脱敏由内核出口完成，见 §4）。

**怎么做（各 adapter 如何在 Channel 上执行）**：`Channel` 对适配器呈现为一个"本地可用通路"的字节抽象（见 §6.3），适配器在其上跑各自协议的客户端语义，**只见通路、不见传输种类/地址/凭据**：

- **postgres（走 SQL）**：在 `Channel` 上以 PostgreSQL 线协议执行 `Intent` 携带的 SQL（经一个把 `Channel` 当作底层流的 pg 客户端）。执行前**绝不重新解析或改写**已放行的 SQL——`classify` 阶段的语法树只用于定档，执行用的是原文，避免"解析与执行看到不同语句"的二义。会话只读等语义已在**建连时**由连接管理层统一施加（见 §3.1 `SET` 一律拒绝、详设 6.3 归池前 `DISCARD ALL` 净化），适配器不在 `execute` 内补打 `SET`。结果集以未脱敏字节装入 `RawResponse`。
- **docker_logs（走只读日志端点）**：把 `Intent` 的取数参数（容器选择 + `since`/`tail`/`follow`）翻译为对**只读日志端点**的请求，两种取数形态——①**直连**远端运行时已暴露的只读 API（安全前提是远端已自行限定该端点只读、可达性受控，网关不为远端越权暴露兜底）；②经**远端只读探针**取数（探针把"只读"下沉到远端进程边界，即便远端运行时能力更宽，探针只转发只读取数，最小暴露面、安全性更高，代价是需远端部署升级探针）。无论哪种形态，适配器对其的消费一致——经 `Channel` 发取数请求、收字节流；两种形态的安全取舍与部署差异详见详设 6.12。**只发取数、不发任何写/控制动作**：端点能力面在 `Intent` schema 与远端探针处即被限定为只读（探针能力面恒不含写动词），适配器侧无写路径可走。`follow` 形态产出的是**流式** `RawResponse`，由内核出口按流式模型脱敏（详设 6.4）。
- **http（走转发）**：把 `Intent` 的 `(method, path, headers, body)` 经 `Channel` **转发**到目标 HTTP 端点，回传未脱敏的状态码/头/体。转发是**忠实搬运**——不在适配器侧改写请求语义（路径/方法已在 `classify`+`check_constraint` 处被白名单约束），凭据由连接管理层在建连边界注入、适配器不经手（`engine_enforced=false`，故归类+细则是该请求合法性的唯一保证，执行阶段不再有第二道判别）。

- **只执行已放行意图**：`execute` 是管线步骤[8]，前置步骤[1]~[6] 求值放行、[7a] 意图审计、[7b] 取连接均已成立后才被调用（见 §6.1）。
- **错误经脱敏后返回，已执行请求绝不返回 deny**：`ExecError` 沿出口经 `Sanitizer` 返回；对有副作用动词，一旦底层已落库副作用，绝不再返回 deny（《详细设计文档》6.1 时序不变量，由内核守护）。
- **会话副作用形态**：对存在会话副作用、无法可靠净化的请求形态，连接管理层将禁用连接复用（即建即用即弃）——该决策属连接管理（8.5），适配器经 `Channel` 抽象消费，不参与池化判定。

### 3.5 `discover`：能力面探测

`async discover(ch) -> Result<CapabilitySurface, DiscoverError>`：真实连上资源探测其能力面。

**怎么做（如何探测能力面）**：`discover` 在递来的 `&mut Channel` 上**只发只读的元信息探测**，把资源的"客观能力事实"装进 `CapabilitySurface`，各 adapter 探测维度不同：

- **postgres**：探测引擎版本、可见 schema/表清单、以及该接入账号的**真实权限**（如 `has_table_privilege`/默认权限）。后者尤为关键——它是详设 6.3"tier 声明权限 ⊆ 底层账号真实权限"这一前提的取证来源：一个声明 `readonly` 的账号若实测拥有写权限，运维据 `discover` 产出可见此缺口（`postern verify` 常规项据此报警）。
- **docker_logs**：探测远端运行时/探针的协议版本与可达的只读端点，确认能力面恒为只读（探针 `protocol_version` 与网关协商，不兼容版本 → `DiscoverError`，详设 6.12）。
- **http**：探测端点可达性与（若资源提供）声明式 API 描述，作为运维声明动词工具映射的事实底稿。
- **产物是纯事实、零授权字段**：`CapabilitySurface` 只装"资源具备何种能力"，**绝不**含任何 allow/tier/grant 字段；授权化是人经控制面圈选的后续动作（发现≠授权）。探测失败（连不上、版本不兼容、权限不可读）一律 `DiscoverError`，绝不据失败结果或部分结果生成任何授权（fail-closed）。

- **仅控制面可触发**（速查表"能力面发现 → 适配器执行，仅控制面可触发"）：经 `daemon::control` 的 `POST /v1/resources/{code}/discover` 进入，是接入侧探测，供运维圈选授权。
- **发现≠授权**：`discover` 只产出**事实**（资源具备何种能力），绝不产生任何授权；授权化由人经控制面圈选。
- **与数据面 `postern_surface` 严格区分**（CONS-20）：数据面 `postern_surface` 是授权快照的纯事实投影，**不触达底层资源、不调用 `Adapter::discover`**；二者命名边界由命名规范固化，禁止互相借用。

### 3.6 Intent 负载格式

本 crate 定义各协议 `Intent` 的结构，并定义 MCP 动词工具的参数 schema（`postern_query` / `postern_observe` / `postern_mutate` / `postern_execute` / `postern_manage` / `postern_destroy` 的 `request` 形态）。外壳层只把该负载**忠实装箱搬运、不增不减、不解释**（8.12 范围外）；解释权唯一归适配器。

**怎么做（负载结构如何组织、为什么解释权唯一归适配器）**：

- **每协议一套自洽的 `Intent` 负载类型，是 classify/check_constraint/execute 三方法的共同入参形态**：负载结构既要能被 `classify` 解读出动词与对象（如 postgres 负载携 SQL 原文、docker_logs 负载携容器选择符 + 只读取数参数、http 负载携 `(method, path, headers, body)`），又要能被 `execute` 直接在 `Channel` 上回放执行。**关键设计取舍**：`classify` 与 `execute` 看到的是**同一份原始负载**——`classify` 阶段产出的语法树/归类只用于定档，绝不改写负载；`execute` 用的仍是负载原文（见 §3.4 postgres"绝不重新解析或改写"），杜绝"解析时与执行时看到不同请求"的二义。
- **负载须可序列化往返且逐字段稳定**（F-12 判定基准）：MCP 动词工具向 Agent 暴露的 `request` 即该负载的对外 schema，经外壳层装箱、跨进程搬运后须能无损反序列化回适配器——故负载类型的 schema 是适配器对外契约的一部分，序列化→反序列化往返后逐字段相等是其正确性底线。
- **为什么解释权必须唯一归适配器**：外壳层只识别协议**形态**（语法层 4xx，8.12），不理解协议**语义**；内核做授权但不解释协议。一条 `Intent` 负载"是什么动词、碰哪些对象、该怎么在通路上执行"这组语义判断，全系于协议解释者。若把负载解释权分散到外壳或内核，会出现"同一负载在不同层被解读为不同意图"的裂缝（公理七要求外壳差异不引起安全语义差异）——故负载格式的**定义权**与**解释权**收敛到唯一属主适配器，是"协议唯一解释者"定位（§1）在数据结构层的落点。

### 3.7 实现要点与工程约束

本小节收口本 crate 的工程落地要点，是 §3.1~§3.6"做什么/怎么做"在并发、错误、性能、测试、可观测性层面的工程约束。与全局工程规范（《详细设计文档》7.x）一致处只一句话引用，不重抄。

**并发与线程模型**

- **`classify` / `check_constraint` 同步纯函数，`execute` / `discover` async**：前两者无 IO（`check_constraint` 在 `classify` 物化 `objects` 之后才跑、零 IO，故内核可在 `evaluate` 前先行物化为 `ConstraintCheck` 入参，保 `evaluate` 纯逻辑零 IO，CONS-8）；后两者在 `Channel` 上做协议 IO，跑在 tokio runtime 上、由内核 await。`Adapter: Send + Sync`——同一 `Adapter` 实例被多请求并发共享调用，故实现须**无内部可变共享态**（归类表/动词映射为不可变只读结构），并发安全由"无共享可变态 + `&self` 方法"而非锁达成。
- **不持有连接、不做池化判定**：适配器经 `&mut Channel` 借用一条通路、用完即还，**不缓存 `Channel`、不跨请求复用**；连接的取/还/净化/弃用全在连接管理层（8.5），`execute`/`discover` 内不出现池化、退避、健康判定逻辑。会话副作用形态是否禁用复用由连接管理层决策，适配器只如实声明该形态（经协议语义），不参与判定。
- **tokio 任务边界**：适配器本身**不 spawn 后台任务**（无独立 worker、无定时器）；流式 `execute`（如 docker_logs `follow`、HTTP 流式体）以 `RawResponse` 的流式形态把背压交还内核，由内核出口的有界缓冲/背压模型承接（详设 6.4），适配器不自建无界缓冲。

**错误处理与传播**

- **一个 `thiserror` 枚举，变体→拒绝阶段映射在 `core::error` 穷尽**：新增错误变体未写映射则无法编译（无 `_ =>` 通配兜底，详设 7.1；§5、L-15）。
- **各方法的失败唯一表达是 `Err`，由内核翻译为 fail-closed**：`ClassifyError → deny`（步骤[2]，公理二）；`ConstraintError`/`Ok(false) → deny`（步骤[4]）；`ExecError → 经出口 Sanitizer 脱敏返回`，但有副作用动词一旦已落库副作用绝不再 deny（步骤[8]/[10] 时序不变量由内核守护，适配器经 `Err` 协同）；`DiscoverError → 接入侧拒绝/报缺口`（控制面，绝不据失败产授权）。
- **绝不吞错放行**：求值相关路径禁 `.ok()`/`.unwrap_or(true)`/`.unwrap_or_default()` 等吞错放行写法（与内核 `EVAL_NO_ERROR_SWALLOWING` 协同，本 crate 经 clippy `-D warnings` 兜底，L-14/B-6）。
- **panic 政策**：遵全局 7.1——`unsafe_code = forbid`，`unwrap_used`/`expect_used`/`panic`/`indexing_slicing`/`arithmetic_side_effects` 等 deny；万一仍 panic，由数据面外壳 CatchPanic 层转为脱敏 deny + `anomaly` 审计（适配器侧不靠 panic 表达失败，失败一律走 `Err`）。

**性能与资源边界**

- **`classify` 单趟 AST 遍历**：定档与对象提取共一趟自顶向下走查，复杂度与语句树节点数线性相关；对**解析产物规模设上界**（语句树节点数 / CTE 与子查询嵌套深度 / 对象集基数），超界按不可靠归类 `Err`——防御深度炸弹式 SQL 把 `classify` 拖成 DoS（公理二，宁可误拒）。
- **`execute` 不持额外内存副本**：结果以流式/借用形态装入 `RawResponse`，大响应（容器日志、流式体）走流式不全量驻留内存；连接数/超时/并发上限由连接管理层统一施加（详设 6.3），适配器不自设池但其流式形态须能被上游有界缓冲背压。
- **`check_constraint` 复杂度**：与 `objects` 基数 × `spec` 白名单规模相关的集合包含判定，均为有界小集合上的线性/对数匹配，无回溯式爆炸。

**测试策略**

- **`classify` 用内存语料集、无需真实资源**（纯函数收益）：维护一份"伪装攻击语料集"（写 CTE 包裹 / 子查询藏写 / 多语句 / 注释混淆 / `DO` 块 / `SET` 篡改 / `EXPLAIN ANALYZE` 写），**两层断言**——每条要么归其**真实最高危档**、要么 `Err`，**无一条降级为低危档放行**（详设 7.3；L-1/L-2/L-6；`postern verify` 项1/项2 的本 crate 前提）。`check_constraint` 同样以内存 `spec` + 物化 `ci` 驱动（白名单内/外、缺信息三类断言，L-7/L-8）。
- **`execute` 两层断言的 postgres 容器集成测试**：在真实 PostgreSQL 容器上验证——①**功能层**：放行意图在 `Channel` 上正确执行并回未脱敏结果；②**安全层（引擎兜底取证）**：一条被误归 `Query` 的写若经只读账号执行，**被引擎拒绝**——印证 `engine_enforced=true` 的强制兜底是真实的、不是文档声明（详设 6.3、6.7 项1）。docker_logs/http 以只读端点/转发目标的轻量 Fake 或容器验证执行路径与流式脱敏交接。
- **deny 路径"零调用"断言**：内核在 deny 短路时**不到达** `execute`，以"deny 路径下 `Adapter::execute` 调用计数==0、零副作用"判定（L-10）。

**可观测性**

- **适配器不落审计、不写运行日志记凭据/地址**：审计的落点与时序在内核（7a/10），适配器**不直接调 `AuditSink`**；它对可观测性的贡献是产出 `ClassifiedIntent.objects`（规范化的 `schema.table`/`route:<path>`/`container:<名>`）——这是内核审计 `denied.objects`/intent/outcome 事件**唯一**的对象事实来源，"对象供审计消费"即指此（§2 职责、§4 边界）。
- **机密红线（详设 7.5）**：适配器侧本就拿不到真实地址/凭据（只见 `Channel`，§6.3）；`Intent` 原文（SQL 可含业务敏感数据）、任何错误串在跨边界前不得入运行日志、不得回显——`ExecError`/`ClassifyError` 经内核出口 `Sanitizer` 套统一安全文案信封后才外泄，绝不携带原始 intent 或底层串（如 `connection refused to 10.0.3.17` 一类，适配器侧拿不到、也绝不构造）。
- **`engine_enforced()` 的诚实声明本身是一项可观测事实**：SQL 类返回 `true`、HTTP/容器类返回 `false`，且 `false` 者须在模块文档显式标注"归类+细则是唯一防线"（公理三；L-9 以返回值单元判定 + 文档标注串结构检查）。

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

> 本节是 `postern-adapters` 的**验收基准**：拿这份清单可逐条判定开发实现的"功能写全没、逻辑对不对"。每条 = **要求 + 通过判定**，通过判定对当前代码只有"通过/不通过"一个答案，无歧义、可复现；判定方式按条目而定（行为观察 / 接口存在 / Stele 契约绿红 / 结构检查），不强求都是单元测试。
>
> 说明：本 crate 的行为级通过判定挂到场景规格（`docs/examples/02/04/05/07`）与红队 `postern verify`（详细设计 6.7 九项）；§7 引用的 `UNCLASSIFIABLE_INTENT_DENIED`、`CONS-20` 是运行时行为契约，不在现行 24 条 Stele 静态契约内，其相关条目以行为观察判定并标注。

### 一、功能完整性（判断：该有的功能都写了吗、行为对吗）

| 编号 | 要求（必须实现） | 通过判定（满足即过，否则不过） |
|---|---|---|
| F-1 `Adapter` 七方法实现 | postgres / docker_logs / http 三实现各提供 `protocol`/`capabilities`/`engine_enforced`/`classify`/`check_constraint`/`execute`/`discover`，签名与 §5 core 定义一致 | 三实现全部存在且能编译；每个 `Adapter` impl 七方法俱全、签名逐一对齐 core trait（缺一方法或签名不符则编译失败） |
| F-2 classify·纯只读归 Query（§3.1） | SQL 无写节点、无 `INTO` 归 `Query` | `classify("SELECT id,status FROM public.orders WHERE status='paid'")` → `Ok(ClassifiedIntent{ capability=Query, objects=["public.orders"] })`（场景 04 §4.1 Trace ①[2]） |
| F-3 classify·Insert/Update/Merge 归 Mutate（§3.1） | 写但非删除/删表/清表 → `Mutate` | `classify("INSERT INTO public.t VALUES(...)")` → `Ok(capability=Mutate)`；`UPDATE`/`MERGE` 同档 |
| F-4 classify·Delete/Drop/Truncate 归 Destroy（§3.1） | 最高危写为删除/删表/清表 → `Destroy` | `classify("DELETE FROM public.orders WHERE id=1")` → `Ok(capability=Destroy)`（顶层即 Destroy，不降为 Mutate） |
| F-5 classify·Show/EXPLAIN(非 ANALYZE) 归 Observe（§3.1） | 只读元信息观测 → `Observe` | `classify("EXPLAIN SELECT * FROM public.orders")` → `Ok(capability=Observe)`；`SHOW search_path` → `Observe` |
| F-6 classify·docker_logs 恒 Observe（§3.1） | 容器日志请求恒只读 | docker_logs `classify(取容器日志 Intent)` → `Ok(capability=Observe, objects=["container:<名>"])`（场景 04 §4.1 Trace ③[2]） |
| F-7 classify·http 按动词工具/路径归类（§3.1） | HTTP 请求按声明动词工具+路径归相应 `Capability` | http `classify({method:"POST", path:"/api/orders"})` → `Ok(capability=Mutate, objects=[route:/api/orders])`（场景 04 §4.1 Trace ②[2]） |
| F-8 check_constraint 各 `kind` 语义（§3.2） | 本 crate 是各 `kind`（`table_allow`/`column_mask`/`container_prefix`/`http_route`/`command_template`/`script_template`/`path_whitelist`/`key_prefix`/`command_class`/`vhost_allow`/`queue_prefix`/`route_allow`/`mask_fields`）语义属主，对一个已物化 `ci` 按 `spec` 判通过/不通过 | `table_allow` 命中白名单内表 → `Ok(true)`、白名单外表 → `Ok(false)`；`http_route` 命中白名单 `(method,path)` → `Ok(true)`、白名单外 → `Ok(false)`；`container_prefix` 前缀匹配 → `Ok(true)`、不匹配 → `Ok(false)`（场景 04 §4.1 Trace ①/②/③ 步骤[4]） |
| F-9 execute 执行放行意图产出未脱敏响应（§3.4） | 在 `Channel` 上执行**已放行** `Intent`，产出**未脱敏** `RawResponse` | 给定已放行 `Intent` + `&mut Channel`（内存 Fake）→ `Ok(RawResponse)`，其内容为原始未脱敏字节（脱敏归内核出口，本方法不擦字节；场景 04 §4.1 Trace ①/②/③ 步骤[8]） |
| F-10 engine_enforced 如实声明（§3.3） | 经返回值如实申明是否有引擎级强制兜底 | postgres `engine_enforced()==true`；http `engine_enforced()==false`；docker_logs `engine_enforced()==false` |
| F-11 discover 真实探测产出能力面（§3.5） | 真实连上资源探测，产出 `CapabilitySurface`（纯事实） | 给定 `&mut Channel` → `Ok(CapabilitySurface)`，其字段全为"资源具备何种能力"的事实、无任何授权字段（场景 02 §4.1 步骤6） |
| F-12 Intent 负载格式 / MCP 动词工具 schema 定义（§3.6） | 本 crate 定义各协议 `Intent` 结构与 `postern_query`/`postern_observe`/`postern_mutate`/`postern_execute`/`postern_manage`/`postern_destroy` 的 `request` schema | 这些 `Intent`/`request` 类型定义在本 crate（非 core、非外壳）；同一负载序列化→反序列化往返后逐字段相等 |

### 二、逻辑正确性（判断：关键逻辑、边界、失败处理对不对）

| 编号 | 要求（行为必须正确） | 通过判定 |
|---|---|---|
| L-1 CTE/子查询里写节点不降级（§3.1，核心） | 危险动词在整棵语句树可见，CTE/子查询里的 DELETE/DROP/TRUNCATE 一律提升为 `Destroy`，绝不因只读外壳包裹而降级 | `classify("WITH x AS (DELETE FROM public.orders RETURNING *) SELECT * FROM x")` → `Ok(capability=Destroy)`，**绝不**为 `Query`/`Mutate`（场景 04 §4.2 A；场景 07 §C 项2；`postern verify 项2`） |
| L-2 子查询藏写归 Destroy（§3.1） | 子查询内 DELETE/DROP/TRUNCATE 同样按最高危写定档 | 子查询藏 `DELETE` 的语句 → `Ok(capability=Destroy)`，不降级 |
| L-3 `SET` 一律拒绝（§3.1） | `SET` 改会话语义、不属只读观测，无白名单逃生口 | `classify("SET search_path = ...")` → `Err(ClassifyError)`；任意 `SET ...` 形态均 `Err`（无任何放行分支） |
| L-4 EXPLAIN ANALYZE 按内部最高危写定档（§3.1） | `EXPLAIN ANALYZE` 真实执行被解释语句，按其内部最高危写节点定档 | `classify("EXPLAIN ANALYZE DELETE FROM public.orders")` → `Ok(capability=Destroy)`（非 Observe）；`EXPLAIN ANALYZE INSERT ...` → `Mutate` |
| L-5 无法可靠归类 → `Err` → deny（fail-closed，§3.1） | 解析失败/多语句/未知节点/`DO`/`COPY`/`CALL` 一律 `Err` | 上述每种输入 `classify(...)` → `Err(ClassifyError)`（白名单外宁可误拒）；内核据此 deny、不进 [3]（场景 04 §4.1 Trace ①[2]、§4.2 A；运行时契约 `UNCLASSIFIABLE_INTENT_DENIED`，行为观察判定） |
| L-6 伪装攻击全集收敛为正确归类或 `Err`（§2 职责7，收敛于 §3.1） | 写 CTE / 子查询藏写 / 多语句 / 注释混淆 / `DO` 块 / `SET` 篡改 → 各自归正确高危档或 `Err`，无一降级放行 | 对"伪装攻击语料"全集逐条断言：每条要么归其真实最高危档、要么 `Err`，**无一条降级为低危档放行**（场景 07 §C 项1/2；`postern verify 项1`、`postern verify 项2`） |
| L-7 check_constraint false/Err → deny（§3.2） | 不通过返回 `Ok(false)`，无法判定返回 `Err(ConstraintError)`，二者皆经内核翻译为拒绝 | 白名单外输入 → `Ok(false)`；判定所需信息缺失/异常 → `Err(ConstraintError)`；两者均不返回 `Ok(true)`（场景 04 §4.1 Trace ③[4]） |
| L-8 细则合并·同 kind 取交集、跨 kind 恒 AND（§3.2） | 默认 fail-closed，不取并集（除非 `spec` 显式声明扩权） | 同 `kind` 多行任一行不满足 → `Ok(false)`；不同 `kind` 任一不满足 → `Ok(false)` |
| L-9 engine_enforced 返回值与防线标注一致（§3.3） | postgres 有引擎账号兜底、http/docker_logs 无；后者归类+细则是唯一防线，须经返回值与文档如实标注 | `postgres.engine_enforced()==true`；`http.engine_enforced()==false`、`docker_logs.engine_enforced()==false`；且 http/docker_logs 模块文档含"归类+细则是唯一防线"标注串（返回值为单元判定，标注为结构检查；本条不依赖 `postern verify`——verify 项1 是内核+引擎层红队结果，挂于 DoD） |
| L-10 execute 只执行已放行意图（§3.4） | `execute` 是步骤[8]，仅在 [1]~[6] 放行、[7a] 审计、[7b] 取连接均成立后被调用 | 内核管线在 deny 短路时**不到达** `execute`（如场景 05 §4.1 步骤1 越权 `manage` 在 [3] RBAC 短路，`execute` 不被调用、零副作用、确未重启容器）；以"deny 路径下 `Adapter::execute` 调用计数==0"判定，行为观察 |
| L-11 已执行请求绝不返回 deny（§3.4） | 有副作用动词一旦底层已落库副作用，错误经内核出口脱敏返回，绝不再返回 deny | 注入"执行已落副作用后出错"的路径 → 返回脱敏后的 `ExecError`，**非 deny**（场景 04 §4.1 Trace ②[8]/[10]；时序不变量由内核守护，本 crate 经 `Err` 协同，行为观察判定） |
| L-12 discover 仅控制面触发·发现≠授权（§3.5） | `discover` 只由控制面触发、只产事实、不产授权；数据面无触发路径 | 数据面 `postern_surface` 是授权快照投影、**不调用** `Adapter::discover`、不触底层资源；`discover` 产物 `CapabilitySurface` 无授权字段（场景 02 §4.1 步骤6；CONS-20 命名边界，行为观察/结构检查判定） |
| L-13 只见 Channel，拿不到地址/凭据/tier（§4） | 适配器只见 `&mut Channel` 抽象，不知传输种类/真实地址/凭据/tier | `execute`/`discover` 签名入参仅 `&mut Channel` + `&Intent`（无 `CredentialTier`/`ResolvedTarget`/`ResourceCredential`/地址类型形参——签名结构检查，缺即不符 §5 承诺）；执行中断 → 适配器经 `Channel` 收错并以 `ExecError` 上报，本 crate 源码内无真实地址/凭据字面量来源（与 B-1 禁止边、B-3 不构造机密协同；场景 04 §4.2 G 拿不到 app 账号、§4.2 C 脱敏出口） |
| L-14 适配器绝不吞错放行（§7，fail-closed） | 任一失败唯一表达为 `Err`，无静默 `Ok`/降级放行路径 | `classify`/`check_constraint`/`execute`/`discover` 的失败均为 `Err(...)`；无 `.ok()`/`.unwrap_or(true)`/`.unwrap_or_default()` 等吞错放行写法（与内核 `EVAL_NO_ERROR_SWALLOWING` 协同；本 crate 经 B-6 clippy 退出码 0 兜底） |
| L-15 错误枚举→拒绝阶段映射穷尽（§5） | 本 crate 一个 `thiserror` 枚举，其变体到拒绝 `stage` 映射在 `core::error` 穷尽 | 新增错误变体未在 `core::error` 写映射则**无法编译**（穷尽 `match`、无 `_ =>` 通配兜底，编译期保证） |
| L-16 适配器只产归类、不产决策（§4 边界） | 适配器既不产 `Decision`（allow/deny/escalate）也不选 `CredentialTier`；授权判定归 `core::eval` | `classify` 返回 `ClassifiedIntent`、`check_constraint` 返回 `bool`、二者签名返回类型均**不含** `Decision`/`CredentialTier`；本 crate 源码无 `Decision::Allow`/`Decision::Deny`/`Decision::Escalate` 构造点、无 `CredentialTier` 选择逻辑（签名 + 结构检查，缺即不符 §4/§5） |

### 三、边界与不变量（机器强制，绿/红即答案）

| 编号 | 要求 | 通过判定（机器） |
|---|---|---|
| B-1 无 adapters↛secrets/transports/store 禁止边（§4/§6/§7） | 依赖图无任一禁止边；只见 Channel、不可达地址凭据是编译期事实 | 契约 `ARCH_FORBIDDEN_EDGES`（+ `_TEETH`）绿；`cargo tree -p postern-adapters -e normal` 不含 `postern-secrets`/`postern-transports`/`postern-store` |
| B-2 唯一依赖边 adapters→core（§6） | 本 crate 仅依赖 `postern-core` | `cargo tree -p postern-adapters` 工作区内依赖仅 `postern-core`；`cargo deny check bans` 退出码 0 |
| B-3 不构造机密类型（§4/§7） | 本 crate 内无 `ResolvedTarget`/`ResourceCredential` 构造点 | 契约 `SEC_CONSTRUCTION_SITES`（+ `_TEETH`）绿 |
| B-4 机密类型不可复制/序列化（§4/§7） | `ResolvedTarget`/`ResourceCredential` 不可 Clone/Serialize，本 crate 无从复制传递 | 契约 `SEC_SECRET_TYPE_DISCIPLINE`（+ `_TEETH`）绿 |
| B-5 不构造 `ConnOrigin`（§4） | `ConnOrigin` 仅 daemon shells(listener) 构造，本 crate 无构造点 | 契约 `SEC_CONSTRUCTION_SITES`（+ `_TEETH`）绿（其规则含 `ConnOrigin` 构造点约束） |
| B-6 lint 红线 | 无 unwrap/expect/panic/吞错放行 | `cargo clippy -p postern-adapters --all-features -- -D warnings` 退出码 0 |

### 通过定义（DoD）

`postern-adapters` **算完成** ⟺ 一、二、三三组**每一条都通过**。任一条不过 = 不通过，必须修。F 类靠"七方法存在且签名对齐 + 给定输入看归类/细则/执行是否符合通过判定"；L 类靠"触发某条件→行为恰为某可观察结果"（含核心 fail-closed：CTE 写不降级 L-1/L-6、归类失败即拒 L-5、`SET` 即拒 L-3、细则 false/Err 即拒 L-7、已执行不返 deny L-11、discover 发现≠授权 L-12、不吞错放行 L-14；只产归类不产决策 L-16）；B 类靠"跑契约/cargo tree|deny/clippy 看绿红"。

另设**集成级红队门**（非本 crate 单元判定，须在 daemon 组装后整链跑过，达成才算整体收尾）：`postern verify 项1`（伪装写经只读账号被引擎/归类拒，整体 deny）、`postern verify 项2`（CTE DELETE 经 mutate 授权按最高危写归 Destroy 被拒，整体 deny）须 PASS——二者依赖内核管线 + 引擎账号，本 crate 经 L-1/L-6 正确归类与 L-9 如实标注 `engine_enforced` 为其供给必要前提。
