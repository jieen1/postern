# 08 · 细则 / 条件 / 拒绝指引（Constraints · Conditions · Deny-notes）

> 本页是「授权」导航组下三类**作用面收窄器**的统一管理页：**细则**（constraint，收窄动词作用的对象面）、**条件**（condition，给动词附加求值谓词）、**拒绝指引**（deny-note，资源所有者亲笔预写、越权时原样转述给 Agent 的 `operator_note`）。设计基座见 `00-设计系统与信息架构.md`，本页只描述布局/状态/交互/预期，复用基座令牌与组件、遵守统一交互模式与后端契约硬约束，**不重定义、不偏离**。

---

## 一、页面定位

**一句话**：在「资源 × 动词」格上挂载三类限制性记录——细则收窄对象作用面、条件附加求值谓词、拒绝指引预写越权时回给 Agent 的人话——让运维把"动词能碰到什么、什么时候能碰、碰不到时人想对它说什么"无歧义地落成策略事实。

- **对应场景**：`docs/examples/03-权限分配与角色管理.md`（细则挂载收窄动词作用面，步骤 5 / 异常 E·细则交集合并）+ `docs/examples/05-越权拒绝与结构化分流.md`（deny-note 预写 → 越权时 `operator_note` 原样转述，步骤 0 / 异常 D；条件谓词 time_window 等见场景 6 引用）。
- **主要 API 端点**（控制面 router `CONTROL_ROUTES`，恰覆盖 §6.5）：
  - `GET /v1/constraints` · `POST /v1/constraints`
  - `GET /v1/conditions` · `POST /v1/conditions`
  - `GET /v1/deny-notes` · `POST /v1/deny-notes`
  - 旁路只读：`GET /v1/resources`（资源代号下拉、adapter 声明的细则 kind 矩阵）。
- **心智**：三类表全是**限制性表**（schema `CHECK(enable_flag=1)`）——记录只会**收紧**授权或**附加**人写文本，永不放宽。删除一条细则/条件 = **扩大动词作用面**（必须危险确认）。本页是"预设决定一切"的收窄层编辑器，不是审批台。

---

## 二、布局

三类记录共用一套「列表页统一骨架」（基座 §七），以**顶部分段切换**（SegmentedControl：细则 / 条件 / 拒绝指引）切换三张 DataTable，主操作 + 筛选条 + 表格随之切换，右侧共用一个 FormDrawer 位（按当前段渲染对应表单）。这样三类同构记录（都挂在 资源×动词 上、都是限制性表、都走同一写流程）复用同一骨架，而非三套页面。

```
┌─ AppShell ───────────────────────────────────────────────────────────────────┐
│ 顶栏：postern │ [模式徽章] [❄ Freeze] [健康灯] [主题]        全局应急区常驻      │
├──────────┬───────────────────────────────────────────────────────────────────┤
│ 导航      │  细则与条件                                  [＋ 新建细则]（主操作） │
│ ─观测     │  ┌─段切换──────────────────────────────────────────────────────┐  │
│  审计     │  │ ●细则 Constraints │ ○条件 Conditions │ ○拒绝指引 Deny-notes │  │
│ ─授权     │  └──────────────────────────────────────────────────────────────┘  │
│  授权矩阵  │  ┌─筛选条─────────────────────────────────────────────────────┐   │
│  角色     │  │ [资源 ▾] [动词 ▾] [kind ▾] [🔍 搜索 spec]   [□仅显非空作用面] │   │
│  绑定     │  └──────────────────────────────────────────────────────────────┘  │
│  细则 ◀━━ │  ┌─DataTable（强制分页 page_no/page_size 缺省20·钳200）────────┐   │
│  条件     │  │ 资源        动词     kind            spec 摘要       version ⋮ │   │
│  拒绝指引  │  │ docker-A   manage  container_prefix prefix=app-    3      ⋮ │   │
│  ─────    │  │ db-main    query   table_allow      3 tables…      0      ⋮ │   │
│ ─接入     │  │ db-main    query   column_mask      2 fields(PII)  1      ⋮ │   │
│  …        │  │ svc-order  mutate  http_route       2 routes…      0      ⋮ │   │
│ ─系统     │  │ …                                                            │   │
│  …        │  └──────────────────────────────────────────────────────────────┘  │
│          │              ◁ 1 2 3 … ▷    每页 [20 ▾]                            │
│          │                                          ┌─ FormDrawer（右侧抽屉）┐ │
│          │                                          │ 新建/编辑 细则          │ │
│          │                                          │ 资源 [docker-A ▾]       │ │
│          │                                          │ 动词 [manage ▾]         │ │
│          │                                          │ kind [container_prefix▾]│ │
│          │                                          │ spec  [prefix=app-    ] │ │
│          │                                          │ ───摘要预览─────────── │ │
│          │                                          │ [取消]      [提交]      │ │
│          │                                          └─────────────────────────┘ │
└──────────┴───────────────────────────────────────────────────────────────────┘
```

**段切换后的表头差异**（同骨架、不同列）：

- **细则 Constraints**：`资源 │ 动词 │ kind │ spec 摘要 │ version │ ⋮`
- **条件 Conditions**：`资源 │ 动词 │ predicate │ spec 摘要 │ 作用域 │ version │ ⋮`（资源/动词列可空——条件表 `resource_id` / `capability` 允许 NULL，表"全局/全动词作用域"，空值以 `*` 灰显）
- **拒绝指引 Deny-notes**：`资源 │ 动词 │ note 原文（截断+悬浮全展） │ version │ ⋮`（note 列等宽 `JetBrains Mono`，标"= 越权时 Agent 收到的 operator_note 原文"）

**行操作菜单（⋮）**：查看详情（spec/note 全文 JsonViewer 或纯文本）、编辑（带 version）、**删除**（`delete_flag=1`，危险确认——删除=扩大作用面）。限制性表**无"停用"项**（schema `CHECK(enable_flag=1)`，不存在 enable_flag=0 的软停用态，只有逻辑删除）。

---

## 三、数据与状态

### 3.1 各表展示字段（与 schema 对齐，永不显敏感物）

| 段 | 表 | 列字段 | 来源 / 约定 |
|---|---|---|---|
| 细则 | `grant_constraints` | resource(代号) · capability · `kind` · `spec` 摘要 · `version` | `ConstraintSpec{kind, spec}`；resource 用 `ResourceCodeBadge`（代号 + adapter/transport 图标，**永不显真实地址**）；spec 是 raw JSON 文本，列内摘要、详情 `JsonViewer` 只读 |
| 条件 | `grant_conditions` | resource(可空) · capability(可空) · `predicate` · `spec` 摘要 · 作用域 · `version` | `ConditionSpec{kind=predicate, spec}`；`resource_id`/`capability` 可 NULL → 作用域列显示"该资源该动词 / 全资源 / 全动词"；predicate ∈ {`rate_limit`,`time_window`,`mode`,`ttl`} |
| 拒绝指引 | `deny_notes` | resource · capability · `note` 原文 · `version` | `note` **原样**展示（公理六，不加工、不截断语义、仅视觉截断+悬浮全展）；标注"越权时即此文本回给 Agent" |

- **细则 kind**（adapter 声明支持哪些，本页 kind 下拉随所选资源的 adapter 动态收窄）：`table_allow` · `column_mask` · `container_prefix` · `http_route` · `command_template` · `command_class` · `key_prefix` · `mask_fields`。本页**不自行解释 spec 语义**——spec 是给 owning adapter 的 raw JSON，前端只做语法（JSON 可解析）的便利校验，授权语义裁决在 daemon（基座原则 8.12：SPA 零安全逻辑）。
- **条件 predicate**（内建谓词注册表）：`rate_limit`（限频）· `time_window`（时间窗）· `mode`（模式门）· `ttl`（有效期）。spec 为该谓词的 raw JSON。
- **id**：行 id 是雪花 id，全程**字符串**（基座 §3.4），等宽截断中段 + 悬浮全展 + 一键复制；详情抽屉显示完整 id。表格主显业务键（资源×动词×kind），id 收于详情/⋮。
- **绝不展示**：真实地址 / 凭据明文 / `secret_hash` —— 本页只涉及策略元数据，但 resource 一律走 `ResourceCodeBadge` 代号呈现（基座原则 6）。

### 3.2 加载 / 错误 / 空三态（fail-closed，基座原则 1）

- **加载**：`LoadingSkeleton` 占满表格行；段切换 / 翻页 / 筛选均触发加载态，绝不复用旧段数据冒充。
- **错误**（`GET` 失败、daemon 不可达、403）：`ErrorState`，**不显任何伪数据**——空表格 + 红色错误条 + 原始错误码/message（后端统一 `{error:{code,message}}` 原样呈现，不替系统编"建议重试"话术，基座原则 2）。fail-closed：拿不到列表即视为"看不见任何已挂载的收窄器"，不渲染过期缓存。
- **空**（该段无记录）：`EmptyState` + 主操作引导。文案据段而异且**只述事实**：
  - 细则空：「该范围尚无对象细则。**未挂细则的动词其作用面由角色与 tier 决定**，细则只会进一步收窄。」+ [新建细则]
  - 条件空：「尚无条件谓词。无条件即该动词不受 rate_limit/time_window/mode/ttl 约束。」+ [新建条件]
  - 拒绝指引空：「尚无拒绝指引。**无预写则越权响应不含 `operator_note` 字段**（网关不代为生成话术，公理六）。」+ [新建拒绝指引]
- **权限不足 / daemon health 异常**：主操作与行写操作按钮置灰 + tooltip 说明，绝不让写表单进入"看似可提交"的乐观态。

---

## 四、交互流

所有写操作走基座「写操作统一流程」：表单（RHF+Zod）→ **提交前摘要预览** →（危险则）确认 → 失效刷新 → 成功提示（policy_rev 前进 + 可跳 audit）/ 失败红色错误（不改视图）/ 409 冲突（提示刷新）。本页所有写都是 `POST`（创建/编辑/逻辑删除均经对应 `POST /v1/{constraints|conditions|deny-notes}`，携带 `version` 做乐观锁，删除以 `delete_flag=1` 表达）。

### 4.1 新建细则（FormDrawer，非危险——收窄只会更安全）

1. 点 [＋ 新建细则] → 右侧 FormDrawer。
2. 选 `资源`（`ResourceCodeBadge` 下拉，源 `GET /v1/resources`）→ `动词` → `kind`（**随资源的 adapter 声明动态收窄**，如 docker 资源只列 `container_prefix`，db 资源列 `table_allow`/`column_mask`）→ 填 `spec`（raw JSON，前端仅校验 JSON 可解析）。
3. **提交前摘要预览**：以人读句呈现"将给 `docker-A` 的 `manage` 挂 `container_prefix` 细则，作用面收窄到匹配 `prefix=app-` 的容器"。预览只陈述将写入的事实，**不预测求值结果**（语义在 adapter）。
4. 提交 `POST /v1/constraints` → 成功：Drawer 关、表格失效刷新（TanStack Query invalidate）、绿色提示「细则已挂载，policy_rev 前进至 N」+「查看 audit」跳 `GET /v1/audit?kind=policy_change`。
5. **同格同 kind 多行 = 交集合并提示**（场景 03·异常 E）：当目标 `(资源,动词,kind)` 已存在同 kind 记录，摘要预览额外提示「该格已有 N 条同 kind 细则，新增后按**交集**生效（更窄，fail-closed）」——这是事实告知，不是危险确认。

### 4.2 新建条件（FormDrawer，非危险）

- 选 `资源`(可留空=全资源) → `动词`(可留空=全动词) → `predicate`（`rate_limit`/`time_window`/`mode`/`ttl` 单选）→ `spec`（raw JSON）。
- 摘要预览：「将给 `(db-main, query)` 附加 `rate_limit` 条件」。留空作用域时明确提示"作用域=全资源/全动词，范围更广，请确认"。
- 提交 `POST /v1/conditions`，成功/失败/409 同 4.1。

### 4.3 新建/编辑拒绝指引（FormDrawer，**资源所有者亲笔**）

- 选 `资源` → `动词` → 填 `note`（多行纯文本，等宽预览）。Drawer 顶部常驻提示：「**此文本越权时将原样回给 Agent（operator_note），网关不加工。写你想让对方看到的人话。**」(公理六)。
- **唯一性约束**：`deny_notes` 有 `UNIQUE(resource_id, capability) WHERE delete_flag=0`——同一 `(资源,动词)` 至多一条生效。若已存在，FormDrawer 进入"编辑"语态（带该行 `version`），而非创建第二条；后端若并发触发唯一冲突，按 409 / 约束错误原样呈现。
- 提交 `POST /v1/deny-notes`，成功提示「拒绝指引已生效，此后对 `(docker-A, manage)` 的越权拒绝将附带此 note」。

### 4.4 编辑（三段通用）

- ⋮ → 编辑 → FormDrawer 预填**当前 version**（读取时拿到，参与乐观锁）。
- 改 spec/note → 摘要预览 diff（旧 → 新）→ 提交携带期望 `version`。
- 409：见 4.6。

### 4.5 删除 = 扩大作用面（**危险动作**，ConfirmDialog）

- ⋮ → 删除 → **ConfirmDialog**（基座危险动作清单含逻辑删除）。
- 文案直述后果，按段而异：
  - 删细则：「删除此细则将**放宽 `(docker-A, manage)` 的对象作用面**——该动词不再受 `container_prefix=app-` 限制。确认？」需显式勾选「我已知此操作扩大授权作用面」。
  - 删条件：「删除后 `(db-main, query)` 不再受 `rate_limit` 约束。」
  - 删拒绝指引：「删除后该 `(资源,动词)` 越权响应将**不再含 operator_note**（回到无人话状态）。」
- 确认 → `POST`（`delete_flag=1`）携带 `version` → 失效刷新 → 成功提示 policy_rev 前进 + 可跳 audit。失败不改视图。

### 4.6 乐观锁 409（编辑/删除均可能）

- 携带的期望 `version` 落后 → 后端 `409 Conflict` 并写 `policy_change` 审计（记录冲突）。
- UI 原样呈现 409，红色提示「他人已修改此记录，请刷新后基于最新 version 重试」，**不静默覆盖、不自动重试、不改本地视图**（基座原则 7 + 场景 03·异常 F）。提供 [刷新本行] 重新拉取最新 version。

### 4.7 本页危险动作清单（一律 ConfirmDialog + 显式勾选）

| 动作 | 段 | 危险性 | 确认方式 |
|---|---|---|---|
| 删除细则（`delete_flag=1`） | 细则 | 扩大动词对象作用面 | ConfirmDialog + 勾选「已知扩大作用面」 |
| 删除条件 | 条件 | 解除 rate_limit/time_window/mode/ttl 约束 | ConfirmDialog + 勾选 |
| 删除拒绝指引 | 拒绝指引 | 越权响应失去 operator_note | ConfirmDialog（不勾选，后果较轻但仍二次确认） |
| 新建/编辑条件且作用域留空 | 条件 | 作用域放大到全资源/全动词 | 摘要预览强提示 + 二次确认 |

> 新建/编辑细则与拒绝指引**本身不危险**（收窄 / 加人话只会更安全），走常规摘要预览即可，无 ConfirmDialog。

---

## 五、复用的基座组件

| 基座组件 | 本页用途 |
|---|---|
| **AppShell** | 全局骨架；本页挂「授权」导航组下 |
| **DataTable** | 三段共用：列排序、`资源/动词/kind` 筛选、**强制分页**（page_no/page_size 缺省20·钳200）、空/错/加载三态、行操作菜单；雪花 id 等宽截断 |
| **ResourceCodeBadge** | 资源列与表单下拉的资源代号（代号 + adapter/transport 图标，永不显真实地址） |
| **CapabilityBadge** | 动词列（observe…destroy 固定配色，只读） |
| **FormDrawer** | 三段共用的创建/编辑抽屉（RHF+Zod、摘要预览、409 提示、成功提示 policy_rev） |
| **ConfirmDialog** | 删除（扩大作用面）、条件作用域留空的二次确认 |
| **JsonViewer** | constraint/condition 的 `spec`（raw JSON）只读展示，等宽、可复制 |
| **EmptyState / ErrorState / LoadingSkeleton** | fail-closed 三态（错误不显伪数据） |
| **TtlBadge** | 仅当 `ttl` 谓词的 spec 含到期信息时，条件行内呈现倒计时（临近转琥珀） |

**本页特有的小构成（用基座组件拼，非新令牌）**：

- **SegmentedControl（段切换）**：细则 / 条件 / 拒绝指引三段切换，复用基座中性令牌；切换即换主操作 + 筛选条 + DataTable 列定义 + FormDrawer 表单，共享同一容器。
- **KindMatrixSelect（kind 动态下拉）**：细则 `kind` 下拉，依所选资源的 adapter 声明矩阵（源 `GET /v1/resources` 携带的 adapter 能力）动态收窄候选，避免给某资源选了它 adapter 不支持的 kind（前端便利，真正裁决在 daemon）。
- **IntersectionHint（交集提示）**：摘要预览内的事实提示行——同格同 kind 已有 N 条 → 标注"新增按交集生效（更窄）"，用基座 `--info` 令牌，仅陈述合并语义不预测结果。
- **VerbatimNote（原文块）**：拒绝指引 `note` 的等宽原文展示（列内 + 详情），强调"所见即 Agent 所得"，无任何富文本/Markdown 渲染（公理六：原样、不加工）。

---

## 六、正常与异常预期

> 一律对照场景规格；异常一律 fail-closed。

### 6.1 正常操作（对应场景 03 步骤 5 / 场景 05 步骤 0）

| 操作 | 预期结果 |
|---|---|
| 给 `docker-A:manage` 挂 `container_prefix=app-`（场景03·步5） | 行写入 `grant_constraints`，version=0；表格出现该行；policy_rev 前进；可跳 audit 见 `policy_change`。此后求值 [4] 对非 `app-` 容器 deny（前端不预测，仅陈述已写入事实） |
| 给 `db-main:query` 先后挂两条 `table_allow`（场景03·异常E） | 两行均写入；第二行摘要预览提示"按交集生效"；表格显示两行同 kind；有效白名单 = 两者交集（fail-closed，更窄） |
| 给 `svc-order:mutate` 挂 `http_route`（routes=POST:/api/orders…） | 行写入；spec 详情 `JsonViewer` 原样可读 |
| 为 `docker-A:manage` 预写 deny-note（场景05·步0） | `deny_notes` 写入并热生效；提示"此后对 `(docker-A,manage)` 越权拒绝附带此 note 原文"；与越权时 Agent 收到的 `operator_note` 逐字一致 |
| 给 `(db-main,query)` 附 `rate_limit` 条件 | `grant_conditions` 写入；条件段表格出现该行 |

### 6.2 异常与边界（fail-closed 预期）

| 场景 | 触发 | 预期（UI 行为） |
|---|---|---|
| **加载失败** | `GET /v1/constraints` 失败 / daemon 不可达 | `ErrorState`，空表 + 原始错误码，**不显伪数据**、不渲染过期缓存；写按钮置灰 |
| **权限不足** | 控制面认证未过（403） | 整页 fail-closed，列表与写操作均不可用，呈现 403 事实，不替系统编引导 |
| **空集** | 该段无记录 / 筛选无命中 | `EmptyState`（据段陈述"无即意味着什么"，见 3.2），不误显他段数据 |
| **分页越界** | 请求超出末页 / page_size>200 | 后端钳 200、缺省 20；UI 按返回的 page_no/page_size 渲染，越界页回退末页，绝不一次拉全量 |
| **乐观锁 409** | 编辑/删除携带过期 version（场景03·异常F） | `409` 原样提示"他人已改，请刷新重试"，不静默覆盖/重试/改视图；[刷新本行] 重取最新 version |
| **删除=扩权** | 删任一细则/条件/指引 | ConfirmDialog 直述"放宽作用面/解除约束/失去 operator_note"后果 + 显式勾选；确认后 `delete_flag=1` |
| **deny-note 唯一冲突** | 同 `(资源,动词)` 已有生效 note 再创建 | FormDrawer 转编辑语态；后端唯一约束触发则按 409/约束错误原样呈现，不创建第二条 |
| **spec 非法 JSON** | 细则/条件 spec 填了不可解析文本 | 前端语法层即时校验拦截（便利），不提交；**语义合法性仍由 daemon 裁决**（前端零安全逻辑），daemon 422 原样呈现、不落库 |
| **adapter 不支持的 kind** | 给某资源选了其 adapter 未声明的 kind | KindMatrixSelect 前端先收窄候选（便利）；若仍触达 daemon，daemon 拒绝、422 原样呈现，不落库 |
| **越权探测防护（旁证）** | — | 本页**不查询、不展示** Agent 的 `your_grants` 或 Scope 外资源是否存在；本页只编辑资源所有者自己资源上的收窄器，不构成拓扑探测面（场景05·异常A 的反面：deny 渲染在 03-denials 页，本页只写 note 源） |

---

## 七、与后端契约对齐

- **分页**：三个 `GET` 端点一律分页，`page_no`/`page_size`（缺省 20、钳 200，对齐 `DB_PAGINATION_MANDATORY`）；UI 按返回值渲染分页器，绝不全量拉取。
- **雪花 id**：行 id 全程**字符串**（`>2^53`），前端绝不当 number 解析；等宽截断 + 悬浮全展 + 复制。
- **限制性表（`CHECK(enable_flag=1)`）**：`grant_constraints` / `grant_conditions` / `deny_notes` 三表均**只可逻辑删除（`delete_flag=1`），无 enable_flag=0 软停用态**——故行操作菜单无"停用"项，"撤下一条收窄器"只能删除，且删除=扩大作用面（危险确认）。
- **乐观锁**：每条记录带 `version`；编辑/删除 `POST` 携带读取时拿到的期望 `version`，落后 → `409 Conflict`（后端同时写 `policy_change` 审计记录冲突）；UI 原样提示刷新重试，不自读自比、不静默覆盖（base 仓储不自读自比，期望 version 唯一来源是调用方先前读取值）。
- **写三联动**：每个写端点 = 事务 COMMIT + 快照重建（policy_rev 前进）+ `policy_change` 审计（L-14）；成功提示带 policy_rev 前进 + 可跳 `GET /v1/audit?kind=policy_change`。
- **deny-note 唯一性**：`UNIQUE(resource_id, capability) WHERE delete_flag=0`——同 `(资源,动词)` 至多一条生效；UI 以"编辑既有"而非"再建一条"承接。
- **operator_note 公理六**：`note` 原文即 `DenyResponse.operator_note`，**原样转述、网关不加工**；`operator_note` 在 `DenyResponse` 配 `skip_serializing_if = Option::is_none`——**无预写则该字段整体不出现**（本页"拒绝指引空"态文案据此措辞）。本页只写 note 源，越权时的逐字段 deny 渲染（含 `request_hint`/`your_grants` 的 scope-bounded 不泄露存在性）由 `DenyResponseView` 在拒绝分析/审计页承担。
- **资源脱敏**：resource 一律 `ResourceCodeBadge` 代号呈现，永不显真实地址/凭据/`secret_hash`（基座原则 6）。
- **错误形态**：后端统一 `{error:{code,message}}` 原样呈现，不替系统编建议话术（基座原则 2）。
- **条件作用域可空**：`grant_conditions` 的 `resource_id` / `capability` 允许 NULL（全资源 / 全动词），UI 以 `*` 灰显并在创建时对留空作用域强提示"范围更广"。
