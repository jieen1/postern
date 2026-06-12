# postern-store 模块详细设计

> 本文是 `postern-store` crate 的模块级详细设计，在《详细设计文档》第八部分 8.11（存储层）的领域裁决之上展开。结构遵循《模块详细设计·索引与规约》（[00-模块详细设计-索引与规约.md](00-模块详细设计-索引与规约.md)）规定的七小节。纯设计，不含任何实现代码、阶段划分或进度状态。与本文冲突时，以《技术设计文档》的七公理与《详细设计文档》第八部分的领域裁决为准。

---

## 1. 定位（一句话）

`postern-store` 是 postern 的**存储载体域**：以"事务"与"append-only"两种纪律，把权威策略状态（policy.db）与审计事件流（按日轮转 JSONL）可靠落地，并向上层提供**唯一写路径**、**只读策略快照**与**append-only 审计写入**三类受约束的访问形态。它是库（非二进制），机密的载体不在本域（归机密面 8.8）。

---

## 2. 承载领域与职责范围

本 crate 承载《详细设计文档》第八部分 **8.11 存储层**这一单一领域。职责范围（封闭列举）：

1. **policy.db schema 与迁移** — 全部策略状态表的结构定义（每表 8 个统一基础字段 + 业务字段）、约束、索引、`PRAGMA` 设置；schema 版本经 `PRAGMA user_version` 管理与迁移。
2. **统一基础仓储（`base` 模块）** — 全工作区**唯一的 SQL 写路径**与受约束读路径。承载契约 D8 的全部落点：8 基础字段、审计字段自动填充、乐观锁、逻辑删除、默认作用域过滤、后端分页执行、固定宽度时间戳生成、归一化入库。
3. **PolicyRepo** — 全部策略状态的**事务**读写编排（rusqlite，同步 API），一律经 `base` 仓储；仅控制面可达。
4. **PolicySnapshot 构建（`snapshot` 模块）** — 在一次事务内全量加载、完成角色继承展开与授权空间物化，产出权威库的**原子投影**，供策略引擎只读消费；内容清单（含/不含）见 §3.4。
5. **JsonlAuditSink（`audit` 模块）** — `core::AuditSink` 的实现：按 UTC 日轮转、物理 append-only 的 JSONL 写入；按日期文件倒序、分页窗口截断的扫描查询；审计存储健康与拒绝服务防护语义的载体侧落地。
6. **`PolicyView` 实现** — 对外暴露 `snapshot() -> Arc<PolicySnapshot>` 的无锁只读视图，是数据面与策略状态之间唯一的桥。

---

## 3. 支持的功能

按对外接口组织（接口签名见 §5）。

### 3.1 统一基础仓储（`base`）—— 唯一写路径

`base` 是 INSERT/UPDATE 语句**唯一允许出现的位置**（契约 `DB_WRITE_PATH_CENTRALIZED`），其能力：

- **审计字段自动填充**：`version / created_at / created_by / updated_at / updated_by` 由 `base` 自动维护，调用方**无法**传入这些字段（仓储 API 不暴露相应参数）。`created_by / updated_by` 取值：控制面写入 = 已认证操作者标识；系统自动写入（sweeper 回收、import 协调）= `system`。
- **乐观锁**：UPDATE 形态恒为 `SET version = version + 1 ... WHERE id = ? AND version = ?`；影响行数为 0 → 版本冲突错误，**不静默重试**。期望 `version` **唯一来源是调用方先前读取值**——`base` 仓储**不得自读自比**（自读自比会使乐观锁恒成立、等于失效）。区分两类写：①"用户意图写"必带期望 version、参与乐观锁；②"系统协调写"（sweeper 谓词幂等更新）不参与乐观锁，因系统写是幂等谓词驱动、无"读后写"竞态。
- **逻辑删除**：删除一律 `UPDATE ... SET delete_flag = 1`（连带 version 自增与 updated_* 维护）；不提供物理删除 API，工作区不存在针对 policy.db 的 `DELETE` 语句（契约 `DB_LOGICAL_DELETE_ONLY`）。不提供 undelete 接口——逻辑删除是终态，重建 = 新增同名新行（partial unique 允许）。
- **级联逻辑删除**：父表逻辑删除时，在**同一事务**内级联把直接子行置 `delete_flag = 1`，`updated_by` 标注级联来源（如 `cascade:resources#<id>`）。级联图见 §3.2。
- **默认作用域构建器**：集合/单条查询默认追加 `delete_flag = 0`，业务查询默认看不到已删数据（契约 `DB_DEFAULT_SCOPE_EXCLUDES_DELETED`）；`enable_flag` 不在默认过滤内（启停是业务语义，由调用方按表语义显式使用）。
- **后端分页执行**：集合查询函数必须接收 `PageQuery`（定义于 core），SQL 必须 `LIMIT ? OFFSET ?`，返回 `Page<T>` 信封（契约 `DB_PAGINATION_MANDATORY`）；禁止无界查询、禁止"全量取回内存再切片"、禁止把分页交给前端。
- **限制性表禁 `enable_flag`**：对 `grant_constraints / grant_conditions / mode_state / deny_notes` 四表，`base` 拒绝写入非 1 的 `enable_flag`（建表层另有 `CHECK (enable_flag = 1)` 兜底）；停用一条限制只能经显式删除或显式 `mode set`，绝不能是悄无声息的 flag 翻转（fail-closed，见 §7）。
- **固定宽度时间戳生成**：`base` 是 policy.db 时间列与审计 `ts` 的**唯一格式化点**，恒 `YYYY-MM-DDTHH:MM:SS.sssZ`（恒 UTC、恒 `Z` 后缀、恒 3 位毫秒、长度恒 24），保证文本字典序可比、TTL/sweeper 的 `< now` 判定不错序。
- **归一化入库**：`principals.name`、`roles.name`、`resources.codename` 等入库前由 `base` 统一归一化（`trim` + 明示大小写策略），唯一索引作用于归一化值（防 `Admin`、` admin ` 类绕过）。

### 3.2 policy.db schema 与迁移

承载《详细设计文档》5.2 的全部策略状态表。每表含 8 个统一基础字段（契约 `DB_BASE_FIELDS_REQUIRED`）；时间列固定宽度 UTC ISO-8601 文本 + `CHECK (length(col) = 24)`；JSON 列由对应插件定义并校验语义；`PRAGMA foreign_keys=ON`；schema 版本走 `PRAGMA user_version`；WAL 模式。

表清单（业务字段见 5.2，本文不复述全部列，只点出存储层关键约束）：

- 授予性表：`principals` / `roles` / `role_inherits` / `role_capabilities` / `resources` / `resource_credential_tiers` / `bindings` / `binding_scope` / `credentials`（credentials 因领域终态字段禁 enable_flag）/ `temp_grants`（禁 enable_flag）。
- 限制性表（禁 `enable_flag`，建表带 `CHECK (enable_flag = 1)`）：`grant_constraints` / `grant_conditions` / `mode_state` / `deny_notes` / `settings`。
- 唯一性统一采用 **partial unique index**（`WHERE delete_flag = 0`），逻辑删除后同名记录可重建、历史行保留可追溯。
- `mode_state` 全局辖区唯一性：唯一索引作用于 `COALESCE(scope_resource_id, 0)`（雪花 id 恒正，`0` 作全局哨兵），杜绝全局模式多行并存使 freeze 被旁路。
- `roles.name` 用 `CHECK (lower(trim(name)) <> 'admin')`（模型层硬约束，配合 §3.1 归一化，防大小写/空白绕过；契约 `SEC_ADMIN_NOT_GRANTABLE` 校验 roles 表带禁 admin 名 CHECK）。
- `temp_grants` 终态字段：`ended_at` + `end_reason CHECK (end_reason IN ('expired','revoked'))`（sweeper 写 `expired`、人工写 `revoked`，语义分明）。
- 级联逻辑删除图（§3.1 落点）：`resources → {resource_credential_tiers, binding_scope, grant_constraints, grant_conditions, mode_state(scope_resource_id), deny_notes}`；`principals → {credentials, bindings, temp_grants}`；`roles → {role_inherits, role_capabilities, bindings}`；`bindings → binding_scope`。

**迁移（`migrate` 模块）**：以 `PRAGMA user_version` 标识 schema 版本，提供版本前向迁移；迁移在事务内执行；版本不被当前实现识别时 fail-closed（不按旧假设解析未知 schema）。

### 3.3 PolicyRepo —— 策略状态事务读写

`PolicyRepo` 把 §3.1 的 `base` 能力组织为面向策略状态的事务读写（principals/credentials/roles/bindings/resources/tiers/constraints/conditions/temp_grants/mode/deny_notes/settings 等）。所有写一律经 `base`，一律在事务内；读端点统一返回 `version`（供乐观锁端到端贯通，见 §6 与控制面交互）。rusqlite 同步 API：写由控制面单线程串行化（写互斥锁），读只在快照重建时发生。

### 3.4 PolicySnapshot 构建与内容清单

`snapshot` 模块在一次事务内全量加载并完成角色继承展开、授权空间物化，产出 `Arc<PolicySnapshot>`——权威库的**原子投影**。求值零库访问、微秒级（在内存查表）。重建与写入在**同一写锁临界区**完成 Arc 原子替换，保证"单一权威状态"无双源。

**快照加载条件**（按表语义分两类，与 fail-closed 一致）：

- **授予性表**：加载 `delete_flag = 0 AND enable_flag = 1`（停用即收回授权）。
- **限制性表**：仅 `delete_flag = 0`，**不引入 enable_flag 过滤**（限制性表的 enable_flag 已被 §3.1/§3.2 禁用；若过滤会构成扩权/解冻的 fail-open）。

**快照构建固化的 fail-closed 兜底**（存储层职责）：

- **引用链可见性过滤**：引用链上任一父行不可见（`delete_flag=1` 或不存在）⇒ 该行不可见——即便级联遗漏也不放行悬挂引用。
- **同辖区多生效模式取最严格**：万一同辖区仍出现多行生效模式，取最严格者（`freeze > maintain > observe > normal`）并写告警审计，绝不取最宽松者。

**PolicySnapshot 内容清单**（数据面与策略状态之间唯一的桥，其边界即 2.3 隔离声明的实际含义）：

- **含**——展开后的授权空间（绑定×角色继承 ∪ 未过期临时授权）、凭证元数据（含 `secret_hash`，即明文哈希）、tier 声明、`grant_constraints` / `grant_conditions`、各辖区 mode、`deny_notes`、approval 设置。
- **不含**——任何 vault 内容（资源凭据、真实地址映射）。
- 每项仅供其消费域读取；面向 Agent 的出口只暴露该 Principal 自身授权世界（出口约束在内核，本域只保证快照内容不含机密）。

> 时间到期语义不进快照新鲜度：`temp_grants.expires_at`、`credentials.expires_at`/`revoked_at`、可信域时效、`mode_state.expires_at` 均由策略引擎在**求值时刻按墙钟二次校验**（TTL 过期判定归策略引擎，见速查表）；快照只是原子投影，不随墙钟自动推进。本域不承担 TTL 过期判定职责。

### 3.5 JsonlAuditSink —— append-only 审计载体

`JsonlAuditSink` 实现 `core::AuditSink`。能力：

- **按 UTC 日轮转**：事件落 `<data_dir>/audit/YYYY-MM-DD.jsonl`，文件物理只追加。
- **写入策略落地**：`deny`/`policy_change`/`credential_event` 逐事件 fsync；`allow` 类默认逐事件 fsync，可经 `settings: audit.fsync=relaxed` 改为 1s 周期批量。事件 `id` 取自 core IdGen，雪花 id 序列化为字符串。
- **分页扫描查询（`scan`）**：按日期文件**倒序**扫描，按分页窗口截断（契约 `DB_PAGINATION_MANDATORY` 对扫描查询同样适用）；不一次性全量读入内存。
- **DoS 防护语义的载体侧落地**（区分两类故障，见 §7）：审计落盘独立配额与水位监控；逼近上限触发可感知降级（告警事件、强制轮转、低价值审计降采样）而非数据面瞬间瘫痪；deny 类事件按窗口聚合计数（写带 `count` 的聚合记录）；保留期有界默认（`audit.retention_days`），到期文件整体删除（append-only 域中唯一允许的删除形态——整文件删除，非行级修改）。

> 审计事件的 schema、kind 体系、记录纪律的**定义**归观测面（8.9）；本域是其物理载体与扫描执行者。"审计写失败 → deny"的处置归数据面内核（8.2）；本域只如实返回写入成败。

---

## 4. 明确边界（不做什么）

每项指明归属域。

- **不定义"存什么"的语义** — 策略 schema 字段语义归领域模型（8.1）、凭证元数据规则归身份与凭证（8.4）、审计事件 schema 与 kind 归观测面（8.9）。本域只承载结构与载体。
- **不发起写入、不判定写入合法性** — 一切策略写操作的发起与合法性校验归控制面（8.10）及各属主域；本域是被调用的载体，不主动写。
- **不消费快照、不求值** — `PolicySnapshot` 的消费、RBAC 展开查表、tier 选择、决策产出归策略引擎（8.3）；本域只构建快照、不读其内容做判断。
- **不持有/不接触机密** — 资源凭据、代号↔真实地址映射、ScrubSet 全部归机密面（8.8）。policy.db **不存任何真实地址与凭据明文**（`secret_hash` 除外，其为单向哈希）；vault 引用以 `vault://` 字符串存储。本 crate 在依赖图上**禁止依赖** `postern-secrets`（依赖图允许 store→core，不允许 store→secrets）。
- **不生成 id** — 雪花 id 唯一来源是 `postern-core::id::IdGen`；本域消费它（契约 `DB_UNIFIED_ID_GENERATOR` 封禁 uuid/ulid/nanoid 等替代库）。
- **不做 TTL 过期判定** — 过期判定在求值时刻由策略引擎按墙钟执行（8.3 / 速查表）；sweeper（归控制面 8.10 自动机）只做可见性回收写入；本域只提供事务写与快照构建载体。
- **不定义审计记录纪律、不处置审计写失败** — 记录纪律（allow/deny 都记、两阶段时序、fsync 策略、DoS 防护语义）归观测面（8.9）定义；"审计不可记 = 不放行"的处置归数据面内核（8.2）。
- **不被 adapters / transports / cli 触达** — 这三者在依赖图上**禁止依赖** `postern-store`（契约 `ARCH_FORBIDDEN_EDGES`）。它们需要的策略事实只能经数据面内核（快照投影）或控制面 API 间接获得。
- **不暴露控制面/数据面端点** — HTTP/JSON 端点形态归外壳层与控制面（8.10/8.12）；本域只提供库级类型与 trait 实现。

---

## 5. 对外接口

本节为设计承诺（与《详细设计文档》第四部分 4.2 一致），非实现。标注定义方与实现方。

### 5.1 本域**实现**的 core trait

- `impl core::PolicyView for ...` —— `fn snapshot(&self) -> Arc<PolicySnapshot>`。trait 定义在 core（8.1），实现在本域；提供无锁只读快照。
- `impl core::AuditSink for JsonlAuditSink` —— `fn record(&self, event: AuditEvent) -> Result<(), AuditError>`。trait 定义在 core（8.1），实现在本域。

### 5.2 本域**定义并导出**的类型与 API（供 daemon 组装点消费）

- `PolicySnapshot` —— 权威库的原子投影；内容清单见 §3.4。被策略引擎只读消费、被 `PolicyView::snapshot` 返回（包裹于 `Arc`）。
- `PolicyRepo` —— 策略状态事务读写编排句柄；仅控制面（daemon::control + sweeper）可达。其写 API **不暴露**审计字段参数（审计字段自动化的接口表达），更新/删除 API **要求携带期望 `version`**（乐观锁端到端贯通的接口表达）；集合读 API **要求 `PageQuery`** 并返回 `Page<T>`（分页强制的接口表达）。
- `JsonlAuditSink` —— append-only 审计载体；除 `AuditSink::record` 外提供 `scan(filter, page) -> Page<AuditEvent>`（倒序、分页）。
- `base` 仓储 API —— 仅本 crate 内部可见（写路径唯一是审计字段自动化、乐观锁、逻辑删除、默认作用域成立的前提）；不作为跨 crate 公开接口。
- `migrate` —— schema 版本检查与迁移入口；由 daemon 启动序列调用。

### 5.3 本域**消费**的 core 定义（store→core 依赖边，依赖图唯一允许的工作区内依赖）

- `core::id::IdGen` —— 雪花 id 来源（表主键与审计事件 id）。
- `core::page::{PageQuery, Page<T>}` —— 统一分页类型（`DEFAULT_SIZE=20`、`MAX_SIZE=200`、`clamp`）。
- `core::AuditEvent` 及其信封字段定义（schema 由观测面 8.9 定义、core 承载类型）。
- 领域类型（`PrincipalId` / `ResourceCode` / `CredentialTier` / `Capability` 等）——快照展开与 schema 行映射所需的纯数据类型。

---

## 6. 与相邻模块的交互

依据权威依赖图：本域唯一允许的工作区内依赖是 `postern-store → postern-core`；本域被 `postern-daemon`（唯一组装点）依赖。本域**绝不**被 `postern-adapters` / `postern-transports` / `postern-cli` 依赖（契约 `ARCH_FORBIDDEN_EDGES`），故下文不存在与这三者的任何交互边。

### 6.1 ← postern-core（store 依赖 core）

- **方向**：本域调用/消费 core 的定义；本域**实现** core 定义的 `PolicyView` 与 `AuditSink` trait。
- **内容**：消费 `IdGen`（生成主键与事件 id）、`PageQuery`/`Page<T>`（分页）、`AuditEvent` 与领域类型（`PrincipalId`/`ResourceCode`/`CredentialTier`/`Capability`/`Timestamp` 等纯数据）；本域产出 `PolicySnapshot` 由 core 的 `Evaluator` 消费（经 daemon 注入，见 6.3），向 core 的 `AuditSink` 契约提交 `AuditEvent`。
- **时机**：编译期建立依赖；运行期在 schema 行映射、快照构建、审计写入时使用这些类型。
- **失败语义**：core 是零 IO 纯类型层，无运行期失败注入本域；本域对 core 类型的使用错误（如 id 生成时钟回拨）沿 core 既定 fail-closed 语义传播（IdGen 时钟回拨拒绝生成，绝不产出可能重复的 id）。

### 6.2 ← postern-daemon::boot（启动序列）

- **方向**：daemon 启动序列调用本域。
- **内容**：开库（policy.db，WAL）→ 调 `migrate` 校验/迁移 schema 版本 → 构建首个 `PolicySnapshot` → 装配 `PolicyView` / `JsonlAuditSink` 句柄注入两平面 router。
- **时机**：启动序列"开库 → 重建快照"步，**在开放数据面之前**（数据面对外可达前必须已有权威快照与可用审计载体）。
- **失败语义**：开库失败、schema 版本不被识别、首个快照构建失败、审计目录不可写 → **fail-closed 拒绝启动**（公理二），daemon 不进入服务状态；绝不以空快照或降级载体开放数据面。

### 6.3 ← postern-daemon::kernel（数据面内核，只读路径）

- **方向**：内核（步骤 [2]–[6] 求值前置）调用本域只读视图；内核（步骤 [7a]/[10] 审计）调用本域审计写入。
- **内容**：
  - **读快照**：`PolicyView::snapshot() -> Arc<PolicySnapshot>`（无锁、每请求取一次）。内核把该快照传给 `core::Evaluator::evaluate`；本域只交付不可变快照，不参与求值。
  - **审计写入**：`AuditSink::record(event)`——步骤 [7a] 有副作用动词的 **intent** 事件、步骤 [10] 的 **outcome**/请求结果事件；连接管理层（daemon::connpool）在通路建立/健康剔除/回收时同样经本域 `JsonlAuditSink` 落 `connection_event`（本域只作载体，审计 kind 与字段语义归观测面 8.9 定义）。
- **时机**：求值管线每请求读一次快照（步骤 [1][3][5][6] 在该快照上查表）；审计在步骤 [7a]（执行前 intent）与 [10]（执行后 outcome / 只读动词单次），以及连接管理层通路建立/健康剔除/回收时。
- **失败语义**：
  - 快照读取本身无锁、不失败（Arc 克隆）；快照内**不含** TTL 终判，过期判定由内核传入 `now`、由 `Evaluator` 二次校验（本域不兜底时序）。
  - 审计 `record` 返回 `Err` 时，**处置在内核**（8.2/6.1）：只读动词审计写失败 → 该请求按 deny 返回；有副作用动词 intent 写失败 → 执行前 deny（此时确未执行），outcome 写失败 → 返回"已执行但审计降级"错误（绝不返回 deny）。本域只如实返回成败、不自行决定放行与否。
  - **本域注入内核的句柄集合中不存在 `PolicyRepo` 与 vault 句柄**（数据面无读写策略/机密的路径，由构造函数签名 + 契约 `ARCH_FORBIDDEN_EDGES` 保证）——内核只持 `PolicyView`（只读）与 `AuditSink`。

### 6.4 ← postern-daemon::control（控制面 + sweeper，写路径）

- **方向**：控制面 API handler 与 sweeper 自动机调用本域 `PolicyRepo` 做事务读写，并触发快照重建。
- **内容**：
  - **事务写**：每次策略变更经 `PolicyRepo` 在事务内执行（经 `base` 自动填充审计字段 + 乐观锁 + 逻辑删除 + 级联）；写后 COMMIT 成功，在**同一写锁内重建 `PolicySnapshot`**（Arc 原子替换），并由控制面落 `policy_change`/`mode_change`/`credential_event` 等审计事件（经本域 `JsonlAuditSink`）——"写入 = 一次事务 + 快照重建 + 审计事件"三联动。
  - **事务读**：控制面读端点经 `PolicyRepo` 取数（分页、默认作用域过滤），响应统一返回 `version` 供乐观锁端到端贯通。
  - **sweeper（系统自动机，actor=system）**：把过期 `temp_grants`/`mode`/`credentials`/超时审批项按谓词幂等回收（写 `ended_at='expired'` 等），不参与乐观锁；回收后同样触发快照重建与审计。
- **时机**：每条管理命令一次（控制面 RPC 进入后）；sweeper 周期触发。快照重建与事务写处于同一临界区。
- **失败语义**：
  - 乐观锁版本不匹配 → 版本冲突错误（控制面映射为 `409 Conflict` 并写 `policy_change` 审计），本域**不静默重试**。
  - 事务任一步失败 → ROLLBACK，权威库不变更，快照不重建（无半截状态）。
  - 写入限制性表非 1 的 `enable_flag`、对 policy.db 发起 `DELETE`、绕过 `base` 的散落写 → 在仓储 API 层即不可表达 / 在契约层被拒（`DB_*` 系列），fail-closed。
  - import 协调失败整体拒绝（无部分 apply）——本域以单事务承载"期望状态 apply"，失败即 ROLLBACK。

### 6.5 与机密面（postern-secrets）的关系——**无依赖边**

- 本域**不依赖、不调用、不感知** `postern-secrets`。资源凭据、代号↔真实地址映射、ScrubSet 全部归机密面持有（8.8）。
- policy.db 仅以 `vault://` 字符串引用机密（`transport_config` 非敏感项、`secret_ref` 等），**不存任何明文真实地址或凭据值**；二者在不同载体（vault.postern 由机密面自持），通过引用字符串解耦。这是 fail-closed 的存储层前提，而非交互边。

---

## 7. 必守不变量

每项标注强制来源（Stele 契约 / 构造函数签名 / schema CHECK / 领域裁决）。

1. **写路径唯一** — INSERT/UPDATE 只在 `base` 模块；任何其他位置的写语句即违规。强制：契约 `DB_WRITE_PATH_CENTRALIZED`（+ `_TEETH` 反例自检：base 之外的 INSERT 必被检出）。这是审计字段自动化与乐观锁成立的前提。
2. **审计字段自动化** — `version/created_at/created_by/updated_at/updated_by` 由 `base` 填充，API 不暴露其参数。强制：仓储 API 签名 + 契约 `DB_WRITE_PATH_CENTRALIZED`。
3. **乐观锁不自读自比** — 期望 `version` 唯一来源是调用方读取值；影响行数 0 → 冲突错误，不静默重试。强制：仓储 API 签名（更新/删除必带期望 version）+ 控制面端到端贯通（6.4）。
4. **只有逻辑删除** — 删除 = `UPDATE delete_flag=1`；工作区无任何针对 policy.db 的 `DELETE`；不提供物理删除与 undelete 接口。强制：契约 `DB_LOGICAL_DELETE_ONLY`（+ `_TEETH`：`DELETE FROM` 必被检出，连 base 内部也不豁免）。
5. **默认作用域排除已删** — 默认查询追加 `delete_flag = 0`。强制：契约 `DB_DEFAULT_SCOPE_EXCLUDES_DELETED`（+ `_TEETH`）。
6. **统一基础字段齐备** — 每张业务表声明全部 8 个基础字段。强制：契约 `DB_BASE_FIELDS_REQUIRED`（+ `_TEETH`：缺字段表必被检出）。
7. **后端分页强制** — 集合查询接收 `PageQuery`、SQL `LIMIT` 封顶、返回 `Page<T>`；审计扫描同样分页截断；禁止无界查询与前端分页。强制：契约 `DB_PAGINATION_MANDATORY`（+ `_TEETH`：无 LIMIT 的集合 SELECT 必被检出）。
8. **裸 SQL 不出本 crate** — SQL 字符串与 `rusqlite`/`sqlparser` 依赖只允许在 `postern-store`；例外仅经受保护的 `contract/sql-exceptions.json` 人工审批登记（初始空）。强制：契约 `DB_NO_RAW_SQL_OUTSIDE_STORE`（+ `_TEETH`：store 外的 SQL 标记与依赖声明必被检出）。
9. **统一雪花 id** — 主键与审计 id 取自 `core::id::IdGen`；不引入替代 id 库。强制：契约 `DB_UNIFIED_ID_GENERATOR`（+ `_TEETH`）+ `cargo deny` 的 `[bans]`。
10. **限制性表禁 `enable_flag`** — `grant_constraints/grant_conditions/mode_state/deny_notes` 建表带 `CHECK (enable_flag = 1)`，`base` 拒绝写入非 1 值；解除限制只能经显式删除或显式 `mode set`（各写专门审计）。强制：schema CHECK + `base` 写校验（fail-closed，防扩权/解冻）。
11. **限制性表快照不过滤 `enable_flag`** — 快照对限制性表仅按 `delete_flag = 0` 加载，绝不引入 enable_flag 过滤（否则构成 fail-open）。强制：`snapshot` 模块加载规则（领域裁决 5.2）。
12. **时间列固定宽度、单一格式化点** — 恒 `YYYY-MM-DDTHH:MM:SS.sssZ`、长度恒 24；`base` 是 policy.db 时间列与审计 `ts` 的唯一格式化点。强制：schema `CHECK (length(col)=24)` + `base` 唯一格式化函数。
13. **快照重建与写入同临界区** — 快照是权威库的原子投影，重建与事务写在同一写锁内完成 Arc 替换，无双源。强制：`PolicyRepo`/`snapshot` 协调（领域裁决 6.2）。
14. **快照 fail-closed 兜底** — 引用链上父行不可见 ⇒ 该行不可见；同辖区多生效模式取最严格者并写告警审计。强制：`snapshot` 构建逻辑（领域裁决 5.2 级联与全局辖区兜底）。
15. **本库不存机密** — 不存任何真实地址与凭据明文（`secret_hash` 单向哈希除外）；机密仅以 `vault://` 引用存储。强制：领域裁决 8.11 + 依赖图禁止 store→secrets 边（`ARCH_FORBIDDEN_EDGES`）。
16. **审计 append-only** — 无行级修改与物理删除语义，唯一允许的删除是保留期到期的整文件删除；事件永不含凭据值与真实地址。强制：JSONL 载体形态 + 观测面记录纪律（8.9）+ 写入前过 Sanitizer（在内核出口，本域只落已脱敏事实）。
17. **数据面无写策略/机密路径** — 注入数据面 router 的句柄集合不含 `PolicyRepo` 与 vault 句柄，只有 `PolicyView`（只读）+ `AuditSink`。强制：daemon 构造函数签名审查 + 契约 `ARCH_FORBIDDEN_EDGES`（7.2-2 红线）。
18. **PolicyRepo 仅控制面可达** — 事务读写句柄不进入数据面依赖集合；本域不被 adapters/transports/cli 依赖。强制：依赖图（`ARCH_FORBIDDEN_EDGES`）+ daemon 注入约束。

---

*交互对象一览*：本模块与 **postern-core**（消费其 IdGen/分页/领域类型，并实现其 `PolicyView`/`AuditSink` trait）、**postern-daemon::boot**（启动开库/迁移/首个快照）、**postern-daemon::kernel**（数据面只读取快照 + append-only 审计写入）、**postern-daemon::control + sweeper**（事务写 + 快照重建 + 审计三联动）交互；与 **postern-secrets** 仅为"无依赖边、经 `vault://` 引用解耦"的关系；**绝不**被 postern-adapters / postern-transports / postern-cli 依赖（契约 `ARCH_FORBIDDEN_EDGES`）。

---

## 8. 验收标准

本节是 `postern-store` 的**验收基准**：每条给出"输入 → 可观察结果"的判据与**验证方式**（统一词汇见 [00 §8 规约](00-模块详细设计-索引与规约.md)）。维度 A 逐条对应 §3 功能、C 逐条对应 §4 边界、D 逐条对应 §7 不变量、E 逐条对应 §6 交互。**完成定义见 §8.7。** 标「**审查**」者为暂无机器规则覆盖、须靠构造签名审查 / 人工代码审查判定的项（已逐条标出）。

### A. 功能完整性（对应 §3）

| # | 功能（§3 落点） | 输入 → 预期可观察结果 | 验证方式 |
|---|---|---|---|
| A1 | 审计字段自动填充（§3.1） | 经 `PolicyRepo` 写一行，调用方未传 `version/created_at/created_by/updated_at/updated_by` → 落库行五字段非空、`version=0`、`created_at=updated_at`、时间戳长 24；控制面写 `created_by=`操作者标识，sweeper 写 `created_by/updated_by='system'` | 集成测试(真实资源:临时 SQLite)；构造签名审查（API 不暴露这五参数，**审查**） |
| A2 | 乐观锁（§3.1） | 持 `version=k` 更新一行 → UPDATE 形如 `SET version=version+1 ... WHERE id=? AND version=?`，落库 `version=k+1`；持过期 `version` 再更新同行 → 影响行数 0 → 返回版本冲突错误、**无重试**、库不变 | 集成测试(真实资源:临时 SQLite)；场景规格 docs/examples/06 §4.2#10、docs/examples/02 §4.2 E7 |
| A3 | 逻辑删除 + 无物理删除（§3.1） | 删一行 → 落库 `delete_flag=1`、`version` 自增、`updated_*` 维护；全工作区无任何 `DELETE FROM` 命中 policy.db；无 undelete API | 集成测试(真实资源:临时 SQLite)；Stele契约 `DB_LOGICAL_DELETE_ONLY`；构造签名审查（无 undelete/物理删除入口，**审查**） |
| A4 | 级联逻辑删除（§3.1/§3.2） | 删父行（如 `resources#x`）→ 同一事务内直接子行（按 §3.2 级联图）`delete_flag=1`、`updated_by` 含 `cascade:resources#<id>` 标注；ROLLBACK 时父子均不变 | 集成测试(真实资源:临时 SQLite) |
| A5 | 默认作用域过滤（§3.1） | 删一行后默认集合/单条查询 → 结果不含该行；显式带 `delete_flag` 谓词的查询才可见已删行；`enable_flag` 不被默认过滤 | 集成测试(真实资源:临时 SQLite)；Stele契约 `DB_DEFAULT_SCOPE_EXCLUDES_DELETED` |
| A6 | 后端分页（§3.1） | 集合查询函数接收 `PageQuery` → SQL 含 `LIMIT ? OFFSET ?`、返回 `Page<T>` 信封；页大小经 `clamp`（>200 截到 200）；无"全量取回内存再切片" | 集成测试(真实资源:临时 SQLite)；Stele契约 `DB_PAGINATION_MANDATORY` |
| A7 | 限制性表禁非 1 `enable_flag`（§3.1/§3.2） | 向 `grant_constraints/grant_conditions/mode_state/deny_notes` 写 `enable_flag=0` → `base` 写校验拒绝 + 建表 `CHECK(enable_flag=1)` 兜底拒绝，库不变 | 集成测试(真实资源:临时 SQLite)；场景规格 docs/examples/06 §4.2#8 |
| A8 | 固定宽度时间戳（§3.1） | `base` 生成的任一时间列/审计 `ts` → 恒 `YYYY-MM-DDTHH:MM:SS.sssZ`、恒 UTC/`Z`/3 位毫秒、长度恒 24；两时间文本字典序与时间序一致 | 单元测试（格式化函数）；集成测试(真实资源:临时 SQLite) `CHECK(length(col)=24)` |
| A9 | 归一化入库（§3.1） | 以 `Admin`/` admin `/`ADMIN` 写 `principals.name` 等 → 入库为归一化值，partial unique 作用于归一化值，绕过性重复被唯一索引拒 | 集成测试(真实资源:临时 SQLite)；Stele契约 `SEC_ADMIN_NOT_GRANTABLE`（roles 禁 admin 名 CHECK） |
| A10 | schema 8 基础字段 + 约束（§3.2） | 每张业务表声明全 8 基础字段；时间列带 `CHECK(length=24)`；`PRAGMA foreign_keys=ON`、WAL；限制性表带 `CHECK(enable_flag=1)`；`roles` 带禁 admin 名 CHECK；唯一性为 partial unique（`WHERE delete_flag=0`）；`mode_state` 唯一索引作用于 `COALESCE(scope_resource_id,0)` | Stele契约 `DB_BASE_FIELDS_REQUIRED`、`SEC_ADMIN_NOT_GRANTABLE`；集成测试(真实资源:临时 SQLite)（partial unique / 全局 mode 单行）；场景规格 docs/examples/02 §4.1 步骤 4 |
| A11 | schema 迁移（§3.2） | 以 `PRAGMA user_version` 标识版本，前向迁移在事务内执行；遇当前实现不识别的版本 → fail-closed 拒绝（不按旧假设解析） | 集成测试(真实资源:临时 SQLite)；场景规格 docs/examples/02 §4.1 步骤 1 |
| A12 | PolicyRepo 事务读写（§3.3） | 所有写经 `base`、在事务内；读端点统一返回 `version`；写由控制面单线程串行化（写互斥） | 集成测试(真实资源:临时 SQLite)；构造签名审查（读端点返回带 `version`、**审查**）；场景规格 docs/examples/03 §4.1 步骤 1~6 |
| A13 | PolicySnapshot 构建与内容清单（§3.4） | 一次事务内全量加载：授予性表按 `delete_flag=0 AND enable_flag=1`、限制性表仅 `delete_flag=0`；产出 `Arc<PolicySnapshot>`；内容**含**展开授权空间/凭证元数据(`secret_hash`)/tier/约束/条件/各辖区 mode/deny_notes/approval 设置，**不含**任何 vault 内容（资源凭据、真实地址映射）；不含 TTL 终判（不随墙钟推进） | 集成测试(真实资源:临时 SQLite)；构造签名审查（快照类型不含 vault 字段、**审查**）；场景规格 docs/examples/03 §4.1 步骤 2~4 |
| A14 | 快照 fail-closed 兜底（§3.4） | 引用链父行 `delete_flag=1`/不存在 → 该子行不在快照（即便级联遗漏）；同辖区出现多生效模式 → 取最严格（`freeze>maintain>observe>normal`）并写告警审计 | 集成测试(内存Fake:构造悬挂引用 + 多模式行)；场景规格 docs/examples/03 §4.2 G（悬挂引用不可见）、docs/examples/06 §4.2#7（多模式取最严格） |
| A15 | JsonlAuditSink 日轮转 + append-only 写（§3.5） | `record(event)` → 落 `<data_dir>/audit/YYYY-MM-DD.jsonl`（UTC 日）、物理只追加；`deny/policy_change/credential_event` 逐事件 fsync，`allow` 默认逐事件 fsync、`audit.fsync=relaxed` 时 1s 周期批量；事件 `id` 为雪花字符串 | 集成测试(真实资源:临时目录)；场景规格 docs/examples/07 §4.1-A、docs/examples/02 §4.1 步骤 1/3 |
| A16 | 审计分页扫描（§3.5） | `scan(filter, page)` → 按日期文件**倒序**、分页窗口截断返回 `Page<AuditEvent>`，不一次性全量读入内存 | 集成测试(真实资源:临时目录)；Stele契约 `DB_PAGINATION_MANDATORY`；场景规格 docs/examples/07 §4.1-A |
| A17 | 审计 DoS 防护载体落地（§3.5） | 逼近独立配额水位 → 触发可感知降级（告警事件 / 强制轮转 / 低价值降采样），高价值事件不降采样；deny 类按窗口聚合写带 `count` 记录；`audit.retention_days` 到期整文件删除（唯一允许的删除形态）；不出现"刷满磁盘 → 全平面瘫痪" | 集成测试(真实资源:临时目录:构造逼近配额)；场景规格 docs/examples/07 §4.2 E3 |

### B. 对外接口契约（对应 §5）

| # | 接口（§5 落点） | 判据 | 验证方式 |
|---|---|---|---|
| B1 | `impl core::PolicyView`（§5.1） | `snapshot(&self) -> Arc<PolicySnapshot>` 签名稳定、无锁、Arc 克隆不失败、每调用返回当前权威快照 | 构造签名审查（**审查**）；集成测试(内存Fake) |
| B2 | `impl core::AuditSink for JsonlAuditSink`（§5.1） | `record(&self, event) -> Result<(),AuditError>` 签名稳定；如实返回写入成败，**不**自行决定放行/拒绝（处置归内核） | 构造签名审查（**审查**）；集成测试(真实资源:临时目录) |
| B3 | `PolicyRepo` 写 API（§5.2） | 写 API **不暴露**审计字段参数；更新/删除 API **要求携带期望 `version`**；集合读 API **要求 `PageQuery`** 并返回 `Page<T>` | 构造签名审查（**审查**）；Stele契约 `DB_WRITE_PATH_CENTRALIZED`、`DB_PAGINATION_MANDATORY` |
| B4 | `JsonlAuditSink::scan`（§5.2） | `scan(filter, page) -> Page<AuditEvent>`：倒序、分页、错误路径如实返回（不吞） | 构造签名审查（**审查**）；集成测试(真实资源:临时目录) |
| B5 | `base` 仓储 API 私有性（§5.2） | `base` 仅 crate 内可见，不作跨 crate 公开接口（写路径唯一的前提） | Stele契约 `DB_WRITE_PATH_CENTRALIZED`、`DB_NO_RAW_SQL_OUTSIDE_STORE`；构造签名审查（可见性，**审查**） |
| B6 | `migrate` 入口（§5.2） | 暴露 schema 版本检查/迁移入口供 boot 调用；版本不识别即 fail-closed | 集成测试(真实资源:临时 SQLite)；场景规格 docs/examples/02 §4.1 步骤 1 |

### C. 边界（禁止项，对应 §4）

| # | 不做什么（§4 落点） | 「确实没做」判据 | 验证方式 |
|---|---|---|---|
| C1 | 不持有/不接触机密、本库不存机密 | policy.db 任一字段无明文真实地址/凭据值（`secret_hash` 单向哈希除外）；机密仅以 `vault://` 字符串存；store **不依赖** `postern-secrets`（无 store→secrets 边） | Stele契约 `ARCH_FORBIDDEN_EDGES`；cargo tree（store 依赖树无 `postern-secrets`）；场景规格 docs/examples/02 §4.1 步骤 2/3、§4.2 E4 |
| C2 | 不被 adapters/transports/cli 触达 | 依赖图无 `adapters→store`/`transports→store`/`cli→store` 边 | Stele契约 `ARCH_FORBIDDEN_EDGES`；cargo tree |
| C3 | 不生成 id | 主键/事件 id 仅取自 `core::id::IdGen`；工作区无 uuid/ulid/nanoid 等替代 id 库 | Stele契约 `DB_UNIFIED_ID_GENERATOR`；cargo deny `[bans]` |
| C4 | 不暴露裸 SQL（不让 SQL 出本 crate） | SQL 字符串与 `rusqlite`/`sqlparser` 依赖只在 `postern-store`；例外仅经 `contract/sql-exceptions.json` 登记（初始空） | Stele契约 `DB_NO_RAW_SQL_OUTSIDE_STORE`；cargo tree（依赖声明） |
| C5 | 不消费快照、不求值 | 本域只构建/返回快照，无读其内容做 RBAC 展开/tier 选择/决策的代码路径（消费归 8.3） | 构造签名审查（store 内无 `Evaluator`/决策逻辑、**审查**） |
| C6 | 不发起写、不判合法性 | 无本域主动发起的写；写合法性校验不在本域（被调用的载体） | 构造签名审查（写入口均由 control/sweeper 触发、**审查**） |
| C7 | 不做 TTL 过期判定 | 快照不含 TTL 终判、不随墙钟推进；`temp_grants/credentials/mode_state` 时效判定不在本域 | 集成测试(真实资源:临时 SQLite)（快照内容含 `expires_at` 原值但无过期裁决）；场景规格 docs/examples/06 §4.2#1 |
| C8 | 不定义审计纪律、不处置写失败 | 本域只如实返回 `record` 成败；审计 kind/schema/记录纪律不在本域定义；"审计不可记=不放行"的处置不在本域 | 构造签名审查（`record` 返回 `Result`、无放行/拒绝决策、**审查**）；场景规格 docs/examples/07 §4.2 E1/E2 |
| C9 | 不暴露控制面/数据面端点 | 本域仅库级类型/trait，无 HTTP/JSON 端点（端点归外壳/控制面） | 构造签名审查（无 HTTP 端点导出、**审查**） |

### D. 必守不变量（对应 §7，沿用 §7 已标强制手段）

| # | 不变量（§7 编号） | 验证判据 | 强制/验证方式 |
|---|---|---|---|
| D1 | 写路径唯一（§7-1） | base 之外的 INSERT/UPDATE 命中 policy.db 即违规；反例自检能检出 base 外 INSERT | Stele契约 `DB_WRITE_PATH_CENTRALIZED`（本体 + `_TEETH` 反例） |
| D2 | 审计字段自动化（§7-2） | 五字段由 base 填充、API 不暴露参数 | 构造签名审查（**审查**）+ Stele契约 `DB_WRITE_PATH_CENTRALIZED` |
| D3 | 乐观锁不自读自比（§7-3） | 期望 version 唯一来源为调用方读取值；影响 0 行 → 冲突不静默重试；base 不自读自比 | 构造签名审查（更新/删除必带期望 version、base 无自读自比、**审查**）；集成测试(真实资源)；场景规格 docs/examples/06 §4.2#10 |
| D4 | 只有逻辑删除（§7-4） | 工作区无任何针对 policy.db 的 `DELETE`（连 base 内部不豁免）；反例自检能检出 `DELETE FROM` | Stele契约 `DB_LOGICAL_DELETE_ONLY`（本体 + `_TEETH`） |
| D5 | 默认作用域排除已删（§7-5） | 默认 SELECT 追加 `delete_flag=0`；反例自检能检出无过滤 SELECT | Stele契约 `DB_DEFAULT_SCOPE_EXCLUDES_DELETED`（本体 + `_TEETH`） |
| D6 | 统一基础字段齐备（§7-6） | 每表声明全 8 基础字段；反例自检能检出缺字段表 | Stele契约 `DB_BASE_FIELDS_REQUIRED`（本体 + `_TEETH`） |
| D7 | 后端分页强制（§7-7） | 集合查询/审计扫描均 `LIMIT` 封顶并返回 `Page<T>`；反例自检能检出无 LIMIT 集合 SELECT | Stele契约 `DB_PAGINATION_MANDATORY`（本体 + `_TEETH`） |
| D8 | 裸 SQL 不出本 crate（§7-8） | SQL/`rusqlite`/`sqlparser` 只在 store；反例自检能检出 store 外 SQL 标记与依赖声明 | Stele契约 `DB_NO_RAW_SQL_OUTSIDE_STORE`（本体 + `_TEETH`）；cargo tree |
| D9 | 统一雪花 id（§7-9） | 主键/审计 id 取自 IdGen；无替代 id 库；反例自检能检出 Cargo.toml 中 uuid 依赖 | Stele契约 `DB_UNIFIED_ID_GENERATOR`（本体 + `_TEETH`）+ cargo deny `[bans]` |
| D10 | 限制性表禁 `enable_flag`（§7-10） | 四表建表带 `CHECK(enable_flag=1)`；base 拒非 1 写入 | 集成测试(真实资源)（CHECK + base 写校验）；场景规格 docs/examples/06 §4.2#8 |
| D11 | 限制性表快照不过滤 `enable_flag`（§7-11） | 快照对限制性表仅按 `delete_flag=0` 加载，无 enable_flag 过滤（否则 fail-open） | 集成测试(真实资源)（限制性表行 enable_flag 任意值仍入快照）；构造签名审查（snapshot 加载规则、**审查**） |
| D12 | 时间列固定宽度、单一格式化点（§7-12） | 恒 24 长度、`base` 唯一格式化函数 | 单元测试（格式化函数）+ 集成测试(真实资源) `CHECK(length=24)` |
| D13 | 快照重建与写入同临界区（§7-13） | 重建与事务写在同一写锁内完成 Arc 替换、无双源 | 构造签名审查（写锁临界区内重建、**审查**）；集成测试(真实资源)（写后快照即反映、无中间态）；场景规格 docs/examples/03 §4.1 步骤 7 |
| D14 | 快照 fail-closed 兜底（§7-14） | 悬挂引用不可见；多生效模式取最严格 + 写告警 | 集成测试(内存Fake)；场景规格 docs/examples/03 §4.2 G（悬挂引用）、docs/examples/06 §4.2#7（多模式取最严格） |
| D15 | 本库不存机密（§7-15） | 无明文真实地址/凭据（`secret_hash` 除外）；仅 `vault://` 引用；无 store→secrets 边 | Stele契约 `ARCH_FORBIDDEN_EDGES`；场景规格 docs/examples/02 §4.1 步骤 2、§4.2 E4 |
| D16 | 审计 append-only（§7-16） | 无行级修改/物理删除语义；唯一删除是保留期到期整文件删除；事件永不含凭据值/真实地址（写入前已过 Sanitizer，本域只落已脱敏事实） | 集成测试(真实资源:临时目录)（只追加 + 整文件删除）；场景规格 docs/examples/07 §4.2 E4 |
| D17 | 数据面无写策略/机密路径（§7-17） | 注入数据面 router 的句柄集仅 `PolicyView`(只读)+`AuditSink`，不含 `PolicyRepo`/vault 句柄 | 构造签名审查（daemon 注入句柄集签名、**审查**）；Stele契约 `ARCH_FORBIDDEN_EDGES` |
| D18 | PolicyRepo 仅控制面可达（§7-18） | 事务读写句柄不进数据面依赖集；本域不被 adapters/transports/cli 依赖 | Stele契约 `ARCH_FORBIDDEN_EDGES`；cargo tree；构造签名审查（注入约束、**审查**） |

### E. 与相邻模块交互（对应 §6，方向/类型/时机/失败语义可验）

| # | 交互（§6 落点） | 方向/类型/时机/失败语义判据 | 验证方式 |
|---|---|---|---|
| E1 | ← core（§6.1） | 方向：store 消费 core 定义并实现其 `PolicyView`/`AuditSink`；类型：`IdGen`/`PageQuery`/`Page<T>`/`AuditEvent`/领域类型；时机：schema 行映射/快照构建/审计写；失败：IdGen 时钟回拨拒绝生成（绝不产出可能重复 id），沿 core fail-closed 传播 | 集成测试(内存Fake:模拟时钟回拨)；构造签名审查（store→core 单向、**审查**） |
| E2 | ← daemon::boot（§6.2） | 方向：boot 调 store；类型/时机：开库 → `migrate` 校验 → 构建首个快照 → 装配 `PolicyView`/`JsonlAuditSink` 注入，**在开放数据面之前**；失败：开库失败 / 版本不识别 / 首个快照失败 / 审计目录不可写 → **fail-closed 拒绝启动**，绝不以空快照或降级载体开放数据面 | 集成测试(真实资源:临时 SQLite/目录:注入各类启动失败,断言不进入服务状态)；场景规格 docs/examples/02 §4.2 E9、docs/examples/05 §4.2 J（快照不可得→boot fail-closed） |
| E3 | ← daemon::kernel 只读快照（§6.3） | 方向：kernel 调 `PolicyView::snapshot`；类型：`Arc<PolicySnapshot>` 不可变、每请求取一次、无锁；时机：求值步骤 [1][3][5][6] 在快照上查表；失败：快照读不失败、不含 TTL 终判（过期判定由 `Evaluator` 按传入 `now` 二次校验） | 集成测试(内存Fake)；场景规格 docs/examples/06 §4.1-A（TTL 二次校验在引擎、本域只交付原值） |
| E4 | ← daemon::kernel 审计写（§6.3） | 方向：kernel 调 `AuditSink::record`；类型：[7a] intent / [10] outcome / connection_event；时机：执行前 intent、执行后 outcome、连接建立/剔除/回收；失败：`record` 返回 `Err` 时**处置在内核**，本域只如实返回成败、不自行放行/拒绝；注入数据面的句柄集不含 `PolicyRepo`/vault | 集成测试(真实资源:临时目录:注入写失败返回 Err)；构造签名审查（数据面句柄集、**审查**）；场景规格 docs/examples/07 §4.2 E1/E2 |
| E5 | ← daemon::control + sweeper 写路径（§6.4） | 方向：control/sweeper 调 `PolicyRepo` 事务写并触发重建；类型："写=一次事务+快照重建+审计事件"三联动，同一写锁临界区；时机：每条管理命令一次 / sweeper 周期；失败：乐观锁不匹配 → 版本冲突（control 映射 409、写 `policy_change`），事务任一步失败 → ROLLBACK（库不变、快照不重建、无半截状态），import 失败整体拒绝（无部分 apply）；sweeper actor=`system`、不参与乐观锁 | 集成测试(真实资源:临时 SQLite)；场景规格 docs/examples/03 §4.1 步骤 1~7、docs/examples/06 §4.1-A/B/D 与 §4.2#10、docs/examples/02 §4.2 E7 |
| E6 | ✗ secrets（§6.5，无依赖边） | 本域不依赖/不调用/不感知 `postern-secrets`；policy.db 仅以 `vault://` 引用机密、不存明文 | Stele契约 `ARCH_FORBIDDEN_EDGES`；cargo tree（无 store→secrets 边）；场景规格 docs/examples/02 §4.1 步骤 2 |

### F. 失败与边界行为（关键 fail-closed 路径）

| # | fail-closed 路径 | 输入 → 预期可观察结果 | 验证方式 |
|---|---|---|---|
| F1 | 启动不可建即拒绝 | 开库失败 / schema 版本不识别 / 首个快照构建失败 / 审计目录不可写 → daemon **拒绝启动**，不进入服务状态、不以空快照或降级载体开放数据面 | 集成测试(真实资源:临时 SQLite/目录)；场景规格 docs/examples/02 §4.2 E9、docs/examples/05 §4.2 J（快照不可得→boot fail-closed） |
| F2 | 事务失败即回滚 | 三联动任一步失败 → ROLLBACK，权威库不变更、快照不重建（无半截/无未审计的状态） | 集成测试(真实资源:临时 SQLite)；场景规格 docs/examples/06 §4.2#14 |
| F3 | 乐观锁冲突即拒绝 | 期望 `version` 不匹配 → 版本冲突错误、**不静默重试**、库不变 | 集成测试(真实资源:临时 SQLite)；场景规格 docs/examples/06 §4.2#10、docs/examples/02 §4.2 E7 |
| F4 | 限制性写即拒绝 | 限制性表写非 1 `enable_flag` / 对 policy.db 发 `DELETE` / 绕过 base 散落写 → 仓储 API 层不可表达或契约层被拒，库不变 | Stele契约 `DB_LOGICAL_DELETE_ONLY`、`DB_WRITE_PATH_CENTRALIZED`；集成测试(真实资源)；场景规格 docs/examples/06 §4.2#8 |
| F5 | 悬挂引用不放行 | 引用链父行不可见 → 子行不入快照（即便级联遗漏，绝不放行悬挂引用） | 集成测试(内存Fake:构造悬挂引用)；场景规格 docs/examples/03 §4.2 G、docs/examples/05 §4.2 J |
| F6 | 多生效模式取最严格 | 同辖区多生效模式 → 取最严格（`freeze>maintain>observe>normal`）+ 写告警审计，绝不取最宽松 | 集成测试(内存Fake)；场景规格 docs/examples/06 §4.2#7 |
| F7 | 审计写失败如实上报 | `record` 写失败 → 返回 `Err`（不吞、不自行放行）；处置（不可记=不放行 / outcome 写失败返"已执行但审计降级"）由内核据返回值决定 | 集成测试(真实资源:临时目录:注入写失败)；场景规格 docs/examples/07 §4.2 E1/E2 |
| F8 | 审计逼近配额可控降级 | 逼近独立配额 → 可感知降级（聚合 + 降采样 + 强制轮转 + 告警），高价值事件不丢；不出现 Agent 可触发的"刷满磁盘 → 全平面瘫痪"环 | 集成测试(真实资源:临时目录)；场景规格 docs/examples/07 §4.2 E3 |
| F9 | 红队自检留痕（载体侧） | `postern verify` 九项每条请求逐条出现在审计（八项 deny 事件含 `stage`+`reason`、第 8 项 allow 事件无敏感回显）——本域作为 append-only 载体如实落库、可复核 | postern verify（六维红队九项整体）；场景规格 docs/examples/07 §4.1-C |

> `postern verify` 九项（详设 6.7）属数据面/引擎/认证职责的运行期验证，本域不重复挂引擎侧判据；本域的相关验收落点是**作为 append-only 载体如实留痕九项探测**（F9）。启动期"`data.sock` 可连 uid 集合不含 daemon 自身 uid"属 boot 启动自检（详设 5.5 硬前置条件②），非 6.7 九项之一，故由 boot 侧集成测试验证、不在本域以 `postern verify <项>` 挂；本域只保证启动期数据面前置（首个快照 + 可用审计载体）就绪（E2/F1）。

### 8.7 完成定义（Definition of Done）

`postern-store` 当且仅当**同时满足上述 A~F 全部验收项**——A 的每项功能可按"输入→可观察结果"验过、B 的每个对外接口签名稳定且错误路径如实、C 的每条禁止项经契约/依赖图/构造签名证实"确实没做"、D 的 18 条不变量各由其标注的 Stele 契约（`DB_*` / `SEC_ADMIN_NOT_GRANTABLE` / `ARCH_FORBIDDEN_EDGES` 本体 + `_TEETH`）/ 构造签名审查 / 测试守住、E 的每个相邻交互按约定方向/类型/时机调用且失败语义为 fail-closed、F 的关键 fail-closed 路径逐条可验——方视为该模块完成。标「**审查**」的项无机器规则覆盖、须以构造签名审查或人工代码审查兜底判定。
