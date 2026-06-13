# 01 · 总览 Dashboard

> 本文是 postern 控制台**总览 Dashboard** 页的详细布局与交互设计。遵循《00 设计系统与信息架构》的视觉令牌、核心组件库、统一交互模式与后端契约硬约束，不重定义、不偏离。纯设计，不含实现代码。

---

## 1. 页面定位

**一句话**：落地首页，把"daemon 是否健康 / 当前全局与各资源处于什么模式 / 最近谁反复被拒在哪一步 / 临时授权是否有即将到期 / 一键拉闸去哪儿按"压缩在一屏内，让管理员 5 秒内判断系统姿态、≤2 步触达应急动作。

**核心心智落地**：Dashboard 不做任何裁决，它只**观测 + 直达**——把"预设决定一切、拒绝引导一切"的客观事实摆出来（健康灯、模式徽章、deny 聚合榜），把"人只修订预设"的入口指向各编辑页。Dashboard 自身唯一的写动作是**全局 freeze**（应急闸），与顶栏 `GlobalEmergencyBar` 同源、同确认。

**对应场景**：全局（横跨 02~07 概览）——
- 02 资源接入：健康灯反映 daemon/审计存储可写态。
- 03/05 授权与拒绝：deny 聚合榜 = 拒绝分析页（03-denials）的浓缩入口。
- 06 动态治理：模式面板 + freeze 应急 + 临时授权（临近到期处理的 Grants 页跳转入口；跨主体枚举无现成端点，不在本页内联）。
- 07 审计自检：deny 榜每行可跳审计流（02-audit）；红队上次结果摘要卡可跳 Verify（04-verify）。

**主要 API 端点**（均为读，唯一写为 freeze）：
- `GET /v1/health` — daemon / 存储健康。
- `GET /v1/denials/summary?window=7d` — 近窗 deny 聚合（按 `(principal, resource, stage, capability)` 计数）；这是本页唯一**真正跨主体**的聚合读端点。
- `POST /v1/mode` 同源读 / `mode_state` 投影 — 全局模式 + 各资源辖区模式覆盖。控制面 router（`CONTROL_ROUTES`）只有 `POST /v1/mode` 写端点，**无** `GET /v1/mode`；ModePanel 与顶栏 `GlobalEmergencyBar` 的模式徽章**同源**，读自 `POST /v1/mode` 同辖区写的 `mode_state` 投影（与 11-mode.md §展示字段「均来自 `POST /v1/mode` 同源读 / `mode_state` 投影」一致），UI 不引用不存在的 `GET /v1/mode` 路由。
- 应急写：`POST /v1/mode`（freeze，全局 `resource=None`）——本页唯一写端点，复用顶栏 `ModeSelector` 的 freeze 高危确认流程。

> ExpiringGrants（右栏临时授权将到期提醒）**不**列入上方真实读端点：控制面 `CONTROL_ROUTES` 中 `GET /v1/grants` 是 **per-principal / scope-bounded** 投影（与 05-grants.md「某 Principal 的展开授权矩阵」、core `DenyResponse.your_grants = BTreeMap<ResourceCode, Vec<String>>` 单主体语义同源），**无法**枚举「跨所有主体的临近到期临时授权」；而 router 中也不存在跨主体临时授权聚合读端点。故 ExpiringGrants 收敛为**跳转入口卡**，不在 Dashboard 内联枚举跨主体临时授权（详见 §3.1 与 §7 契约对齐）。

---

## 2. 布局

遵循基座 AppShell（顶栏 `GlobalEmergencyBar` + 左导航 + 内容区）。Dashboard 内容区为**卡片网格**（非列表页骨架——它是观测面板，不是单实体表格），信息密度高但分四象限层级清晰：第一行健康与模式（系统姿态），第二行 deny 聚合榜（拒绝引导，占主视觉），右栏临时授权到期 + 红队摘要 + 快捷入口。

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ AppShell 顶栏：[postern]  ●健康灯  [模式:NORMAL▾]  [⚡FREEZE]  [☀/☾]            │  ← GlobalEmergencyBar 常驻
├───────────┬──────────────────────────────────────────────────────────────────┤
│ 导航       │  总览 Dashboard                              [刷新 ⟳] 最后更新 12:04:31 │
│ ▸总览(选)  │ ┌──────────────────────────┐ ┌──────────────────────────────────┐ │
│ ─观测      │ │ HealthCard               │ │ ModePanel  当前模式姿态           │ │
│  审计      │ │ ● daemon       UP        │ │ 全局: ●NORMAL                    │ │
│  拒绝分析  │ │ ● audit store  WRITABLE  │ │ 资源覆盖 (3):                     │ │
│  红队自检  │ │ ● policy_rev   #4821     │ │  db-main    ●OBSERVE  ⏱1h12m     │ │
│ ─授权      │ │ ● uptime       3d 04h    │ │  docker-A   ●MAINTAIN ⏱42m       │ │
│  授权矩阵  │ │ 容量 ▓▓▓▓▓░ 61%          │ │  svc-pay    ●FREEZE              │ │
│  角色      │ │              [详情 Health]│ │             [管理模式 → Mode]     │ │
│  绑定      │ └──────────────────────────┘ └──────────────────────────────────┘ │
│  细则      │ ┌────────────────────────────────────────────┐ ┌────────────────┐ │
│ ─接入      │ │ DenialsTopTable  最近高频拒绝  window:[7d▾]  │ │ ExpiringGrants │ │
│  资源      │ │ ┌──────┬──────┬──────┬──────┬─────┬───────┐ │ │ 临时授权将到期  │ │
│  主体      │ │ │主体  │资源  │动词  │阶段  │计数 │       │ │ │ 跨主体临时授权  │ │
│  凭证      │ │ ├──────┼──────┼──────┼──────┼─────┼───────┤ │ │ 的临近到期需在  │ │
│ ─系统      │ │ │agent3│db-mn │mutate│constr│  47 │ →audit│ │ │ Grants 页按主  │ │
│  模式      │ │ │agent1│svc-x │manage│ rbac │  31 │ →audit│ │ │ 体维度查看与    │ │
│  审批      │ │ │agent2│dckrA │execut│ mode │  18 │ →audit│ │ │ 处理（续期/吊销）│ │
│  设置      │ │ │…     │…     │…     │…     │ …   │       │ │ │                │ │
│  导入导出  │ │ └──────┴──────┴──────┴──────┴─────┴───────┘ │ │   [前往 Grants]│ │
│  健康      │ │ 仅聚合计数，方向由人裁决   [全部 → Denials]   │ ├────────────────┤ │
│           │ └────────────────────────────────────────────┘ │ VerifyCard     │ │
│           │                                                  │ 上次红队 12/06  │ │
│           │                                                  │ ▣ 8/9 PASS ✗1  │ │
│           │                                                  │   [运行→Verify]│ │
│           │                                                  └────────────────┘ │
└───────────┴──────────────────────────────────────────────────────────────────┘
   freeze 生效时整页顶部出现红色脉冲横幅（GlobalEmergencyBar），覆盖于内容区之上：
┌──────────────────────────────────────────────────────────────────────────────┐
│ ⚡ 系统已 FREEZE（全局）— 一切动作被拒，仅保留只读视图   [解除 Freeze → Mode]    │  ← --freeze 脉冲
└──────────────────────────────────────────────────────────────────────────────┘
```

区域职责：
- **HealthCard**（左上）：系统级硬指标，fail-closed 的第一信号。
- **ModePanel**（右上）：当前姿态——全局模式 + 各资源覆盖，TTL 倒计时。读自 `POST /v1/mode` 同源读 / `mode_state` 投影（与顶栏 `GlobalEmergencyBar` 模式徽章同源；router 无 `GET /v1/mode` 路由，不引用之）；不在此直接改模式（除 freeze 经顶栏），改模式跳 Mode 页。
- **DenialsTopTable**（主视觉，左下宽）：近窗 deny 聚合榜，本页最大信息块，体现"拒绝引导一切"；走真正跨主体的 `GET /v1/denials/summary` 聚合读。
- **ExpiringGrants + VerifyCard**（右栏窄）：临时授权处理的**跳转入口卡**（per-principal 的 `GET /v1/grants` 无法跨主体枚举临近到期，故本页不内联枚举，只引导去 Grants 页按主体看）+ 上次红队结果摘要。
- **顶栏 freeze**：应急闸常驻，与本页无重复 freeze 控件——本页不再放第二个 freeze 按钮，避免双入口语义漂移；freeze 横幅由 `GlobalEmergencyBar` 全局渲染。

---

## 3. 数据与状态

### 3.1 各卡片展示字段

| 卡片 | 来源 | 展示字段 | 脱敏纪律 |
|---|---|---|---|
| HealthCard | `GET /v1/health` | daemon 存活、audit store 可写/容量水位、当前 `policy_rev`、uptime | policy_rev 等宽字符串呈现（雪花纪律不适用 rev，但与机器事实同等宽体例）；不显路径/真实地址 |
| ModePanel | `POST /v1/mode` 同源读 / `mode_state` 投影（**非** `GET /v1/mode`——router 无此路由） | 全局 `Mode`（normal/observe/maintain/freeze）+ 各资源覆盖项 `(resource_code, mode, ttl 倒计时)` | 资源只显代号（ResourceCodeBadge）；mode 用 ModeSelector 配色徽章 |
| DenialsTopTable | `GET /v1/denials/summary?window` | 每行 `(principal, resource, capability, stage, count)`；按 count 降序 | principal 显名/代号、resource 显代号、capability 用 CapabilityBadge、stage 用 StageChip；**不显真实地址、不显 reason 明细**（明细在 Denials 页逐字段看） |
| ExpiringGrants | 无内联数据源（跳转入口卡） | 不枚举任何条目；只一句引导文案 + [前往 Grants] | 不渲染主体/资源/到期项——跨主体临近到期由 Grants 页按主体维度处理 |
| VerifyCard | 客户端缓存上次 `POST /v1/verify` 报告（本页**不主动触发** verify） | 上次运行时间、`all_pass`、`pass/total`、失败项数 | 仅摘要计数；逐条 gap_note 在 Verify 页看 |

> 说明（ExpiringGrants 为何不内联枚举）：控制面 `GET /v1/grants` 是 **per-principal / scope-bounded** 投影——返回**某一个** Principal 的展开授权矩阵 + 其临时授权清单（与 05-grants.md「principal 维度」、core `DenyResponse.your_grants = BTreeMap<ResourceCode, Vec<String>>` 单主体语义同源），**不**是「跨所有主体的授权全集」。一个单主体、scope-bounded 的端点无法枚举 Dashboard 需要的「跨主体临近到期临时授权」，而 router 中也不存在跨主体临时授权聚合读端点（区别于 `GET /v1/denials/summary` 这一真正的跨主体聚合）。因此 Dashboard **不**复用 `GET /v1/grants` 伪造跨主体枚举，ExpiringGrants 收敛为跳转入口卡：临近到期的临时授权统一在 Grants 页**按主体维度**查看与处理（续期/吊销）。临时格的「当前生效 + 临近到期」判定（`expires_at >= now 且 ended_at 为空`、剩余时间 `< 阈值`）发生在 Grants 页。
>
> 注：若后续要在 Dashboard 内联「跨主体临近到期临时授权」提醒，需先在契约（base IA §六 + `CONTROL_ROUTES`）中**新增**一个真正的跨主体临时授权聚合读端点（如 `GET /v1/grants/temp` 的全局列表，而非复用单主体 `GET /v1/grants`），落地 router 后本卡再引用之；在该端点落地前，文档不引用任何不存在的跨主体 grants 读路由（与 §主要API 端点纪律一致）。

### 3.2 三态呈现（每张卡片独立，fail-closed）

- **加载中**：每卡片各自 `LoadingSkeleton`（卡内骨架行），不阻塞其他卡片渲染；卡片间独立请求、独立失败，一张失败不连累其余。
- **错误态**：`ErrorState` 占据该卡片体——**绝不显伪数据、绝不显陈旧缓存伪装成最新**。fail-closed 具体到每卡：
  - HealthCard 取数失败 ⇒ 健康灯显**红/未知态**（"daemon 不可达"），而非默认绿。系统姿态不确定即按最坏呈现。
  - ModePanel 取数失败 ⇒ 显"模式状态未知"红条，**不**默认显 NORMAL（不确定的模式不能假装成"无限制"）。其取数来源为 `POST /v1/mode` 同源读 / `mode_state` 投影（与顶栏徽章同源），失败时顶栏徽章与本卡一并按未知呈现。
  - DenialsTopTable 取数失败 ⇒ 卡内 ErrorState + 重试；不显"暂无拒绝"（取不到 ≠ 没有拒绝）。
  - VerifyCard 失败 ⇒ 各自 ErrorState，互不影响。
  - ExpiringGrants 无内联取数（跳转入口卡，不请求 `GET /v1/grants`），故无独立 ErrorState/空态；它恒显引导文案 + [前往 Grants]，临近到期的真实判定与三态由 Grants 页承担。
- **空态**：`EmptyState`，且空态语义必须真实：
  - DenialsTopTable 窗内零 deny ⇒ "近 7 天无拒绝记录"（这是**正向**事实，可显，与"取数失败"区分清楚）。
  - ModePanel 无资源覆盖 ⇒ 只显全局行 + "无资源级模式覆盖"。
  - VerifyCard 从未运行 ⇒ "尚未运行红队自检"+ [运行→Verify]，不伪造 PASS。

---

## 4. 交互流

Dashboard 以**观测 + 跳转**为主，写动作极少。所有写遵循基座统一写流程：表单→摘要预览→危险确认→失效刷新→成功/失败/409。

### 4.1 只读动作（无写，跳转/刷新）

| 用户动作 | 系统响应 | 预期结果 |
|---|---|---|
| 进入 Dashboard | 并发拉取 health / mode（`POST /v1/mode` 同源读·`mode_state` 投影）/ denials 三源；各卡独立骨架→数据。ExpiringGrants 为静态跳转卡，不发起取数 | 一屏呈现系统姿态；任一源失败该卡 fail-closed，不阻塞其余 |
| 点 [刷新 ⟳] | 失效全部取数源（health/mode/denials）缓存并重拉（TanStack Query refetch）；ExpiringGrants 无取数不参与刷新 | "最后更新"时间戳前进；卡片重新骨架→新数据；失败卡保持 ErrorState |
| DenialsTopTable 切 `window`（7d/24h/30d） | 以新 `window` 重拉 `GET /v1/denials/summary` | 榜单按新窗重算计数；切换中显骨架 |
| 点 deny 榜某行 [→audit] | 跳 Audit 页并预填筛选（principal + decision=deny + 该 stage/capability） | 落地 02-audit 已筛好该主体的 deny 流，可逐条看 reason（明细不在 Dashboard 泄露） |
| 点 DenialsTopTable [全部→Denials] | 跳 03-denials 页 | 落地完整拒绝分析（多维聚合、可下钻） |
| 点 ModePanel [管理模式→Mode] / 某资源覆盖行 | 跳 11-mode 页（资源行预选该资源） | 落地 Mode 页改模式（改模式是写动作，在 Mode 页走统一写流程，不在 Dashboard 内联） |
| 点 ExpiringGrants [前往 Grants] | 跳 05-grants 页（无预选主体——本卡不内联枚举任何主体/条目） | 落地 Grants 页，运维**按主体维度**逐个查看临近到期临时授权并处理（续期/吊销在 Grants 页确认）。Dashboard 不内联枚举跨主体临时授权（per-principal 端点无法提供） |
| 点 VerifyCard [运行→Verify] | 跳 04-verify 页 | 落地 Verify 页触发红队（运行是高耗动作，本页只显上次摘要、不主动触发） |
| 点 HealthCard [详情→Health] | 跳系统区 Health 视图 | 落地完整健康明细 |
| 悬浮/点雪花 id（如 policy_rev 关联、grant id） | 悬浮全展 + 一键复制 | id 恒等宽字符串，复制不丢精度 |

### 4.2 唯一写动作 + 危险动作清单

本页**唯一写动作**经顶栏 `GlobalEmergencyBar`，Dashboard 不另设写控件：

| 危险动作 | 触发位置 | 确认方式 | 写流程 |
|---|---|---|---|
| **全局 Freeze（一键拉闸）** | 顶栏 `GlobalEmergencyBar` 的 [⚡FREEZE]（Dashboard 与全站同一控件） | `ConfirmDialog` 高危确认：摘要预览"全局切 freeze ⇒ 一切动作被拒、保留只读视图、在用连接强制 abort"；需**显式勾选/输入确认**（防误触），可选 TTL | 表单(ModeSelector freeze + 可选 TTL)→摘要预览→危险确认→`POST /v1/mode`(resource=None)→失效刷新 mode（`POST /v1/mode` 同源读·`mode_state` 投影）/denials→成功提示(policy_rev 前进 + 可跳 audit 的 policy_change 事件)/失败红错(不改本地姿态)/409(他人已改模式，提示刷新重拉 mode 后重试) |
| **解除 Freeze（回落 normal）** | freeze 横幅 [解除 Freeze→Mode] 或顶栏 | 同上确认（解冻也是改全局姿态，二次确认） | 同上，目标 `normal` |

> freeze 成功后：`GlobalEmergencyBar` 立刻渲染红色脉冲横幅（覆盖内容区顶部），ModePanel 全局行转 FREEZE 红徽章；这是热生效——下一次 evaluate 读到新快照即拒。Dashboard 不参与连接 abort（那是数据面连接管理层的事），UI 只如实呈现新姿态。

> Dashboard **不内联**改模式（非 freeze）、不内联续期/吊销临时授权、不内联触发 verify——这些写动作各归其页，在对应页走统一写流程并二次确认。本页对这些动作只提供**跳转入口**，避免在观测面板上散落写控件造成误操作。

---

## 5. 复用的基座组件

本页全部由基座组件拼装，无新增独立组件（卡片是基座组件的组合容器）：

| 本页构成 | 复用基座组件 | 说明 |
|---|---|---|
| 顶栏应急区 + freeze 横幅 | `AppShell` / `GlobalEmergencyBar` / `ConfirmDialog` / `ModeSelector` | 全站常驻，Dashboard 不重复实现 freeze 逻辑，复用同一控件与确认流 |
| HealthCard | `LoadingSkeleton` / `ErrorState` + 健康灯（语义色点）+ 容量进度条 | 健康灯用 `--allow/--warn/--deny` 语义色 + 图标（状态不仅靠色） |
| ModePanel | `ModeSelector`（只读徽章态）/ `ResourceCodeBadge` / `TtlBadge` / `EmptyState` / `ErrorState` | 读自 `POST /v1/mode` 同源读·`mode_state` 投影（与顶栏徽章同源，非 `GET /v1/mode`）；模式徽章用 normal/observe/maintain/freeze 固定配色；资源行 ResourceCodeBadge；TTL 倒计时 TtlBadge 临近转琥珀 |
| DenialsTopTable | `DataTable`（强制分页/排序/空态/骨架）/ `CapabilityBadge` / `StageChip` / `ResourceCodeBadge` / 等宽 id | 即 DataTable 的聚合视图：count 列默认降序；stage 用 StageChip 固定色序；capability 用色温递增徽章；行操作菜单含 [→audit] |
| ExpiringGrants | `EmptyState` 风格的引导卡 + 跳转链接（不用 DataTable，无内联枚举） | 跳转入口卡：一句引导文案 + [前往 Grants]；不渲染主体/资源/到期项（per-principal `GET /v1/grants` 无法跨主体枚举，临近到期由 Grants 页按主体维度承担） |
| VerifyCard | `VerifyItemRow`（摘要计数态）/ `LoadingSkeleton` / `EmptyState` | 仅显 all_pass + pass/total + 失败项数；逐条 PASS/FAIL 在 Verify 页 |
| 雪花 id / hash | 等宽（JetBrains Mono）+ 中段截断 + 悬浮全展 + 复制 | 全站 id 纪律，本页 policy_rev / grant id 等遵守 |

---

## 6. 正常与异常预期

对照场景 02~07 概览，Dashboard 作为浓缩入口，其正常/异常预期一律 fail-closed。

### 6.1 正常操作（逐步可验收）

1. 健康正常 ⇒ HealthCard 三灯绿（daemon UP / store WRITABLE / policy_rev 显当前值）、容量水位未逼近上限 ⇒ 验收：三灯绿、rev 等于后端 `GET /v1/health` 返回值。
2. 全局 normal、db-main 切 observe（场景 06 §2.2）⇒ ModePanel 全局行 NORMAL 中性徽章、db-main 行 OBSERVE 蓝徽章 + TTL 倒计时 ⇒ 验收：徽章与 `POST /v1/mode` 同源读·`mode_state` 投影各辖区状态一致（且与顶栏 `GlobalEmergencyBar` 同源），TTL 倒计时随墙钟递减。
3. agent3 反复 mutate 被 constraint 拒（场景 05/06）⇒ DenialsTopTable 出现 `agent3 / db-main / mutate / constraint / N` 行且 N 随窗内累积 ⇒ 验收：计数 == `GET /v1/denials/summary` 该分组 count；点 [→audit] 落地已筛该主体 deny 流。
4. 临近到期临时授权的查看在 Grants 页（per-principal）：Dashboard 的 ExpiringGrants 为跳转入口卡 ⇒ 验收：本卡只显引导文案 + [前往 Grants]、不内联枚举任何到期项；点击落地 05-grants 页，运维按主体维度看到 `expires_at` 临近的临时格（剩余时间 == `expires_at` - 墙钟、过期格不再出现）由 Grants 页承担。
5. 上次红队 8/9 PASS（场景 07 §4）⇒ VerifyCard 显 `8/9 PASS ✗1` + 失败标记 ⇒ 验收：与上次 `POST /v1/verify` 报告 all_pass=false、pass=8 一致；点 [运行→Verify] 落地 Verify 页。
6. 全局 freeze（场景 06 §2.4 一键拉闸）⇒ 顶栏确认后红色脉冲横幅出现、ModePanel 全局 FREEZE ⇒ 验收：`POST /v1/mode` 200、policy_rev 前进、成功提示可跳 policy_change audit；横幅持续至解冻。

### 6.2 异常与边界（一律 fail-closed）

| 异常/边界 | 触发条件 | 系统行为 | 预期结果（fail-closed） |
|---|---|---|---|
| daemon 不可达 | `GET /v1/health` 失败/超时 | HealthCard 不默认绿 | 健康灯显红/未知"daemon 不可达"，不伪装 UP；其余卡片各自尝试，失败即各自 ErrorState |
| 模式取数失败 | `POST /v1/mode` 同源读·`mode_state` 投影失败（router 无 `GET /v1/mode`） | ModePanel 不默认 NORMAL | 显"模式状态未知"红条；**不**呈现"无限制"假象（不确定模式按未知，不按 normal）；顶栏同源徽章一并按未知 |
| deny 摘要取数失败 | `GET /v1/denials/summary` 失败 | DenialsTopTable 不显空 | 卡内 ErrorState + 重试，不显"无拒绝"（取不到≠无） |
| 权限不足/未认证 | 控制面认证（SO_PEERCRED+本地凭据，L-1）拒 | 整页/卡片 401/403 | fail-closed：显"无权访问控制面"，不渲染任何数据；不缓存伪数据 |
| 越权/Scope 外数据缺失 | 某资源/主体在当前视图 Scope 外 | 后端不返回该项 | UI 不显其存在性（与"不存在"不可区分）；deny 榜仅显 Scope 内项，不暴露 Scope 外资源存在性（grants 的 scope-bounded 在 Grants 页生效，Dashboard 不内联 grants 故无此面） |
| 分页越界 | DenialsTopTable 翻到超范围页（ExpiringGrants 无分页，不取数） | 后端钳制 | 返回空页（非报错），UI 显空态"本页无数据"；不崩 |
| freeze 写 409 | 提交 freeze 时他人已改全局模式（乐观锁版本不匹配） | `POST /v1/mode` 回 409 | 明确提示"他人已修改模式，请刷新重试"；**不改**本地姿态、不假装已 freeze；刷新重拉 mode 后可重试 |
| freeze 写失败(5xx) | 事务/快照重建/审计任一失败（无半态，L-14） | 后端整体 fail-closed | UI 显红色错误、本地姿态不变；不显伪 freeze 横幅（未落库即未冻结） |
| 审计存储逼近水位 | health 容量水位高（场景 07 §存储健康） | HealthCard 水位条转琥珀/红 | 显容量告警（"audit store 容量 61%/逼近上限"），区分"存储健康（平面级）"与"单条写失败"；不阻塞页面其余信息 |
| daemon 重启后 | 重连后首次拉取 | 各取数源（health/mode/denials）重拉 | 模式按持久化 `mode_state.expires_at` 恢复（非内存倒计时）；UI 呈现恢复后的真实姿态。临时授权的过期回收在 Grants 页按持久化 `expires_at` 体现，Dashboard 不内联 grants 故不在本页判定 |
| 上次 verify 有 FAIL | all_pass=false | VerifyCard 显失败计数 | 显 `✗N`，但**不**替系统生成"建议"——只陈述事实计数，修订方向由人到 Verify 页看 gap_note 后裁决 |

---

## 7. 与后端契约对齐

本页凡涉及契约处，逐项对齐《00》第八部分硬约束：

- **强制分页**（`DB_PAGINATION_MANDATORY`）：DenialsTopTable 凡走集合读端点（`GET /v1/denials/summary`）一律带 `page_no/page_size`；缺省 `page_no=1, page_size=20`，`page_size>200` 钳到 200、`page_no<1` 钳到 1（对齐 `endpoints::page_query` / `PageQuery::clamp`）。Dashboard 默认只取首页（榜单 Top N），不滚动加载全量。ExpiringGrants 为跳转入口卡、不走任何集合读端点，故不涉及本约束；临时授权的分页读在 Grants 页对 per-principal `GET /v1/grants` 生效。
- **雪花 id 字符串不丢精度**：policy_rev、grant id、principal id 等一切 id 全程作**字符串**渲染，前端绝不当 number 解析；等宽呈现、中段截断、悬浮全展、一键复制（policy_rev 虽为 `u64` rev，也以等宽机器事实体例显示，不参与算术展示）。
- **deny 逐字段不泄露存在性**：deny 聚合榜只显后端 `denials/summary` 返回的 Scope 内分组（principal/resource/capability/stage/count）；**不**展示 reason 全文（明细在 Denials/Audit 页逐字段看，`operator_note` 在那里原样转述、不加工），**不**暴露 Scope 外或不存在资源——out-of-scope 与 nonexistent 在 UI 上不可区分（对齐 `DenyResponse.your_grants` 的 scope-bounded 语义）。
- **模式语义固定**：ModePanel 与 freeze 严格用四值 `normal/observe/maintain/freeze`（固定集合、不可由策略新增，CONS-18）；全局键为 `None`、资源覆盖为 `Some(code)`（对齐 `PolicySnapshot.modes`）；UI 不臆造第五种模式、不把"未知"渲染成 normal。
- **端点面只引用 router 实有路由**（对齐 `CONTROL_ROUTES`）：
  - **模式读无独立 `GET /v1/mode`**：`CONTROL_ROUTES` 中模式只有 `POST /v1/mode` 写端点；当前模式姿态读自 `POST /v1/mode` 同源写的 `mode_state` 投影（与 11-mode.md §展示字段、顶栏 `GlobalEmergencyBar` 同源），ModePanel 不引用任何不存在的 `GET /v1/mode` 路由。
  - **`GET /v1/grants` 是 per-principal / scope-bounded**：返回单个 Principal 的展开授权矩阵 + 其临时授权清单（对齐 core `DenyResponse.your_grants = BTreeMap<ResourceCode, Vec<String>>` 的单主体语义、05-grants.md「principal 维度」），**不**是跨主体全集。Dashboard 不复用它伪造「跨主体临近到期临时授权」枚举；router 中亦无跨主体临时授权聚合读端点，故 ExpiringGrants 收敛为跳转入口卡（要在 Dashboard 内联跨主体提醒，须先在契约新增真正的聚合读端点再引用）。
- **写端点乐观锁版本**：freeze 写 `POST /v1/mode` 携带期望 version，版本冲突回 **409 Conflict**（对齐 `WriteHttp::Conflict`），UI 提示"他人已改、刷新重试"、不改本地视图；5xx 写失败（事务/快照重建/审计任一失败，无半态，L-14）UI 红错且不改姿态。
- **写后果可追溯**：freeze 成功后提示 `policy_rev` 前进，并可跳转对应 `policy_change` audit 事件（写端点三联动的审计支 `actor=operator` 走乐观锁，L-14）。
- **脱敏纪律**：HealthCard/ModePanel/DenialsTopTable 全程不显真实地址、不显凭据明文、不显 `secret_hash`；资源只显代号（`ResourceCode`），凭据若涉及只显元数据（本页不直接展示凭据；ExpiringGrants 不内联授权数据，脱敏面在 Grants 页生效）。
- **fail-closed 呈现**：一切加载失败/取数失败/权限不足/数据缺失默认呈现为受限/未知/拒绝态，绝不乐观假装健康或已生效（对齐《00》原则一与七公理 fail-closed）。
