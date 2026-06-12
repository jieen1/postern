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

> 本节是 `postern-store` 的**验收基准**：拿这份清单可逐条判定开发实现的"功能写全没、逻辑对不对"。每条 = **要求 + 通过判定**，通过判定对当前代码只有"通过/不通过"一个答案，无歧义、可复现；判定方式按条目而定（行为观察 / 接口存在 / Stele 契约绿红 / 结构检查 / 依赖图），不强求都是单元测试。无机器规则覆盖、只能靠构造签名审查或人工代码审查判定的项，标「**审查**」并写成"满足某确切条件即过"的二元形式。
>
> 说明：第 7 节引用的不变量 §7-2/3/11/12/13/17/18 在现行 24 条 Stele 契约中无逐字对应规则，其相关条目以构造签名审查或集成测试的二元判据判定（已逐条标「**审查**」或给出确切输入→结果）。本域承载领域为存储载体，`postern verify`（详设 6.7）九项属引擎/认证侧运行期验证，本域只作为 append-only 载体如实留痕（见 L-15，对应详设 6.7 第 8 项脱敏探测在审计中无敏感回显）；启动期"`data.sock` 可连 uid 不含 daemon 自身 uid"属 boot 自检（详设 5.5），非 6.7 九项之一，不在本域以 `postern verify <项>` 挂。

### 一、功能完整性（判断：该有的功能都写了吗、行为对吗）

| 编号 | 要求（必须实现） | 通过判定（满足即过，否则不过） |
|---|---|---|
| F-1 审计字段自动填充（§3.1） | `version/created_at/created_by/updated_at/updated_by` 由 `base` 自动维护，API 不暴露这五参数 | 经 `PolicyRepo` 写一行、调用方不传这五字段 → 落库行五字段非空、`version=0`、`created_at==updated_at`、时间戳长度 24；控制面写时 `created_by` == 已认证操作者标识、sweeper 写时 `created_by==updated_by=='system'`；仓储 API 签名不含这五参数（**审查**） |
| F-2 乐观锁 `version`（§3.1） | UPDATE 恒 `SET version=version+1 ... WHERE id=? AND version=?`，期望 version 来自调用方读取值，`base` 不自读自比 | 持 `version=k` 更新一行 → 落库 `version==k+1`；更新/删除 API 签名要求携带期望 `version`（**审查**）；`base` 内无"先 SELECT version 再用于 WHERE"的自读自比（**审查**） |
| F-3 仅逻辑删除（§3.1） | 删除恒 `UPDATE ... SET delete_flag=1`，无物理删除、无 undelete | 删一行 → 落库该行 `delete_flag==1`、`version` 自增、`updated_*` 维护；仓储 API 无 undelete / 物理删除入口（**审查**）；契约 `DB_LOGICAL_DELETE_ONLY` 绿 |
| F-4 级联逻辑删除（§3.1/§3.2） | 父表逻辑删除时同一事务内级联置子行 `delete_flag=1` 并标注来源 | 删 `resources#x` → 按 §3.2 级联图其直接子行 `delete_flag==1`、`updated_by` 含 `cascade:resources#<id>`；事务 ROLLBACK 时父子行均不变 |
| F-5 默认作用域 `delete_flag=0`（§3.1） | 默认集合/单条查询追加 `delete_flag=0`，`enable_flag` 不进默认过滤 | 删一行后默认查询 → 结果集不含该行；仅显式带 `delete_flag` 谓词的查询能见到已删行；同一查询对 `enable_flag=0` 的未删行仍返回；契约 `DB_DEFAULT_SCOPE_EXCLUDES_DELETED` 绿 |
| F-6 写路径唯一（§3.1/§5.2） | INSERT/UPDATE 只在 `base`；`base` 仅 crate 内可见 | 契约 `DB_WRITE_PATH_CENTRALIZED` 绿；`base` 模块不导出为跨 crate 公开接口（**审查**） |
| F-7 后端分页（§3.1） | 集合查询接收 `PageQuery`，SQL `LIMIT ? OFFSET ?`，返回 `Page<T>`，页大小 `clamp` | 集合查询函数签名含 `PageQuery` 入参、返回 `Page<T>`（**审查**）；传 `page_size=201` → 实际 `LIMIT==200`（`clamp(201)==200`）；契约 `DB_PAGINATION_MANDATORY` 绿 |
| F-8 限制性表禁非 1 `enable_flag`（§3.1/§3.2） | `grant_constraints/grant_conditions/mode_state/deny_notes` 拒 `enable_flag≠1` | 向上述任一表写 `enable_flag=0` → `base` 写校验返回错误且建表 `CHECK(enable_flag=1)` 兜底拒绝，库不变 |
| F-9 固定宽度时间戳（§3.1） | `base` 是 policy.db 时间列与审计 `ts` 唯一格式化点，恒 `YYYY-MM-DDTHH:MM:SS.sssZ` | `base` 生成的任一时间文本：恒 UTC、恒 `Z` 结尾、恒 3 位毫秒、`len==24`；时间列带 `CHECK(length(col)=24)`；任意两时间文本字典序排序结果 == 时间先后排序结果 |
| F-10 归一化入库（§3.2） | `principals.name`/`roles.name`/`resources.codename` 入库前 `trim`+明示大小写策略，唯一索引作用于归一化值 | 以 `Admin`/` admin `/`ADMIN` 写 `roles.name` → 入库为归一化值、`CHECK(lower(trim(name))<>'admin')` 拒绝；先后写归一化后相同的两条名 → 第二条被 partial unique 索引拒；契约 `SEC_ADMIN_NOT_GRANTABLE` 绿 |
| F-11 schema 8 基础字段 + 约束（§3.2） | 每业务表声明全 8 基础字段；FK on、WAL；partial unique；`mode_state` 全局单行哨兵 | 契约 `DB_BASE_FIELDS_REQUIRED` 绿；每张业务表唯一索引为 partial unique（`WHERE delete_flag=0`）、逻辑删后同名可重建（**审查/集成**）；`mode_state` 唯一索引作用于 `COALESCE(scope_resource_id,0)` → 同辖区第二行全局 mode 被唯一索引拒 |
| F-12 PolicySnapshot 构建与内容清单（§3.4） | 一次事务内全量加载并展开，产 `Arc<PolicySnapshot>`；内容含/不含见 §3.4 | 授予性表按 `delete_flag=0 AND enable_flag=1`、限制性表仅 `delete_flag=0` 加载；快照含展开授权空间/凭证元数据(含 `secret_hash`)/tier/约束/条件/各辖区 mode/deny_notes/approval；快照类型中**不含**任何 vault 字段（资源凭据、真实地址映射）（**审查**）；快照内 `expires_at` 等为原值、无过期裁决字段 |
| F-13 JsonlAuditSink 按日轮转 + 分页扫描（§3.5） | 按 UTC 日轮转 append-only 写；`scan` 倒序分页 | `record(event)` → 落 `<data_dir>/audit/YYYY-MM-DD.jsonl`（UTC 日界）、文件物理只追加；事件 `id` 为雪花字符串；`scan(filter,page)` → 按日期文件倒序、分页窗口截断返回 `Page<AuditEvent>`、不全量读入内存；契约 `DB_PAGINATION_MANDATORY` 对 `scan` 绿 |
| F-14 审计 fsync 策略落地（§3.5） | `deny`/`policy_change`/`credential_event` 逐事件 fsync；`allow` 默认逐事件 fsync，`settings: audit.fsync=relaxed` 时改 1s 周期批量 | 写一条 `deny`/`policy_change`/`credential_event` → 该事件在 `record` 返回前已 fsync 落盘（**审查/集成**）；`audit.fsync` 缺省 → `allow` 类逐事件 fsync；置 `audit.fsync=relaxed` → `allow` 类按 1s 周期批量 fsync，而 `deny`/`policy_change`/`credential_event` 仍逐事件 fsync（不受 relaxed 影响）（**审查/集成**） |
| F-15 schema 迁移（§3.2/§5.2） | `PRAGMA user_version` 标识版本，前向迁移在事务内；不识别版本 fail-closed | `migrate` 入口存在并由 boot 调用（**审查**）；对当前实现可识别版本 → 迁移在单事务内完成、版本号前进；对不识别版本 → 返回错误拒绝、不按旧假设解析（库不变） |

### 二、逻辑正确性（判断：关键逻辑、边界、全部关键 fail-closed 失败路径对不对）

| 编号 | 要求（行为必须正确） | 通过判定 |
|---|---|---|
| L-1 逻辑删除后默认不返回（§3.1） | 删后默认查询看不到该行 | 删一行后，不带 `delete_flag` 谓词的默认集合/单条查询 → 该行**不在**结果集；带显式 `delete_flag=1` 谓词的查询能取到该行 |
| L-2 限制性表禁 `enable_flag`（§7-10） | 停用限制只能显式删除或 `mode set`，绝非 flag 翻转 | 对 `mode_state` 写 `enable_flag=0` → 写入被拒、库不变；解除冻结改走 `mode set normal` 时落 `mode_change` 审计；场景 docs/examples/06 §4.2#8 预期"写入被拒"复现 |
| L-2b 限制性表快照不过滤 `enable_flag`（§7-11） | 快照对限制性表仅按 `delete_flag=0` 加载，绝不引入 `enable_flag` 过滤（否则构成解冻/解约 fail-open） | 构造一条 `enable_flag=0`（或任意非 1 值）的 `grant_constraints`/`mode_state`/`grant_conditions`/`deny_notes` 行（仅供反例，绕开 §3.1 写校验直插库）→ 重建后的快照**仍包含**该限制行（限制照常生效）；`snapshot` 加载该四表的 SQL 谓词中**不含** `enable_flag` 条件（**审查**） |
| L-3 级联逻辑删除（§3.1/§3.2） | 父删则子行同事务级联删 | 删 `principals#p` → `credentials/bindings/temp_grants` 对应子行 `delete_flag==1`、`updated_by` 含 `cascade:principals#<id>`；事务任一步失败 ROLLBACK → 父子均保持 `delete_flag==0` |
| L-4 乐观锁冲突 409（§3.1/§6.4） | 期望 version 不匹配即冲突、不静默重试 | 持过期 `version` 更新同行 → UPDATE 影响 0 行 → 返回版本冲突错误、库不变、无重试；控制面把它映射为 `409 Conflict` 并写 `policy_change` 审计；场景 docs/examples/06 §4.2#10、docs/examples/03 §4.2 F、docs/examples/02 §4.2 E7 复现 |
| L-5 审计写失败 = deny（只读单次）（§6.3/§7-16） | 只读动词审计 `record` 失败 → 该请求 deny；本域只如实返回成败 | 注入 `record` 返回 `Err(AuditError)` → 本域如实返回 `Err`、**不**自行放行/拒绝（处置在内核：内核据此 deny 该只读请求）；场景 docs/examples/07 §4.2 E1 复现 |
| L-6 审计写失败两阶段（有副作用）（§6.3） | 有副作用动词：intent 写失败 → 执行前 deny；outcome 写失败 → 返"已执行但审计降级"，绝不谎报 deny | 步骤[7a] intent `record` 失败 → 本域如实返 `Err`（内核据此执行前 deny，确未执行）；步骤[10] outcome `record` 失败 → 本域如实返 `Err`（内核返"已执行但审计降级"、不返 deny）；场景 docs/examples/07 §4.2 E2 复现 |
| L-7 固定宽度时间戳可比（§3.1/§7-12） | 文本字典序 == 时间序，保证 TTL/sweeper `< now` 不错序 | 取两个跨毫秒/跨秒/跨日的 `base` 时间文本，其字符串比较结果与真实时间先后一致；长度恒 24（任一不等长即不过） |
| L-8 快照原子重建（§3.4/§6.4） | 重建与事务写在同一写锁临界区，Arc 原子替换，无双源、无半截 | COMMIT 后在同一写锁内重建快照（**审查**：重建调用处于写锁临界区）；并发读 `PolicyView::snapshot` 只会拿到"重建前完整旧快照"或"重建后完整新快照"，不存在读到半截状态；场景 docs/examples/03 §4.1 步骤 7 复现 |
| L-9 悬挂引用不放行（§3.4/§7-14） | 引用链父行不可见 ⇒ 子行不入快照，即便级联遗漏 | 构造父行 `delete_flag=1`（或不存在）而子行残留 → 重建后的快照**不含**该子行；`postern grants` 投影中该资源完全不出现、不暴露其曾存在；场景 docs/examples/03 §4.2 G、docs/examples/05 §4.2 J 复现 |
| L-10 多生效模式取最严格（§3.4/§7-14） | 同辖区多生效模式 → 取最严格并写告警，绝不取最宽松 | 构造同辖区出现 `freeze` 与 `normal` 两生效模式 → 快照取 `freeze`（序 `freeze>maintain>observe>normal`）并落一条告警审计；绝不取 `normal`；场景 docs/examples/06 §4.2#7 复现 |
| L-11 TTL 不进快照、不随墙钟推进（§3.4/§4） | 快照只是原子投影，过期判定归引擎按 `now` 二次校验 | 快照内 `temp_grants.expires_at`/`credentials.expires_at`/`revoked_at`/`mode_state.expires_at` 为原值且无任何过期裁决；同一快照不因时间流逝而内容变化（无 TTL 终判字段）；场景 docs/examples/06 §4.2#1（即刻失效判定在引擎、本域只交付原值）复现 |
| L-12 启动不可建即拒绝（§6.2） | 开库/迁移/首个快照/审计目录任一不可得 → fail-closed 拒绝启动 | 分别注入：开库失败 / schema 版本不识别 / 首个快照构建失败 / 审计目录不可写 → boot **拒绝进入服务状态**，绝不以空快照或降级审计载体开放数据面；场景 docs/examples/05 §4.2 J（快照不可得 → boot fail-closed 拒绝启动）复现 |
| L-13 事务失败即回滚（§6.4） | 三联动任一步失败 → ROLLBACK，库不变、快照不重建 | 注入"事务+快照重建+审计"三联动任一步失败 → 权威库无变更、快照不重建（无半截/无未审计中间态）；import 协调整体拒绝（无部分 apply）；场景 docs/examples/06 §4.2#14（解冻写失败 → 状态保持更严格侧）与该场景一致性声明复现 |
| L-14 审计 DoS 可控降级（§3.5/§6.3） | 逼近配额 → 可感知降级而非全平面瘫痪 | 构造逼近独立配额水位 → 触发告警事件 / 强制轮转 / 低价值降采样、且高价值（deny/policy_change/credential_event）事件不丢；deny 类按窗口聚合写带 `count` 记录；`audit.retention_days` 到期整文件删除（唯一允许的删除形态）；不出现"Agent 刷满磁盘 → 全平面瘫痪"；场景 docs/examples/07 §4.2 E3 复现 |
| L-15 审计 append-only 不含机密（§7-16） | 无行级修改/物理删除（除整文件保留期删除）；事件永不含凭据值/真实地址 | 任一已落审计文件只见追加、无行级原地改写；唯一删除形态是保留期到期整文件删除；任一事件字段无明文真实地址/凭据值（写前已过内核出口 Sanitizer，本域只落已脱敏事实）；`postern verify` 九项中第 8 项（脱敏探测）放行事件无敏感回显、可在审计复核；场景 docs/examples/07 §4.2 E4、§4.1-C 复现 |

### 三、边界与不变量（机器强制，绿/红即答案）

| 编号 | 要求 | 通过判定（机器；标「审查」者为构造签名/人工审查） |
|---|---|---|
| B-1 8 基础字段齐备（§7-6） | 每业务表声明全 8 基础字段 | 契约 `DB_BASE_FIELDS_REQUIRED`（+ `_TEETH`）绿 |
| B-2 写路径唯一（§7-1） | INSERT/UPDATE 只在 `base` | 契约 `DB_WRITE_PATH_CENTRALIZED`（+ `_TEETH`）绿 |
| B-3 仅逻辑删除（§7-4） | 工作区无任何针对 policy.db 的 `DELETE`（连 base 内部不豁免） | 契约 `DB_LOGICAL_DELETE_ONLY`（+ `_TEETH`）绿 |
| B-4 默认作用域排除已删（§7-5） | 默认 SELECT 追加 `delete_flag=0` | 契约 `DB_DEFAULT_SCOPE_EXCLUDES_DELETED`（+ `_TEETH`）绿 |
| B-5 后端分页强制（§7-7） | 集合查询/审计扫描均 `LIMIT` 封顶并返回 `Page<T>` | 契约 `DB_PAGINATION_MANDATORY`（+ `_TEETH`）绿 |
| B-6 裸 SQL 不出本 crate（§7-8） | SQL 字符串与 `rusqlite`/`sqlparser` 只在 `postern-store`；例外经 `contract/sql-exceptions.json` 登记（初始空） | 契约 `DB_NO_RAW_SQL_OUTSIDE_STORE`（+ `_TEETH`）绿 |
| B-7 统一雪花 id（§7-9） | 主键/审计 id 取自 `core::IdGen`，无替代 id 库 | 契约 `DB_UNIFIED_ID_GENERATOR`（+ `_TEETH`）绿；`cargo deny` 的 `[bans]` 命中 uuid/ulid/nanoid 为 0 |
| B-8 禁 admin（§3.2） | `roles` 表带禁 admin 名 CHECK `lower(trim(name))<>'admin'`（配合 §3.1 归一化） | 契约 `SEC_ADMIN_NOT_GRANTABLE`（+ `_TEETH`）绿 |
| B-9 依赖图无禁止边（§4/§7-15/17/18） | store ↛ secrets；adapters/transports/cli ↛ store；store 不碰机密载体 | 契约 `ARCH_FORBIDDEN_EDGES`（+ `_TEETH`）绿；`cargo tree -p postern-store -e normal` 无 `postern-secrets`，且无 `adapters→store`/`transports→store`/`cli→store` 边 |
| B-10 本库不存机密（§4/§7-15） | policy.db 无明文真实地址/凭据值（`secret_hash` 单向哈希除外），机密仅 `vault://` 引用 | 契约 `ARCH_FORBIDDEN_EDGES` 绿（无 store→secrets 边）；schema 与落库扫描无明文地址/凭据列（**审查**）；场景 docs/examples/02 §4.1 步骤 2、§4.2 E4 复现 |
| B-11 数据面无写策略/机密句柄（§7-17） | 注入数据面 router 的句柄集仅 `PolicyView`(只读)+`AuditSink`，不含 `PolicyRepo`/vault | daemon 构造函数注入句柄集签名不含 `PolicyRepo`/vault 句柄（**审查**）；契约 `ARCH_FORBIDDEN_EDGES` 绿；场景 docs/examples/06 §4.2#11 复现 |
| B-12 PolicyRepo 仅控制面可达（§7-18） | 事务读写句柄不进数据面依赖集；本域不被 adapters/transports/cli 依赖 | 契约 `ARCH_FORBIDDEN_EDGES` 绿；`cargo tree` 无 adapters/transports/cli → store 边；daemon 注入约束（**审查**） |

### 通过定义（DoD）

`postern-store` **算完成** ⟺ 一、二、三三组**每一条都通过**。任一条不过 = 不通过，必须修。F 类（F-1～F-15）靠"给定输入看落库/快照/审计是否符合通过判定"（含标「**审查**」的构造签名/可见性核对）；L 类靠"触发某条件→行为恰为某可观察结果"（覆盖逻辑删后默认不返回、限制性表禁 enable_flag 写入、限制性表快照不过滤 enable_flag、级联删、乐观锁 409、审计写失败=deny 的只读单次与有副作用两阶段、固定宽度时间戳可比、快照原子重建、悬挂引用不放行、多模式取最严格、TTL 不进快照、启动/事务 fail-closed、DoS 降级、append-only 不含机密 等全部关键 fail-closed / fail-open 防护路径）；B 类靠"跑契约/`cargo tree`/`cargo deny` 看绿红"。本域不重复挂引擎侧 `postern verify` 九项判据，仅以 append-only 载体如实留痕九项探测（L-15，对应详设 6.7 第 8 项）作为相关验收落点。
