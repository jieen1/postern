# 11 · 模式 Mode

> 运行期的"方向盘与刹车"：把全局或某资源辖区切到 `normal/observe/maintain/freeze` 四档固定模式，可附 TTL，热生效收窄动词集；`freeze` 是一键拉闸（拒绝一切 + 砍断在飞危险操作）。本页是控制面 `POST /v1/mode` 的图形视图，**复用基座令牌与组件，遵守统一交互模式与后端契约硬约束**（见 `00-设计系统与信息架构.md`）。纯设计，不含实现代码。

---

## 一、页面定位

**一句话**：以"当前各辖区生效模式 + 覆盖关系"为中心的运行态收紧台——切换全局/单资源模式（四档固定、可附 TTL）、一键 freeze 急停、并直读切换对授权世界的收窄效果。

- **对应场景**：`docs/examples/06-动态权限调整与失控切断.md` 之 §2.2（模式切换）、§4.1·B/C（observe/maintain 热生效、freeze 切断在飞）、§4.2 异常表 #2/#7/#8/#10/#13/#14。
- **主要 API**：
  - `POST /v1/mode` —— 写：切换某辖区模式（全局或单资源）、附 TTL、回落 normal。**唯一写端点**，携带乐观锁 `version`。
  - `GET /v1/grants` —— 读：当前生效授权世界（与数据面 `your_grants` 同源），用于回显"切到该模式后某主体还剩哪些动词"，验证收窄效果。
- **不在本页**：临时提权（`/v1/grants/temp/*` → 05-grants）、吊销凭证（→ 10-principals-credentials）、连接强制中断的审计明细（→ 02-audit `connection_event`）。本页只管"按操作冻结/收窄"这一粒度；"按身份切断"在凭证页。
- **顶栏联动**：基座 `GlobalEmergencyBar` 的 Freeze 急停开关与全局模式徽章与本页**同源**（都读/写 `/v1/mode` 全局行）。顶栏管"全局一键"，本页管"全辖区全貌 + 资源级粒度"。

---

## 二、布局

本页**不是**标准列表页（模式集合固定四档、辖区有限），而是"看板（全貌）+ 切换抽屉（写）"的详情态骨架。主区是辖区模式看板（DataTable 承载资源辖区行 + 顶部全局态卡片），右侧 FormDrawer 承载切换写流程。

```
┌─ AppShell ────────────────────────────────────────────────────────────────┐
│ [顶栏] postern   …   ◐主题   ⬤daemon健康   [全局模式: OBSERVE▾]  [⏻ FREEZE] │ ← GlobalEmergencyBar(同源)
├─ 左导航 ─┬─ 内容区: 模式 Mode ─────────────────────────────────────────────┤
│ 总览     │  模式 Mode                                  [切换模式 ▸](右上主操作)│
│ 观测…    │  当前各辖区运行模式与覆盖关系；最严者生效（freeze>observe>maintain>normal）│
│ 授权…    │ ┌─ 全局辖区 (Global) ────────────────────────────────────────┐  │
│ 接入…    │ │ [ModeBadge: OBSERVE]  作用域: 全局   TTL: [TtlBadge 23:41]   │  │
│ 系统     │ │ 生效自 2026-06-14 09:12 · by admin · policy_rev 1042         │  │
│  ▸模式   │ │ 收窄: 仅放行 observe / query (只读)         [切换▸] [回落normal▸]│  │
│  审批    │ └─────────────────────────────────────────────────────────────┘  │
│  设置    │                                                                   │
│  …       │  资源级覆盖 (Resource overrides)            [筛选: 资源代号 / 模式▾]│
│          │ ┌─ DataTable (强制分页 page_no/page_size 20/200) ──────────────┐  │
│          │ │ 资源代号        本地模式   有效模式(取严)  TTL      生效自/by  ⋯│  │
│          │ │ ──────────────────────────────────────────────────────────── │  │
│          │ │ docker-A🐳tcp   MAINTAIN   MAINTAIN      ⏳1:58:..  …/admin  [⋯]│  │
│          │ │ redis-main⚡tcp  —(继承)    OBSERVE←全局   —         继承      [⋯]│  │
│          │ │ db-main🛢tcp     FREEZE     FREEZE        —         …/admin  [⋯]│  │
│          │ │ svc-order🌐http  —(继承)    OBSERVE←全局   —         继承      [⋯]│  │
│          │ └──────────────────────────────────────────────────────────────┘ │
│          │   [行操作⋯] 切换此资源 / 回落继承 / 查看该资源下各主体收窄(→grants) │
│          │                                                                   │
│          │ ┌─ 收窄影响预览 (可选展开, 读 GET /v1/grants) ─────────────────┐  │
│          │ │ 选定资源/主体 → 表格: 主体 | RBAC原始动词 | 该模式后剩余动词 │  │
│          │ └──────────────────────────────────────────────────────────────┘ │
└──────────┴───────────────────────────────────────────────────────────────────┘

右侧抽屉 ModeSwitchDrawer（点"切换模式"/行"切换此资源"时滑出）:
┌─ FormDrawer: 切换模式 ───────────────────────────────┐
│ 作用域 Scope:  ( ) 全局   (•) 资源: [ResourceCodeBadge db-main ▾] │
│ 目标模式 Mode (ModeSelector, 单选):                  │
│   (•) normal  ( ) observe  ( ) maintain  ( ) freeze  │
│   ┌ 各档收窄说明(只读事实, 不替系统编建议) ─────────┐ │
│   │ freeze: 拒绝一切动词(含只读)。最高危。           │ │
│   └─────────────────────────────────────────────────┘ │
│ TTL (可选): [____] [分▾]  空=长期, 到期由 sweeper 回落 │
│ ── 摘要预览 (提交前) ────────────────────────────────│
│  辖区 db-main: observe → FREEZE   TTL: 1h            │
│  影响: 该辖区一切动词将被拒; 在飞危险操作将被强制中断 │
│  期望 version: 1042  (乐观锁)                        │
│  [ ConfirmDialog 高危: 输入资源代号 "db-main" 确认 ] │
│                              [取消]  [确认切换 ⚠]    │
└──────────────────────────────────────────────────────┘
```

- 全局态卡片常驻主区顶部（单行、无分页），与顶栏 `GlobalEmergencyBar` 同源；资源级覆盖走 DataTable（强制分页）。
- DataTable 关键列：**本地模式**（该辖区自身存的 `mode_state` 行；`—(继承)` = 该资源无覆盖行）与**有效模式**（`本地.meet(全局)` 取严，标注 `←全局` 说明继承来源）两列并列，使"覆盖关系/取最严"一眼可辨。

---

## 三、数据与状态

### 展示字段（均来自 `POST /v1/mode` 同源读 / `mode_state` 投影）
| 字段 | 来源 | 呈现 | 纪律 |
|---|---|---|---|
| `scope` | `scope_resource_id`（NULL=全局） | 全局态卡片 / 资源代号行 | 资源用 `ResourceCodeBadge`（代号，**永不显真实地址**） |
| `mode`（本地） | `mode_state.mode` | `ModeBadge`：normal=中性 / observe=蓝 / maintain=琥珀 / freeze=红 | 固定四档配色，状态不仅靠色（带文字 + 图标） |
| 有效模式 | core `effective_mode` = `global.meet(scoped)` | 取严后的 `ModeBadge` + `←全局/←本地` 标注 | 取严计算在 core；前端只展示，**不自己另算一套** |
| `expires_at` | `mode_state.expires_at` | `TtlBadge` 倒计时（绝对墙钟，临近转琥珀；空=长期） | 绝对时刻，非倒计时；空值显"长期" |
| `version` | `mode_state.version` | 隐式持有，写时回传（乐观锁） | 切换抽屉摘要显式展示期望 version |
| 生效自 / by / policy_rev | 写后元数据 | 卡片副行（`updated_at` / `updated_by` / 落库 policy_rev） | id/代号等宽；可跳对应 `mode_change` audit |
| 收窄说明 | core 内置常量表语义（只读） | 各档"放行哪些动词"的客观陈述 | **事实陈述，不生成建议话术**（原则 2） |

> 协议中一切 id 恒为字符串、等宽显示、中段截断 + 悬浮全展 + 一键复制；**绝不当 number 解析**（雪花精度纪律）。

### 三态（fail-closed）
- **加载中**：全局态卡片与 DataTable 用 `LoadingSkeleton`；顶栏全局模式徽章在未读到前显"加载中"占位，**不假定 normal**。
- **错误**：读 `/v1/mode` 失败 → `ErrorState`（fail-closed，**不显伪数据、不假装 normal**）；明确红色文案"无法读取当前模式，请重试"。控制面不可达（`control.sock` 缺席/无权/daemon 未运行）→ 同 ErrorState 并提示"控制面不可达——本页无法确认或更改安全状态"（对应场景 §4.2 #12：daemon 侧状态不因前端缺席而改变，**不静默以为成功**）。
- **空**：无任何资源级覆盖行 → DataTable `EmptyState`："当前无资源级模式覆盖，全部辖区继承全局模式 `<全局模式>`"，带"切换模式"主操作引导。全局无显式行即 `normal`（store 无生效行=normal，如实呈现）。

---

## 四、交互流

所有写均走基座**统一写流程**：表单（ModeSelector + TTL）→ 提交前**摘要预览**（旧模式→新模式 + 收窄影响 + 期望 version）→ 危险确认 → 提交 `POST /v1/mode` → 失效刷新（看板 + 顶栏徽章 + 受影响 grants 视图）→ 成功提示（policy_rev 前进 + 可跳 `mode_change` audit）/ 失败红色错误（**不改本地视图**）/ 409 冲突（提示刷新重读）。

| 用户动作 | 系统响应 | 预期结果 |
|---|---|---|
| 点"切换模式"/行"切换此资源" | 滑出 `ModeSwitchDrawer`，预填作用域与当前模式 | 表单就绪；危险确认按目标模式动态升降级 |
| 选 normal/observe/maintain | 摘要预览：`旧→新` + 该档收窄事实（如 observe=只读） | 标准确认（`ConfirmDialog` 勾选）即可提交 |
| 选 **freeze** | 摘要追加："拒绝一切动词（含只读）；在飞危险操作将被强制中断" | **最高危确认**：需**输入辖区标识**（全局输入 `GLOBAL`，资源输入其代号）才解锁"确认切换" |
| 设 TTL | 摘要显示"到期 `<expires_at>` 由 sweeper 自动回落上层默认" | 留空=长期；到期回落留 `mode_change` 审计痕迹 |
| 确认切换 | `POST /v1/mode`（带 `version`）→ 三联动（事务+快照重建+审计） | 成功 → 看板 + 顶栏徽章 + grants 视图刷新；toast "模式已切换，policy_rev → N（可查 audit）" |
| 行"回落继承/normal" | 摘要："该辖区回落到上层默认（全局/normal）" | 解除限制经**显式切换**留痕（绝非翻 enable_flag）；同走确认 + 写流程 |
| 顶栏 FREEZE 急停 | 等价"全局 → freeze"，同源最高危确认 | 全局 freeze；本页全局态卡片与顶栏横幅同步进入红色脉冲冻结态 |
| 展开"收窄影响预览" | 读 `GET /v1/grants`（按辖区/主体） | 表格对比"RBAC 原始动词 vs 该模式后剩余动词"，验证收窄；**只读、不写** |

### 本页危险动作清单（一律 ConfirmDialog）
1. **切到 freeze（全局或单资源）= 最高危**：摘要明示"拒绝一切 + 砍断在飞"；需**输入辖区标识**确认（防误触，对齐基座原则 4 freeze 防误触）。
2. **切到 observe / maintain**：扩限制类，标准确认（勾选 + 摘要预览）。
3. **回落 normal / 回落继承**：解除限制（放宽侧），仍需确认（解除限制不可无声，对齐场景 #8）——摘要明示"将放宽至 `<上层模式>`"。
4. **附短 TTL 的 freeze**：额外提示"到期将自动解冻回落，是否符合预期"——避免误以为永久冻结。

---

## 五、复用的基座组件

| 组件 | 本页用法 |
|---|---|
| `AppShell` | 全局骨架；本页挂左导航"系统 › 模式"。 |
| `GlobalEmergencyBar` | 顶栏 Freeze 急停 + 全局模式徽章，与本页全局态**同源**（读写同一 `/v1/mode` 全局行）。 |
| `ModeSelector` | 抽屉内 normal/observe/maintain/freeze 单选 + TTL 输入；freeze 触发高危确认。 |
| `DataTable` | 资源级覆盖列表（强制分页 `page_no/page_size`、排序、按资源代号/模式筛选、行操作菜单、空/加载/错误三态）。 |
| `ResourceCodeBadge` | 辖区资源代号（等宽 + adapter/transport 小图标，**永不显真实地址**）。 |
| `CapabilityBadge` | "收窄影响预览"中动词列（observe…destroy 色温递增）。 |
| `FormDrawer` | `ModeSwitchDrawer` 容器：RHF+Zod 校验 + 摘要预览 + 409 提示 + 成功提示。 |
| `ConfirmDialog` | freeze 输入确认 / observe·maintain·回落 勾选确认。 |
| `TtlBadge` | 模式 TTL 倒计时（临近过期转琥珀，空=长期）。 |
| `EmptyState / ErrorState / LoadingSkeleton` | 三态 fail-closed 呈现。 |

**本页特有的组装（用基座组件拼，非新原子组件）：**
- `ModeBadge`：模式徽章——直接复用基座"模式配色"令牌（normal=中性 / observe=蓝 / maintain=琥珀 / freeze=红）+ 文字 + 图标的小徽章（=基座徽章 + 模式令牌）。
- `GlobalModeCard`：全局态卡片——`ModeBadge` + `TtlBadge` + 元数据副行 + 两个行操作按钮拼装。
- `OverrideRow`：DataTable 行渲染——本地 `ModeBadge`、有效 `ModeBadge`（带 `←全局/←本地` 取严来源标注）、`TtlBadge`、行操作菜单。
- `NarrowingPreview`：收窄影响预览——读 `GET /v1/grants`，用 `CapabilityBadge` 列对比 RBAC 原始动词与该模式后剩余动词。

---

## 六、正常与异常预期

### 正常（对照场景 §4.1·B/C）
- **切 observe（全局）**：摘要 `normal→observe`；确认后 `POST /v1/mode` 成功 → 全局态卡片转蓝 OBSERVE、顶栏徽章同步、policy_rev 前进、可跳 `mode_change`。收窄预览显示各主体仅剩 observe/query。正在跑的只读监控**不受影响**（热生效，前端无需提示重启）。
- **切 maintain（单资源 + TTL 2h）**：`docker-A` 行本地模式转 MAINTAIN、`TtlBadge ⏳1:59:..` 倒计时；有效模式取 `本地.meet(全局)`。到期由 sweeper 回落上层默认，看板自动刷新回落后态（轮询/失效刷新）。
- **freeze（全局，一键拉闸）**：输入 `GLOBAL` 确认 → 全局态卡片 + 顶栏进入红色脉冲冻结横幅；收窄预览显示**一切动词被拒（含只读）**。场景 §4.1·C #11 在飞 destroy 被强制中断的事实，在"切断已生效"提示中陈述并可跳 `connection_event` audit（本页不展示连接明细，仅给跳转）。
- **回落 normal**：显式切换写流程，policy_rev 前进、留 `mode_change` 痕迹；看板回 NORMAL。

### 异常 / 边界（一律 fail-closed，对照 §4.2）
| 触发 | 本页预期（fail-closed） |
|---|---|
| **读模式失败 / 控制面不可达**（#12） | `ErrorState`，**不显伪数据、不假定 normal**；明示"无法确认或更改安全状态"；写按钮置灰。 |
| **权限不足 / 越权**（控制面认证失败） | 写返回错误 → 红色提示，**本地视图不变**；不静默"以为切换成功"。 |
| **乐观锁 409**（#10：两运维并发改同辖区） | `UPDATE WHERE version=?` 命中 0 行 → 409 原样呈现 "他人已改该辖区模式，请刷新重读最新 version 再试"；**不静默重试覆盖**；摘要中的期望 version 与服务端不符即拒。 |
| **多行生效模式兜底**（#7：旁路 freeze 为 normal） | 看板"有效模式"由 core 取**最严者**（`freeze>observe>maintain>normal`）；前端如实展示最严结果，**绝不取最宽松**；若后端附告警事件，本页提供跳 audit。 |
| **试图悄停冻结（enable_flag 翻转）**（#8） | 本页**无"停用此模式行"控件**——解除冻结**只能**经显式"回落 normal"写流程；限制性表 `CHECK(enable_flag=1)` 在后端兜底。 |
| **非法模式名 / 缺 TTL 语义** | 模式固定四档（`ModeSelector` 物理只有四项，不可自定义，CONS-18）；TTL 选填，非法值前端 Zod 拒、后端 `CHECK` 兜底。 |
| **解冻写失败（事务/审计任一失败）**（#14） | 三联动任一失败即整体失败 → 返回错误，**状态保持更严侧（仍冻结）**；提示重试；绝不"以为解冻实则仍冻"或反之。 |
| **分页 / 大量资源覆盖** | DataTable 强制分页（缺省 20、钳 200）；空覆盖→ EmptyState（"全部继承全局 `<模式>`"）。 |
| **grants 视图越权/数据缺失** | 收窄预览读 `GET /v1/grants` 缺数据 → 不补默认、不显伪格；按 fail-closed 显空/受限态。 |

---

## 七、与后端契约对齐

- **分页**：资源级覆盖 DataTable 一律分页，`page_no/page_size` 缺省 `20`、钳 `200`（对齐 `DB_PAGINATION_MANDATORY`）。
- **雪花 id 字符串**：`mode_state` 行 id、`scope_resource_id` 关联的资源 id、policy_rev 等全程字符串、等宽展示、**绝不丢精度**。
- **乐观锁 version**：`POST /v1/mode` 携带先前读取的期望 `mode_state.version`；冲突返回 `409 Conflict`，**原样呈现、不静默重试**（场景 §3.1 写命令携带期望 version）。
- **模式固定四档、内置语义**：`normal/observe/maintain/freeze` 由 core 内置常量表定义覆盖规则（Normal=全放行、Observe={observe,query}、Maintain={observe,query,mutate,execute}、Freeze=全拒），**不从策略库读"模式定义"、不可由策略新增**（CONS-18）。前端 ModeSelector 物理只有四项。
- **取最严覆盖**：有效模式 = `global.meet(scoped)`，strictness `normal<maintain<observe<freeze`（**observe 比 maintain 更严**，因只读）；取严计算在 core，前端只展示其结果（含多行兜底取最严），**不自行另算**。
- **限制性表**：`mode_state` 禁 `enable_flag` 翻转（`CHECK(enable_flag=1)`）——解除限制**只能**经显式 `mode set normal`/回落写流程，各留 `mode_change` 审计，**绝非无声 flag 翻转**。
- **deny 不泄露存在性**：收窄预览借 `GET /v1/grants` 时，按主体作用域呈现其**自身**授权世界（`your_grants` 同源）；out-of-scope / 不存在资源不可区分（`DENY_RESPONSE_SCOPE_BOUNDED`），缺数据不补默认。
- **脱敏在前端成立**：辖区只显资源代号（`ResourceCodeBadge`），**永不显真实地址 / 凭据明文 / secret_hash**。
- **TTL 绝对墙钟**：`expires_at` 为绝对时刻持久化，daemon 重启后剩余继续计时（不重置/不延长）；前端 `TtlBadge` 据绝对时刻渲染倒计时，不做本地倒计时累加假设。
