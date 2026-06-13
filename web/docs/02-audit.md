# 02 · 审计 Audit

> 控制台「观测」组的审计事件流页。本文在[设计基座](00-设计系统与信息架构.md)之上展开，复用其令牌、组件与统一交互模式，不重定义；对应场景 [07 · 审计、拒绝分析与红队自检](../../docs/examples/07-审计与红队自检.md)。**纯设计，不含实现代码。**

---

## 一、页面定位

**审计事件流的只读查询视图**——按主体/时间/事件类/决策筛选，倒序、强制分页地查"我的 Agent 做了什么、哪一步被拒、为什么"，并把两阶段（intent/outcome）写事件按同请求 id 配对折叠。

- **对应场景**：场景 07 §4.1-A（审计查询）、§4.2-E1/E2/E4/E5/E8（异常）。回答"我的 Agent 做了什么？"。
- **主要 API**：`GET /v1/audit?since&principal&kind&decision&page_no&page_size`（倒序、强制分页）。导出对应 CLI `--format jsonl`。
- **页面性质**：纯**控制面读**动作，无写操作（写在数据面执行副产物，本页只查）。Agent 物理不可达本端点（`control.sock=0600`）。

---

## 二、布局

列表页统一骨架（基座 §七）：标题 + 主操作（右上：导出）+ 筛选条 + DataTable（强制分页）+ 行展开。本页**无创建/编辑**，故无右上「新建」、无 FormDrawer。

```
┌─ AppShell ────────────────────────────────────────────────────────────────┐
│ [postern]   ◆freeze切换  ●mode徽章  ☀/☾  ●daemon健康灯           顶栏常驻 │
├───────────┬────────────────────────────────────────────────────────────────┤
│ 总览       │  审计 Audit                                  [⭳ 导出 JSONL ▾]   │ ← 标题 + 主操作
│ ─观测      │  事件流 · 倒序（ts 新→旧）· 强制分页（后端）                    │
│  ▸审计 ◄   │ ┌─ 筛选条 ───────────────────────────────────────────────────┐ │
│   拒绝分析  │ │ since[时间范围▾] principal[主体▾] kind[事件类▾]            │ │
│   红队自检  │ │ decision[ 全部 | ●allow | ●deny | escalate_denied ]        │ │
│ ─授权      │ │                                  [应用]  [清空]  共 N 条     │ │
│  授权矩阵   │ └─────────────────────────────────────────────────────────────┘ │
│   角色      │ ┌─ DataTable ─────────────────────────────────────────────────┐ │
│   绑定      │ │ ⌄ ts↓     kind    principal  resource    cap    decision    │ │ ← 列头(ts 默认排序↓)
│   细则      │ ├─────────────────────────────────────────────────────────────┤ │
│   条件      │ │ ⌄ 06-13   request agent3     svc-order   mutate ◼allow op   │ │
│   拒绝指引  │ │ │ 14:22:07  ⟂ 两阶段配对（intent + outcome · 同 req id）  │ │ ← 行展开(见§四)
│ ─接入      │ │ ├ id 7300…0123  policy_rev 41  duration 38ms  ⧉            │ │
│   资源      │ │ │ objects [route:/api/orders]  response_digest sha256:9f… │ │
│   主体      │ │ └─────────────────────────────────────────────────────────  │ │
│   凭证      │ │ ⌄ 06-13   request agent1     db-main     mutate ◼deny rbac  │ │
│ ─系统      │ │ │ stage ◆rbac  reason: role=observer 不含 db-main:mutate   │ │ ← deny 展开
│   模式      │ │ ├ intent_digest sha256:3c…(脱敏)  policy_rev 41  ⧉        │ │
│   审批队列  │ │ ⌄ 06-13   request agent1     svc-crm     query  ◼deny auth  │ │
│   设置      │ │ ⌄ 06-13   policy_change —     db-main     —      ◼allow —    │ │
│   导入导出  │ │ ⌄ 06-13   lifecycle —         —          —      —           │ │
│   健康      │ │ … （每页 page_size 行）                                      │ │
│           │ ├─────────────────────────────────────────────────────────────┤ │
│           │ │  每页[20▾]  ‹ 第 page_no 页 / 共 ⌈total/size⌉ ›   total=N    │ │ ← 强制分页条
│           │ └─────────────────────────────────────────────────────────────┘ │
└───────────┴────────────────────────────────────────────────────────────────┘
```

布局要点：
- **筛选条**与表格分离、常驻顶部；`/` 聚焦筛选（基座键盘约定）。decision 用分段单选（含 `escalate_denied`，与 `deny` 可区分）。
- **行折叠态**只显高密度摘要列；**展开态**才显全信封字段与两阶段配对，避免一屏信息过载（基座原则五密度优先）。
- **导出**为主操作按钮（`▾` 下拉：导出当前筛选结果为 JSONL），不在行级。
- 分页条**常驻表底**，即便空态也显（强制分页是契约，不是可选）。

---

## 三、数据与状态

### 3.1 展示字段（审计信封，detailed design 5.3）

每行折叠态显高密度摘要；展开态显全字段。字段一律取自后端信封，前端不二次加工、不编造。

| 字段 | 折叠列 | 展开 | 呈现纪律 |
|---|---|---|---|
| `ts` | ● | ● | 本地时区 + 悬浮显 UTC 原值；列默认倒序排序锚 |
| `id` | | ● | 雪花 id **字符串**，等宽截断 `7300…0123` + 悬浮全展 + ⧉ 复制（基座 §3.4，绝不当 number） |
| `kind` | ● | ● | `request`/`policy_change`/`credential_event`/`lifecycle`/`connection_event`/`alert` 等，事件类徽章 |
| `entry` | | ● | `mcp`/`http`，shell 入口 |
| `principal` + `principal_id` | ●(name) | ● | 名字 + 截断雪花 id；`None` 时显 `—`（step [1] 前 deny 无主体） |
| `credential_id` | | ● | 截断雪花 id；**绝不**显 `secret_hash`/明文（基座原则六） |
| `resource` + `resource_id` | ●(code) | ● | **ResourceCodeBadge**：资源代号 + adapter/transport 小图标；**永不显真实地址** |
| `capability` | ● | ● | **CapabilityBadge**：observe/query/mutate/execute/manage/destroy，色温递增；`None` 显 `—`（classify deny 无能力） |
| `objects` | | ● | `ObjectRef` 字符串数组（如 `route:/api/orders`、`container:app-order`），等宽、匿名化代号 |
| `decision` | ● | ● | **DecisionBadge**：`allow`/`deny`/`escalate_denied` 固定语义色 |
| `stage` | ●(deny时) | ● | **StageChip**：仅 deny 有；auth/classify/rbac/constraint/condition/tier/transport/exec/audit/discover；allow 显 `—` |
| `reason` | | ● | deny 引用策略事实原文（如 `role=observer 不含 db-main:mutate`）；allow 为空 |
| `policy_rev` | | ● | 决策时策略修订号，对账锚点 |
| `intent_digest` | | ●(deny) | **脱敏后**摘要 sha256，等宽 + ⧉；供归类，不可逆 |
| `tier` | ●(allow时小字) | ● | allow 选中的凭证档（如 `op`/`ro`）；CredentialTier 名 |
| `duration_ms` | | ●(outcome) | 仅有副作用动词 outcome 有 |
| `response_digest` | | ●(allow/outcome) | sha256 摘要，**结构上不含响应内容**，等宽 + ⧉；intent 事件无此字段 |

> **结构性脱敏（场景 E4）**：以上字段中**结构上不存在** token/会话值/真实地址/账号密码——审计正文写入前过同一 Sanitizer，`response_digest`/`intent_digest` 只存摘要，`connection_event` 只记 resource/tier/transport 种类。前端只如实渲染后端给的脱敏事实，不需也不应"还原"任何摘要。

### 3.2 三态（fail-closed）

| 状态 | 呈现 | fail-closed 纪律 |
|---|---|---|
| **加载中** | **LoadingSkeleton**：表格行骨架（保留列结构）+ 分页条占位 | 不显任何旧数据当新数据；不乐观渲染 |
| **错误** | **ErrorState**：红色，"审计查询失败"+ 后端错误码（如 401/403/500），主操作「重试」 | **绝不显伪数据/部分数据**；表区清空为错误态，不退化为"看似成功的空列表" |
| **空** | **EmptyState**：在当前筛选下无事件，提示"当前筛选无匹配事件"+「清空筛选」引导 | 空≠错；空态仍显分页条 `total=0`，明确是"真的没有"而非"加载失败" |

> 关键区分（基座原则一）：**错误态 ≠ 空态**。查询出错 fail-closed 呈现为错误（不显任何行），不得退化成空列表诱导管理员误以为"没有事件"。

---

## 四、交互流

本页**无写操作**——审计是只读查询 + 导出。统一写流程（表单→摘要→确认→刷新）在本页不适用；导出虽是动作，但不改服务端状态、不走乐观锁。

### 4.1 查询与筛选

| 用户动作 | 系统响应 | 预期结果 |
|---|---|---|
| 设 `since`（时间范围）/ `principal` / `kind` / `decision`，点「应用」 | 发 `GET /v1/audit?...&page_no=1&page_size=<当前>`，回 `page_no=1` | 表格刷新为筛选结果，倒序（ts 新→旧），分页条显新 `total`；翻页位置重置第 1 页 |
| 点「清空」 | 清空所有筛选参数，重查第 1 页 | 回到全量（仍分页）首页 |
| 点列头 `ts` | 切换排序方向（默认↓）；排序在**后端**生效（重新请求） | 不在前端切片/排序整页（契约 `DB_PAGINATION_MANDATORY`） |
| 翻页 `›` / 改「每页」 | 发新请求带新 `page_no`/`page_size`（缺省 20、>200 由后端钳到 200） | 回对应窗口；前端**不缓存/不切片全量**（场景 A-2） |

### 4.2 行展开与两阶段配对

| 用户动作 | 系统响应 | 预期结果 |
|---|---|---|
| 点击行 `⌄` 展开 | 展开该事件全信封字段（§3.1 展开列） | deny 行显 **StageChip + reason + intent_digest**；allow 行显 tier + objects + response_digest |
| 展开**有副作用动词**（mutate/execute/manage/destroy）的写事件 | **AuditEventRow** 把同请求 id 的 `intent` 与 `outcome` 两条**配对折叠**为一组呈现 | intent（执行前，无 `response_digest`）与 outcome（执行后，含 `response_digest`/`duration_ms`）以同一请求 id 对账可见（场景 A-3）；intent 缺 outcome 时显"仅发起痕迹"提示（场景 E2-a：执行前 deny / E2-b：outcome 写失败） |
| 复制雪花 id / 摘要 | ⧉ 一键复制完整字符串 | 复制全精度字符串，绝不丢精度（基座 §3.4） |

### 4.3 导出（唯一"动作"，非写操作）

| 用户动作 | 系统响应 | 预期结果 |
|---|---|---|
| 点「导出 JSONL」 | 以**当前筛选条件**导出（对应 CLI `--format jsonl`），逐行 JSON | 机器形态、雪花 id 字符串、内容**仍脱敏**、与人类渲染同源（场景 A-5）；导出不改服务端状态 |

### 4.4 危险动作清单

**本页无危险动作**——无授权/扩权/freeze/吊销/shutdown/import/删除（基座 §七危险动作清单逐项核对，本页均不涉及），故无 **ConfirmDialog**。审计页是观测台、纯读，破坏性操作不在此页。

---

## 五、复用的基座组件

| 组件 | 本页用途 |
|---|---|
| **AppShell** / **GlobalEmergencyBar** | 全局骨架与顶栏应急区（常驻，非本页特有） |
| **DataTable** | 审计事件密集表：`ts` 列排序、筛选条联动、**强制分页**（20/钳 200）、加载骨架、空态、行展开 |
| **AuditEventRow** | 行渲染主体：全信封字段；两阶段 intent/outcome 同请求 id **配对折叠**；等宽 id/摘要 |
| **DecisionBadge** | `decision` 列：allow/deny/escalate_denied 固定语义色；deny 可展开 stage+reason |
| **StageChip** | deny 展开态的 `stage`：auth…discover 固定色 + 顺序语义 |
| **CapabilityBadge** | `capability` 列：六动词色温递增，只读 |
| **ResourceCodeBadge** | `resource` 列：代号 + adapter/transport 图标，**永不显真实地址** |
| **TtlBadge** | （仅 `mode`/临时授权相关事件展开时）TTL 呈现，非本页主路径 |
| **JsonViewer** | 单事件「查看原始 JSON」展开（等宽、只读、可复制；已脱敏） |
| **LoadingSkeleton / ErrorState / EmptyState** | 三态（§3.2），fail-closed |

**本页特有构成（用基座组件拼，不新造令牌/原语）**：

- **AuditFilterBar**：`since`（时间范围选择）+ `principal`（下拉）+ `kind`（下拉）+ `decision`（分段单选，含 `escalate_denied`）+「应用/清空」+ `total` 计数。复用基座筛选控件样式与令牌。
- **TwoPhaseGroup**：在 **AuditEventRow** 内，把同请求 id 的 intent/outcome 折叠为一组的视觉容器（连接线 + intent→outcome 时序），用等宽 id 关联、`response_digest` 有无区分两相。
- **ExportMenu**：主操作下拉，把当前 AuditFilterBar 条件透传给导出（JSONL）。

---

## 六、正常与异常预期

### 6.1 正常（对照场景 §4.1-A）

| 步 | 操作 | 预期结果 |
|---|---|---|
| A-1 | 筛 `principal=agent3` + `since` + `decision=allow`，第 1 页 | 回 `Page<AuditEvent>` 信封，倒序，含 `total`/`page_no`/`page_size`；每行 `kind=request`、`decision=allow`、写动词项含 `tier=op`、`capability=mutate`、`objects=[route:/api/orders]`、`response_digest=sha256:…`（**不含响应内容**）；**无**真实地址/Cookie/账密 |
| A-2 | 翻到第 2 页 | 回下一窗口，`page_no=2`；前端不缓存全量、后端按窗口截断 |
| A-3 | 展开某 mutate 事件 | 同请求 id 的 `intent`（无 `response_digest`）与 `outcome`（含 `response_digest`/`duration_ms`）配对可对账——"发起即有痕、结果可追溯" |
| A-4 | 筛 `principal=agent1` + `decision=deny` + `since=昨天` | 每行含 **StageChip**（如 `rbac`）+ `reason`（如 `role=observer 不含 db-main:mutate`）+ `intent_digest` + `policy_rev`；deny 可完整还原"哪一步、为何拒" |
| A-5 | 导出 JSONL | 逐行 JSON，雪花 id 字符串，内容脱敏、与人类渲染同源 |

### 6.2 异常 / 边界（一律 fail-closed）

| 编号/类 | 触发 | 预期（fail-closed） |
|---|---|---|
| 加载失败 | `GET /v1/audit` 返回 5xx / 网络断 | **ErrorState** 红色 + 「重试」；**不显任何行、不显伪数据**；不退化为空列表（§3.2） |
| 权限/认证 | 401/403（控制面认证失败、非授权会话） | 错误态明示"无权访问审计"；**不泄露**任何事件内容；与"空结果"严格区分 |
| 空结果 | 当前筛选无匹配（场景如 agent2 当日无 deny） | **EmptyState** "当前筛选无匹配事件" + 「清空筛选」；分页条 `total=0`；空≠错 |
| 分页越界 | 请求 `page_no` 超出 `⌈total/size⌉` | 后端钳/回空窗口；前端显空态 + 复位至有效页，不报"崩溃" |
| 分页参数 | 用户/链接传 `page_size>200` 或 `page_no<1` | 后端钳到 `[1,200]`/`page_no≥1`（基座硬约束）；前端按回值显示实际生效页大小，不静默假装 |
| 两阶段不全（E2） | 写事件只有 `intent`、无 `outcome`（执行前 deny 或 outcome 写失败） | 配对组显"仅发起痕迹"——intent 在库留发起痕；**不**伪造 outcome、**不**谎报已完成（诚实，公理六） |
| 越权/数据缺失（E5） | 审计行引用的资源/主体在当前可见世界缺失 | 显匿名化代号占位、`—`；**不暴露** Scope 外资源/他人权限是否存在；不报"数据损坏" |
| 跨重启连续（E8） | daemon 重启后查重启前事件 | 审计 append-only 不丢历史，仍可查（含上一 `policy_rev`）；前端无特殊处理，照常分页查询 |
| 雪花 id | 任意 id 字段 | 全程**字符串**渲染、等宽截断、悬浮全展、⧉ 复制；**绝不**当 number 解析丢精度 |

> 所有不确定态（加载/错误/权限/缺失）默认呈现为**受限/拒绝态**，绝不乐观假装有数据（基座原则一）。

---

## 七、与后端契约对齐

| 契约点 | 本页落地 |
|---|---|
| **强制分页** `DB_PAGINATION_MANDATORY` | `page_no`/`page_size` 缺省 **20**、上限钳 **200**（`PageQuery::clamp`，`page_no≥1`、`page_size∈[1,200]`）；翻页/排序**均后端重查**，前端**绝不**取全量再切片/排序；分页条常驻 |
| **`Page<T>` 信封** | 渲染 `items`/`page_no`/`page_size`/`total`；`total` 驱动分页条总页数与空态判定 |
| **倒序** | `ts` 新→旧，后端默认序；前端列排序切换亦后端生效 |
| **雪花 id 字符串** | `id`/`principal_id`/`resource_id`/`credential_id`/摘要 一律字符串、等宽、绝不丢精度（基座 §3.4） |
| **decision 取值** | `allow` / `deny` / `escalate_denied`（escalate 折叠为带可区分文案的 deny，DecisionBadge 三色，筛选含此值） |
| **stage 闭集** | StageChip 仅渲染后端闭集：auth/classify/rbac/constraint/condition/tier/transport/exec/audit/discover；**不**编造阶段 |
| **deny 逐字段、不泄露存在性** | deny 行只显 `stage`+`reason`+`intent_digest`+`policy_rev`，资源/对象为匿名化代号；**不**暴露 Scope 外资源/他人权限存在性（场景 E5、技设 6.4） |
| **结构性脱敏** | 任何字段中**结构上无** secret_hash/凭据明文/真实地址/会话值；`response_digest`/`intent_digest` 仅 sha256 摘要、不含内容；前端只如实渲染脱敏事实（场景 E4） |
| **乐观锁 / 写端点** | **不适用**——本页纯读，无写、无 version、无 409（写并发冲突见 [03-denials.md] 触发的策略调整与 [05-grants.md] 的 elevate） |
| **错误 fail-closed** | 一切错误（5xx/超时/权限）呈现为 ErrorState，不显伪数据、不退化为空列表 |
