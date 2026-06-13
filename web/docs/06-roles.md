# 06 · 角色 Roles

> 本文是 postern 控制台「角色 Roles」页的详细布局与交互设计，在《00 设计系统与信息架构》基座之上展开：复用其令牌、核心组件与统一交互模式，不重定义、不偏离。对应场景《docs/examples/03-权限分配与角色管理.md》。**纯设计，不含实现代码。**

---

## 一、页面定位

**一句话**：角色 Roles 是「信任等级（动词集）」的规则编辑器——可视化标准角色阶梯 `observer ⊂ operator ⊂ maintainer`（经显式继承展开）与不在阶梯上的窄角色（如 `log-observer={observe}`），并供运维新建/编辑角色的动词集与继承边；`admin` 在本页**无任何入口**（创建/编辑/继承均不提供），与模型层硬约束 `SEC_ADMIN_NOT_GRANTABLE` 一致。

- **对应场景**：场景 3《权限分配与角色管理》§3.2「角色阶梯页」、§4.1 步骤 1（声明角色阶梯 + 窄角色）、§4.2-A（试图授予 admin 的模型层硬拒）、§4.2-H（动词非法）。
- **主要 API**：`GET /v1/roles`（列表 + 有效动词集展开）、`POST /v1/roles`（创建/编辑/删除角色，写事务 + 快照重建 + 审计三联动）。
- **不在本页**：把角色挂到 Principal（role × scope binding）属于《07 绑定 Bindings》；展开后的 (资源×动词) 矩阵属于《05 授权矩阵 Grants》。本页只编辑「角色 = 动词集 + 继承边」这一资源无关的纯定义。

> **心智锚点**：角色就是动词集（推荐阶梯非强制，窄角色与标准阶梯可并存）。本页是「修订预设」，不是「批准请求」。SPA 零安全逻辑——所有动词集合法性、无环校验、admin 硬拒、继承展开都在 daemon 完成，前端只渲染 daemon 回报的事实。

---

## 二、布局

遵循基座「列表页统一骨架」：标题 + 主操作（右上）+ 筛选条 + DataTable（强制分页）+ 行操作 + 右侧 FormDrawer。本页 DataTable 行内嵌一列「有效动词集」徽章组（CapabilityBadge），并附一个**只读的继承阶梯视图**（基座组件拼装的本页特有 LadderGraph）置于表格上方，一眼可辨 `observer ⊂ operator ⊂ maintainer` 与游离的窄角色。

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ AppShell 顶栏: [postern]  [模式徽章 normal]  [Freeze ⏻]  [☀/☾]  [daemon ●健康] │
├────────────┬─────────────────────────────────────────────────────────────────┤
│ 导航        │  角色 Roles                                  [+ 新建角色]         │
│  观测       │  资源无关的信任等级（动词集）。admin 不可声明、不可授予。          │
│   审计      │ ┌─ 继承阶梯（只读 LadderGraph）─────────────────────────────────┐ │
│   拒绝分析   │ │  observer ─inherits→ operator ─inherits→ maintainer           │ │
│   红队自检   │ │  {observe,query}     ⊕{mutate,execute}   ⊕{manage}           │ │
│  授权       │ │                                                               │ │
│ ▸ 角色      │ │  游离窄角色：  log-observer {observe}                          │ │
│   绑定      │ │  · destroy 不进任何角色（单格+TTL，见 授权矩阵）              │ │
│   细则      │ └───────────────────────────────────────────────────────────────┘ │
│  接入       │ ┌─ 筛选条 ──────────────────────────────────────────────────────┐ │
│   资源      │ │ [🔍 名称…(/聚焦)] [动词▾ observe…destroy] [类型▾ 阶梯/窄角色]  │ │
│   主体      │ └───────────────────────────────────────────────────────────────┘ │
│   凭证      │ ┌─ DataTable（强制分页 page_no/page_size，缺省20钳200）─────────┐ │
│  系统       │ │ 名称        有效动词集                继承自      ver   操作    │ │
│   模式      │ │ observer    [observe][query]         —          0    [⋯]    │ │
│   …         │ │ operator    [observe][query]                                  │ │
│             │ │             [mutate][execute]        observer    0    [⋯]    │ │
│             │ │ maintainer  [observe][query][mutate]                          │ │
│             │ │             [execute][manage]        operator    0    [⋯]    │ │
│             │ │ log-observer[observe]                —          0    [⋯]    │ │
│             │ │ ─────────────────────────────────────────────────────────── │ │
│             │ │ 行操作[⋯]: 编辑动词集 / 编辑继承 / 删除（逻辑删除·危险）     │ │
│             │ └───────────────────────────────────────────────────────────────┘ │
│             │  共 4 条 · 第 1/1 页 · [page_size 20 ▾]      [◀ 上一页][下一页 ▶]│
└────────────┴─────────────────────────────────────────────────────────────────┘
        右侧 FormDrawer 位（创建/编辑时滑出，见 §四）↘
```

**新建/编辑角色 FormDrawer**（右侧滑出，基座 FormDrawer）：

```
┌─ 新建角色 ───────────────────────────────────────────┐
│ 名称  [______________]  唯一(delete_flag=0)·非空      │
│        ⚠ 输入 "admin"/"Admin"/" admin " → 前端即禁用   │
│           （便利提示；真正硬拒在 daemon）              │
│ 描述  [______________________________] (可选)         │
│ ┌─ 动词集（至少勾 1）──────────────────────────────┐  │
│ │ ☐ observe(蓝灰) ☐ query(蓝) ☐ mutate(琥珀)       │  │
│ │ ☐ execute(橙)   ☐ manage(紫)                     │  │
│ │ ☒ destroy(红) — 禁用·不可勾（destroy 不进角色）  │  │
│ │   每勾一项右侧标 action: ● allow ○ escalate       │  │
│ └──────────────────────────────────────────────────┘  │
│ ┌─ 继承自（可选·多选，仅已存在角色）───────────────┐  │
│ │ ☐ observer  ☐ operator  ☐ maintainer             │  │
│ │ （选中后下方实时预览「有效动词集 = 自身⊕继承」）  │  │
│ │ ⚠ 不可选会成环的父角色（daemon 校验无环）         │  │
│ └──────────────────────────────────────────────────┘  │
│ ─ 有效动词集预览（本地拼装·最终以 daemon 为准）──     │
│   {observe, query}                                    │
│ ───────────────────────────────────────────────────── │
│                          [取消]  [预览摘要 →]          │
└───────────────────────────────────────────────────────┘
```

---

## 三、数据与状态

### 3.1 展示字段（来源 `GET /v1/roles`，对齐 schema `roles`/`role_inherits`/`role_capabilities`）

| 字段 | 来源 | 呈现 |
|---|---|---|
| `id` | roles.id（雪花，字符串） | 等宽截断 `7300…0123` + 悬浮全展 + 复制；**绝不当 number** |
| `name` | roles.name | 正文；阶梯角色加阶梯序号微标 |
| 有效动词集 | role_capabilities + role_inherits **展开**（daemon 算） | CapabilityBadge 组（observe…manage 固定配色，色温递增）；每徽章带 action（allow/escalate）微角标 |
| 直接动词集 | role_capabilities（不含继承） | 编辑抽屉里区分「自身」与「继承得来」 |
| `继承自` | role_inherits.parent_role_id → 父 name | 链式徽章 `operator`；无则 `—` |
| `version` | roles.version | 等宽；编辑/删除时回传（乐观锁） |
| `created_by`/`updated_by`/`updated_at` | 审计四联字段 | 行展开/详情区显示（actor + 时间）|

> **有效动词集是 daemon 展开的事实**：前端在抽屉里做「自身⊕继承」的本地预览仅为输入辅助，标注「最终以 daemon 为准」；列表列的有效动词集**一律取 daemon 回报值**，不由前端推算（SPA 零安全逻辑）。`destroy` 与 `admin` 永不出现在任何角色的有效动词集中。

### 3.2 加载/错误/空三态（fail-closed）

- **加载**：DataTable 与 LadderGraph 区域显示 LoadingSkeleton（骨架行 + 占位徽章）。骨架期间「新建角色」按钮可点（创建不依赖列表数据），但行操作不可达。
- **错误**（`GET /v1/roles` 失败 / 桥不可达 / daemon 不健康）：整表替换为 ErrorState——**不显任何伪角色、不显缓存阶梯图**；文案为客观事实（如「无法加载角色：控制面不可达」），附「重试」。继承阶梯图同时进入错误态（不渲染半截阶梯，避免误导）。fail-closed：宁可空白，不显可能过期/错误的动词集。
- **空**（合法返回 0 条）：EmptyState，文案陈述事实「尚无任何角色」+ 主操作引导「新建角色」。**不**预填/建议「要不要一键创建标准阶梯」之类话术（真话且只说事实，不替系统编建议）。

---

## 四、交互流

所有写操作走基座**写操作统一流程**：表单（RHF+Zod）→ 提交前摘要预览 →（危险则）ConfirmDialog 确认 → 失效刷新 → 成功提示（policy_rev 前进 + 可跳 audit）/ 失败红色错误（不改视图）/ 409 冲突（提示刷新重试）。每个写端点 = 事务 COMMIT + 快照重建 + 审计三联动。

### 4.1 动作 → 系统响应 → 预期结果

| 用户动作 | 系统响应 | 预期结果 |
|---|---|---|
| 点「新建角色」 | 右侧 FormDrawer 滑出，空表单 | 见 §二抽屉；名称、动词集（≥1）、继承（可选）|
| 填名称 = `admin`/大小写/带空白变体 | 前端 Zod 即禁用提交并红字提示 | 便利拦截；即便绕过前端，daemon 仍硬拒（见 §六-A）|
| 勾动词、设每动词 action（allow/escalate）| 本地实时拼「有效动词集」预览 | `destroy` 复选框**禁用不可勾**（destroy 不进角色）|
| 选「继承自」父角色 | 预览区合并「自身⊕父有效集」 | 仅可选已存在角色；成环候选被 daemon 拒（前端置灰已知环）|
| 点「预览摘要 →」| FormDrawer 转摘要视图：将写入的 name/直接动词集/action/继承边、携带的 `version`（编辑时）| 运维确认「我要落库的是这些」|
| 摘要点「提交」（新建/编辑动词集）| `POST /v1/roles` → 事务 + 快照重建 + 审计 | 成功：toast「角色已保存，policy_rev 前进至 N」+「查看 audit」链接；抽屉关、列表失效刷新 |
| 行操作「编辑动词集 / 编辑继承」| 抽屉预填当前值 + 当前 `version` | 改完同上流程；提交携带读取时 `version` |
| 行操作「删除」 | **ConfirmDialog**（危险）→ `POST /v1/roles`（`delete_flag=1`，携 `version`）| 见 §4.2 危险动作 |

### 4.2 本页危险动作清单（一律 ConfirmDialog）

| 危险动作 | 为何危险 | 确认方式 |
|---|---|---|
| **删除角色**（逻辑删除 `delete_flag=1`）| 角色被删后，引用它的 binding 在快照构建时不再贡献授权——可能**收窄某些 Principal 的授权面**；逻辑删除是终态、不提供 undelete | ConfirmDialog：摘要列出「该角色名 + 有效动词集 + version」；提示「删除角色会影响引用它的绑定的授权展开（在『绑定/授权矩阵』核对）」；需**显式勾选「我已知晓影响」**后方可确认。文案只陈述事实，不替系统判断「是否安全」 |
| **编辑会缩小动词集 / 移除继承边**（扩权反向，即收权）| 减少有效动词集 = 收回某些资源格的授权 | 走标准摘要预览即可（收权方向非扩权，不强制二次确认）；摘要明确标「将移除动词 X / 继承边 Y」的 diff |

> 注：本页**不存在** freeze/shutdown/凭证吊销/import 覆盖/destroy 单格授予等动作（它们分属顶栏应急区与其它页）。角色编辑本身不直接扩权到某个 Principal（要等 binding），故除「删除」外不强制危险确认；但删除因其级联授权后果，按基座危险动作清单（「删除（逻辑删除）」）走 ConfirmDialog。

---

## 五、复用的基座组件

| 组件 | 本页用法 |
|---|---|
| **AppShell / GlobalEmergencyBar** | 全局骨架与顶栏应急区（freeze/模式/健康灯），本页不特殊化 |
| **DataTable** | 角色列表：名称/有效动词集/继承自/version/操作；强制分页（page_no/page_size 缺省 20 钳 200）；列排序（名称）、列筛选；雪花 id 列等宽截断；行操作菜单 |
| **CapabilityBadge** | 「有效动词集」「直接动词集」徽章组；固定配色（observe 蓝灰…manage 紫，色温递增）；只读；带 action（allow/escalate）微角标 |
| **FormDrawer** | 新建/编辑角色（名称 + 动词集复选 + 继承多选 + 摘要预览）；RHF+Zod；409 提示 |
| **ConfirmDialog** | 删除角色（显式勾选确认） |
| **EmptyState / ErrorState / LoadingSkeleton** | 三态，fail-closed（错误态不显伪角色/半截阶梯）|
| **AuditEventRow**（跳转目标）| 成功 toast 的「查看 audit」跳到对应 `policy_change` 事件 |

**本页特有组件（用基座组件拼）**：

- **LadderGraph（只读继承阶梯视图）**：用 CapabilityBadge + 连接线渲染 `observer ─inherits→ operator ─inherits→ maintainer` 的动词集递进（每段标 `⊕{新增动词}`），并把不在阶梯上的窄角色（无继承、单/少动词）列为「游离窄角色」。纯展示 daemon 回报的继承边与有效动词集，零计算逻辑。底部常驻一行事实注脚「destroy 不进任何角色——经单格 + TTL 在授权矩阵显式授予」。
- **CapabilityPicker（抽屉内）**：observe/query/mutate/execute/manage 五个复选 + 每项 action（allow/escalate）单选；`destroy` 复选框**渲染为禁用态并加红删除线**（在 UI 上表达「不可选」与模型层一致）；无 admin 任何控件。

---

## 六、正常与异常预期（对照场景规格）

### 6.1 正常操作（对照 §4.1 步骤 1）

- **声明阶梯**：依次创建 `observer`(observe,query)、`operator`(inherits observer; mutate,execute)、`maintainer`(inherits operator; manage)、`log-observer`(observe)。每次提交后预期：toast「policy_rev 前进」、列表失效刷新、新角色出现在 DataTable，且 LadderGraph 即时反映新继承段。
- **有效动词集展开正确**：列表中 `operator` 的有效动词集显示 {observe, query, mutate, execute}（含继承展开）、`maintainer` 显示 {observe, query, mutate, execute, manage}、`log-observer` 显示 {observe}（窄角色，无继承、不含 query）。这些值**取自 daemon**，与场景 §4.1 步骤 1 逐项一致。
- **新角色 version=0**：每条新建实体返回 `version=0`，列表 version 列显示 0；后续编辑回传该值。
- **审计可追溯**：每条写入产生一条 `policy_change`（actor=运维、policy_rev 递增、表名 + 行 id + 写后 version）；成功 toast 的「查看 audit」可跳到该事件。

### 6.2 异常与边界（一律 fail-closed）

| 场景 | 触发 | 本页预期（fail-closed）|
|---|---|---|
| **A · 授予/声明 admin（§4.2-A）** | 名称填 `admin`/`Admin`/` admin `（大小写/空白变体），或被诱导脚本/桥直传 | 前端：Zod 即禁用提交并提示「admin 不可作为可授予角色」（**便利层**）。即便绕过前端到 daemon：返回 `{error:{code,message}}`（如 422，message 为常量安全文案），**不写入任何行**；UI 原样转述该 message、不改本地视图、不静默成功。本页**结构上无创建 admin 的入口**（无该控件），与 `SEC_ADMIN_NOT_GRANTABLE` 一致 |
| **H · 动词非法（§4.2-H）** | 试图提交非 6 动词之一（如经桥直传 `frobnicate`）| UI 控件只暴露闭集 6 动词中可选的 5 个（destroy 禁用），结构上无法选出非法动词；若经非 UI 路径直传，daemon `role_capabilities` 的 6 动词 CHECK 拒绝→422，UI 原样呈现错误、不落库视图 |
| **成环继承** | 选父角色形成环（如 A inherits B 且 B inherits A）| 前端对已知环置灰候选；提交后 daemon 应用层无环校验拒绝→明确错误，UI 红色错误、不改视图 |
| **加载失败** | `GET /v1/roles` 失败 / 桥不可达 | ErrorState 替换全表 + 阶梯图；不显伪角色、不显缓存阶梯；附「重试」。fail-closed：不确定即受限态 |
| **空集** | 合法返回 0 条 | EmptyState + 「新建角色」引导；不编建议话术 |
| **分页越界** | page_no 超出总页数 | 依基座：钳制 page_size≤200、缺省 20；越界页返回空集呈 EmptyState（非错误），分页器回落到末页 |
| **F · 乐观锁 409（§4.2-F）** | 两个标签页/SPA 与 CLI 并发改同一角色（动词集或继承），第二个携过期 `version` 提交 | 第二个写入**不生效、不静默覆盖、不静默重试**；UI 原样呈现 409 并提示「他人已改，请重新读取最新 version 再改」；不改本地视图。运维点「刷新」重读最新 version 后再编辑 |
| **越权 / 数据缺失** | 控制面认证不足或行不可见 | fail-closed：对应行/操作不出现或返回受限态错误，UI 不臆造数据；删除已被他人逻辑删除的角色→按 409 或「行不可见」处理，提示刷新 |
| **删除影响** | 删除被 binding 引用的角色 | UI 不阻止删除（合法操作），但 ConfirmDialog 已提示「会影响引用它的绑定的授权展开」；删除后引用该角色的 binding 在快照中不再贡献授权（在《07 绑定》《05 授权矩阵》核对），UI 不替运维判断后果是否「安全」，只陈述事实 |

---

## 七、与后端契约对齐

- **分页**：`GET /v1/roles` 携 `page_no/page_size`，缺省 20、钳 200（对齐 `DB_PAGINATION_MANDATORY`）；DataTable 强制分页、不一次拉全量。
- **雪花 id 字符串**：`roles.id` 等机器 id 全程当字符串渲染（等宽、中段截断、悬浮全展、复制），**绝不**当 number 解析（>2^53 会丢精度）。
- **乐观锁 version**：编辑/删除（`POST /v1/roles`）携带读取时拿到的 `version`；不匹配→409，UI 提示刷新重试，期望 version 唯一来源是先前读取值（base 仓储不自读自比）。
- **逻辑删除 / enable_flag**：删除角色走 `delete_flag=1`（终态、无 undelete）；`roles` 唯一性按 partial unique（`WHERE delete_flag=0`），故删除后可重建同名角色再绑定。本页不提供 `enable_flag` 切换（与 schema 一致：限制性禁用语义不在角色表面向运维暴露）。
- **admin 硬约束**：`roles.name CHECK(lower(trim(name)) <> 'admin')` + `Capability` 枚举无 Admin 变体（双重硬约束，`SEC_ADMIN_NOT_GRANTABLE`）。前端无创建/授予 admin 的入口（结构性缺失），名称变体禁用为便利层，真正硬拒在 daemon——前端从不替 daemon 做安全决策。
- **6 动词闭集 + action**：动词集仅 {observe,query,mutate,execute,manage,destroy}，其中 `destroy` 不进角色（UI 禁用）；每动词 action ∈ {allow,escalate}（对齐 `role_capabilities.action` 与 `GrantAction`）。
- **错误信封**：所有写失败统一 `{error:{code,message}}`；UI 原样转述 `message`（常量安全文案），不加工、不编建议（真话且只说事实）。
- **写三联动可追溯**：每次成功写入提示 policy_rev 前进，并可跳转对应 `policy_change` 审计事件（可追溯原则）。
- **匿名化/脱敏**：本页不涉及资源真实地址/凭据/secret_hash；角色是资源无关的动词集定义，天然不含敏感载荷——仍遵守「永不显真实地址/明文/secret_hash」总纪律。
