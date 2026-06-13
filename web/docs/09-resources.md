# 09 · 资源 Resources

> 本文是 postern 控制台「资源 Resources」页的详细布局与交互设计，在 [00-设计系统与信息架构](00-设计系统与信息架构.md)（令牌/组件/IA/统一交互模式/后端契约硬约束）之上展开，**复用基座、不重定义**。纯设计，不含实现代码。标识符/组件名/端点用英文（与基座一致）。

---

## 一、页面定位

**资源接入台**：管理员在此把异构后端（服务器经 ssh/ssm、数据库经 postgres、业务系统经 http、运行时经 docker）声明为受控资源——绑定 transport/adapter、打标签、声明凭据等级 tier（动词集 + `vault://` 引用）、配置会话来源资源的 auth_flow，并触发能力面探测（discover）。**本库不存任何真实地址，UI 只显代号**（设计原则 6）。

- **对应场景**：[场景 02 · 资源接入与认证配置](../../docs/examples/02-资源接入与认证配置.md)。承接对象是「资源、tier、认证、匿名化在此一次性钉死」；本页**不**承载主体/凭据明文录入（落 10-principals-credentials）与授权格细则圈选回写（落 08-constraints-conditions），但能力面探测的圈选入口由本页发起。
- **主要 API 端点**：
  - `GET /v1/resources` —— 资源列表（`Page<T>` 信封，强制分页）。
  - `POST /v1/resources` —— 声明/修订资源（含 transport/adapter/labels/credential_tiers/auth_flow，乐观锁）。
  - `POST /v1/resources/{code}/discover` —— 能力面探测（发现≠授权，返回 `CapabilitySurface`）。
- **关联只读上下文**（本页只引用、不在此写）：`POST /v1/verify`（接入后红队自检，详见 04-verify）、`POST /v1/credentials`（凭据明文录入，详见 10）。

---

## 二、布局

遵循基座「列表页统一骨架」：标题 + 主操作（右上）+ 筛选条 + DataTable（强制分页）+ 行操作 + 右侧 FormDrawer。资源接入是多面板表单（基本信息 / tiers / auth_flow），FormDrawer 内分段；discover 与校验结果用右侧抽屉的次级视图承载。

### 2.1 列表页（默认视图）

```
┌─ AppShell ───────────────────────────────────────────────────────────────────────┐
│ [postern]   〔Freeze ⏻〕 〔mode: normal〕   ◯ daemon healthy        ☀/☾ theme      │  ← GlobalEmergencyBar（基座顶栏常驻）
├──────────────┬────────────────────────────────────────────────────────────────────┤
│ 导航          │  资源 Resources                                   〔+ 接入资源〕      │  ← 标题 + 主操作（右上，打开 FormDrawer）
│  ─ 观测       │ ┌────────────────────────────────────────────────────────────────┐ │
│   审计        │ │ 筛选: [代号/标签 ⌕]  adapter▾  transport▾  状态▾(启用/停用)  tier▾ │ │  ← 筛选条（"/" 聚焦）
│  ─ 授权       │ └────────────────────────────────────────────────────────────────┘ │
│   …           │ ┌────────────────────────────────────────────────────────────────┐ │
│  ─ 接入       │ │ code        adapter  transport  tiers          labels    状态  ⋮ │ │  ← DataTable 表头（可排序）
│  ▸ 资源 ●     │ │ ───────────────────────────────────────────────────────────────│ │
│   主体        │ │ ▣db-main    postgres   ssm       ro·rw         env=prod  启用  ⋮ │ │
│   凭证        │ │ ▣svc-order  http       ssh       ro·op         tier:web  启用  ⋮ │ │  ← 行：ResourceCodeBadge + Capability/tier 徽章
│  ─ 系统       │ │ ▣docker-A   docker     ssh       ops           env=prod  启用  ⋮ │ │
│   …           │ │ ▣svc-crm    http       ssh       ro            tier:web  停用  ⋮ │ │  ← 停用行：弱化 + 琥珀"停用"徽章
│              │ │                                                                  │ │
│              │ │   〔行操作 ⋮〕 = 探测 discover / 编辑 / 配置认证 / 停用|启用       │ │
│              │ └────────────────────────────────────────────────────────────────┘ │
│              │   page_no ◂ 1/3 ▸    page_size [20▾]   共 47 条                       │  ← 强制分页（缺省20/钳200）
└──────────────┴────────────────────────────────────────────────────────────────────┘
```

列说明（紧凑表格，密度优先）：

| 列 | 组件/呈现 | 约定 |
|---|---|---|
| `code`（codename） | **ResourceCodeBadge**（等宽 + adapter/transport 小图标） | 永不显真实地址；点击进详情抽屉 |
| `adapter` | 文本徽章（postgres/http/docker/…） | 决定可分类的动词集 |
| `transport` | 文本徽章（ssh/ssm/direct） | 到达通路；真实坐标仅 `vault://{code}/target` 引用 |
| `tiers` | 多枚 **CapabilityBadge** 串（按 tier 折叠：`ro·rw`） | 悬浮展开每 tier 的动词集；只读 |
| `labels` | `key=value` 小徽章（截断+悬浮） | 供 Scope 标签选择器展开（binding_scope.selector） |
| 状态 | 启用/停用徽章（`enable_flag`） | 停用=琥珀；逻辑删除（`delete_flag`）行不显 |
| `⋮` | 行操作菜单 | discover / 编辑 / 配置认证 / 停用·启用 |

> `id`（雪花）不进主表列（管理员按 `code` 寻址）；若详情区展示 `id`，按基座 3.4 等宽截断 `7300…0123` + 悬浮全展 + 复制，**全程字符串**。

### 2.2 接入/编辑抽屉（FormDrawer，分段）

```
┌─ FormDrawer: 接入资源 / 编辑 db-main ──────────────────────────┐
│ ① 基本信息                                                     │
│   代号 code      [ db-main______ ]  （唯一，未删集内）           │
│   adapter        ( postgres ▾ )    transport ( ssm ▾ )         │
│   engine_enforced  ◉ true  ○ false   （adapter 决定缺省）       │
│   标签 labels    [env=prod ✕][region=… ✕]  [+ 添加]            │
│                                                                │
│ ② 真实地址（匿名化）                                           │
│   host / port    [ ················ ]  ⓘ 提交即转 vault 引用    │  ← 输入后回显为 vault://db-main/target，不回明文
│   现值: vault://db-main/target                                  │
│                                                                │
│ ③ 凭据等级 tiers（≥1 只读 tier）                               │
│   ┌ ro  ▸ caps: [observe][query]  secret_ref: vault://…/ro ┐  │
│   ┌ rw  ▸ caps: [mutate]          secret_ref: vault://…/rw ┐  │
│   〔+ 添加 tier〕   （tier 凭据明文 → 10-credentials 录入）     │
│                                                                │
│ ④ 认证流程 auth_flow（仅会话来源 tier 显示，见 §4.4）          │
│   [ 为某 tier 配置认证 → 展开 auth_flow 子表单 ]               │
│                                                                │
│  期望版本 version: 7   （编辑时携带；新建无）                   │
│ ────────────────────────────────────────────────────────────  │
│            〔取消〕      〔预览摘要 →〕                         │  ← 提交前必经摘要预览
└────────────────────────────────────────────────────────────────┘
```

### 2.3 能力面探测视图（discover，右侧抽屉次级视图）

```
┌─ Discover: db-main（postgres / ssm）────────────────────────────┐
│ ⚠ 发现 ≠ 授权 —— 探测只列"有哪些对象"，未圈选对象一律默认拒绝   │  ← 显式边界横幅（info/warn）
│ 探测于 2026-06-13 14:22 · 经 ssm 真实连上 · CapabilitySurface    │
│                                                                  │
│ 探得能力 capabilities:  [observe][query][mutate]   （只读展示）  │  ← CapabilityBadge
│                                                                  │
│ 探得对象 objects（圈选纳入授权细则）:                            │
│   ☐ public.orders        ☐ public.customers   ☐ public.audit    │
│   ☐ public.payments      ☑ public.products    …                 │
│                                                                  │
│  已选 1 项 →  〔以选中对象去配置 query 细则（08）〕              │  ← 圈选只跳转，不在本页落授权
└──────────────────────────────────────────────────────────────────┘
```

---

## 三、数据与状态

### 3.1 展示字段（均来自后端，无前端编造）

- **列表行**（`GET /v1/resources` 的 `Page<T>.list[]`，对应 `resources` + `resource_labels` + `resource_credential_tiers` 投影）：`code`(codename)、`adapter`、`transport`、`tiers[]`(每项 `tier` + `capabilities[]` + `secret_ref` 引用串)、`labels[]`、`enable_flag`、`version`、`id`(字符串)。
- **真实地址**：**永不下发明文**。`transport_config` 仅含 `host_ref="vault://{code}/target"` 一类引用串；UI 只显引用串本身（等宽），绝不显 `10.0.3.x`/instance-id/域名。
- **discover 结果**（`POST /v1/resources/{code}/discover` → `CapabilitySurface`）：`capabilities: Capability[]`（observe…destroy）、`objects: string[]`（如 `"public.orders"`）。**只读事实，不含凭据/地址**。
- **tier 元数据**：`tier`(代号)、`capabilities`(动词集)、`secret_ref`(`vault://` 引用，**不可解析、不可点开**)、`auth_flow`(非敏感配置 JSON，会话来源 tier 才有)。**绝不显 secret_hash / 凭据明文**（设计原则 6）。

### 3.2 三态（fail-closed，基座组件）

| 态 | 组件 | 呈现 |
|---|---|---|
| 加载中 | **LoadingSkeleton** | 表格骨架行；discover 探测中显进度态（"经 ssm 连接探测中…"，可取消），**不预填伪对象** |
| 错误 | **ErrorState** | 红色错误带，**不显伪数据/不显缓存旧表**；错误文案经脱敏（见 §6/§7），绝不回显后端原始地址串 |
| 空 | **EmptyState** | "尚无资源。接入第一个资源以纳入网关" + 〔+ 接入资源〕主操作引导 |

discover 的三态独立于列表：探测失败 → 抽屉内 ErrorState 显「具体缺口」（如"目标端口不可达，疑未发布转发"），**经脱敏**、不含真实端口/IP（场景 E3）。校验失败（tier⊄真实权限）→ 抽屉内显缺口项并**阻断接入完成**（见 §4.5）。

---

## 四、交互流

所有写操作走基座**统一写流程**：表单（RHF+Zod）→ **摘要预览** →（危险则）ConfirmDialog → 失效刷新（TanStack Query invalidate）→ 成功提示（`policy_rev` 前进 + 可跳 audit）/ 失败红色错误（不改本地视图）/ 409 冲突（提示刷新重试）。

### 4.1 浏览/筛选/分页（只读）

- 输入筛选 / 切 adapter·transport·状态·tier → 重查 `GET /v1/resources`（带筛选 + `page_no`/`page_size`）→ 表格更新。
- 翻页/改页大小 → 重查；`page_size` 超 200 由后端钳到 200（前端选择器仅给 20/50/100/200，对齐契约）。

### 4.2 接入资源（写，POST /v1/resources）

1. 点〔+ 接入资源〕→ FormDrawer（§2.2）。
2. 填基本信息（code/adapter/transport/labels/engine_enforced）、真实地址（输入框）、tiers（动词集 + 后续凭据引用）。
3. 〔预览摘要 →〕：弹**摘要预览**——逐字段列将写入什么（含"真实地址将转为 `vault://{code}/target` 引用、明文不入库"、"声明的 tiers 及动词集"、"engine_enforced 取值"）。
4. 确认提交 → `POST /v1/resources`。
5. **成功**：抽屉关闭、列表失效刷新、新行出现；toast「资源 `db-main` 已接入，policy_rev → N，可查看 audit ↗」。资源此刻**无授权格、默认拒绝一切**（公理一），提示"下一步：探测能力面 / 录入凭据 / 配置授权"。
6. **失败/409**：见 §6。

### 4.3 能力面探测 discover（半写半读，POST /v1/resources/{code}/discover）

1. 行操作或详情内点〔探测 discover〕。
2. daemon 经该资源 transport 真实连上、adapter 探测 → 返回 `CapabilitySurface`。
3. 探测视图（§2.3）列 `capabilities` + `objects`，**置顶横幅明示"发现≠授权"**。
4. 管理员圈选 objects → 〔以选中对象去配置细则〕**跳转 08-constraints-conditions**（带选中对象作入参），**本页不落授权**。未圈选对象在策略层一律默认拒绝（公理一），UI 不暗示"探到即可用"（场景 E8）。
5. 探测失败 → 抽屉 ErrorState 显脱敏缺口 + 可发起"代修"入口（把缺口转译为后续修正写调用，语义裁决仍在 daemon，场景 E3）。

> discover 是接入侧动作，**只在控制面呈现**；本页不提供任何把 discover 结果推给数据面/Agent 的路径（数据面只有 `postern_surface` 快照投影，不触达 discover，场景 E8）。

### 4.4 配置认证流程 auth_flow（写，并入 POST /v1/resources）

仅**会话来源**资源（如 `svc-*` 的 http adapter）显示。auth_flow 是**逐 tier** 声明，落 `resource_credential_tiers(resource,tier).auth_flow`（JSON）。

```
┌─ auth_flow: svc-order / tier=op ──────────────────────────┐
│ 流程类型 flow      ( form_login ▾ ) （form_login/basic/…）  │
│ 认证端点 endpoint  [ /api/login__ ]                         │
│ 会话注入 injection ( cookie ▾ )  name [ JSESSIONID ]        │
│ CSRF               ◉ 提取并回填   字段 [ _csrf____ ]         │
│ 刷新策略 refresh   ( session_renew ▾ )  硬过期 [fail_closed]│  ← on_hard_expire 锁 fail_closed
│ 凭据引用 credential_ref  vault://svc-order/op  （只读引用）  │
│ 二次验证 2FA       ☑ required  阶段[onboarding_only] otp    │  ← 需 2FA 时一次性 OTP 在场完成
│   └ 〔接入期完成 OTP〕  [ ______ ]  （人在场，单次）         │
└────────────────────────────────────────────────────────────┘
```

- **非敏感配置**（flow/endpoint/injection/csrf/refresh）写入 `auth_flow`；**敏感账号/长效会话**一律 `vault://` 引用，**永不入表、永不回显明文**。
- **2FA**：仅 `stage=onboarding_only`、`method=otp`；OTP 在此页一次性完成（接入期人在场）。UI **不假装能绕过 2FA**；若系统每次登录强制 2FA 且无长效会话，如实标注"网关只在单次会话有效期内运作，硬过期后数据面 fail-closed 拒绝并提示重新接入"（场景 E2，诚实边界）。
- `on_hard_expire` 固定 `fail_closed`（不可改为在线放行，与 L-12 同气质）。

### 4.5 接入期 tier⊆账号真实权限校验（系统侧，UI 呈现 + 阻断）

凭据录入后由 daemon 自动执行（用账号试探一次只读），UI 在接入校验结果视图呈现：

```
┌─ 接入校验: svc-order ────────────────────────────────────┐
│ ✓ ro  声明[observe,query] ⊆ 实测可达接口             PASS │  ← --allow 绿
│ ✗ op  声明[mutate] 但账号实测无写权限                 FAIL │  ← --deny 红 + 缺口
│        缺口: "op 声明含 mutate，账号无写权限"             │
│ 〔接入完成〕 ← 灰禁（有 FAIL 项不可完成接入）             │  ← 任一 FAIL 阻断
└──────────────────────────────────────────────────────────┘
```

「声明 ⊆ 真实」全 PASS 方可接入完成；任一 FAIL → **阻断接入完成**、高亮缺口、管理员须改声明或换账号重试（场景 E5，fail-closed）。该项同时纳入 `POST /v1/verify` 常规项（跳 04-verify 查看）。

### 4.6 停用/启用资源（写，POST /v1/resources，乐观锁）

- 〔停用〕**危险动作** → ConfirmDialog（"停用 `svc-crm` 将使其 Scope 内授权不可达，确认？"，显式勾选确认）→ `POST /v1/resources`（置 `enable_flag=0`，携 `version`）→ 刷新。
- 启用为对称操作，非危险，仍走摘要预览。

### 4.7 本页危险动作清单（一律 ConfirmDialog，基座统一）

| 危险动作 | 确认方式 |
|---|---|
| 接入含 `mutate/manage/destroy` tier 的资源（声明高危动词面） | 摘要预览 + ConfirmDialog（列出将声明的高危动词） |
| 停用资源（断其 Scope 可达性） | ConfirmDialog + 显式勾选 |
| 修订真实地址（改 transport_config 引用，影响所有连接） | ConfirmDialog（提示"将影响该资源全部建连"）|
| 触发 discover（真实连上后端、走真实凭据） | 弱确认（说明"将经 transport 真实连接探测"，非破坏性，不强阻断）|

> 本页**无** destroy 单格授权、freeze、shutdown、import 覆盖（那些在 grants/mode/system 页）；接入资源本身不直接扩权（仅声明面，授权落 03/08）。

---

## 五、复用的基座组件

| 基座组件 | 本页用途 |
|---|---|
| **AppShell / GlobalEmergencyBar** | 全局骨架 + 顶栏应急区（Freeze/mode/健康灯，常驻） |
| **DataTable** | 资源列表（排序/筛选/**强制分页**/空态/骨架/行操作菜单） |
| **ResourceCodeBadge** | `code` 列与详情标题（等宽 + adapter/transport 图标，永不显地址） |
| **CapabilityBadge** | tier 动词集、discover `capabilities` 展示（observe…destroy 色温递增，只读） |
| **FormDrawer / FormModal** | 接入/编辑抽屉（分段：基本/地址/tiers/auth_flow）；RHF+Zod 校验、摘要预览、409 提示 |
| **ConfirmDialog** | 停用、改地址、声明高危动词面、discover 弱确认 |
| **TtlBadge** | （会话来源）长效会话/刷新有效期临近过期转琥珀（若 auth_flow 暴露有效期元数据）|
| **JsonViewer**（只读） | auth_flow 非敏感配置的事实展示（等宽、可复制，不可编辑敏感项）|
| **EmptyState / ErrorState / LoadingSkeleton** | 三态（fail-closed，错误不显伪数据） |
| **VerifyItemRow**（引用） | 接入后红队自检结果（PASS/FAIL + gap_note）经本页入口跳 04-verify 呈现 |

**本页特有构成**（均用基座组件拼，不新增令牌）：

- **能力面探测视图**（§2.3）：ErrorState/CapabilityBadge + 圈选清单（复选）+ "发现≠授权" info/warn 横幅 + 跳转入口；圈选只产入参，不落授权。
- **接入校验结果视图**（§2.4/§4.5）：用 allow/deny 语义色 + VerifyItemRow 气质的 PASS/FAIL 行 + 缺口文案 + 阻断按钮（有 FAIL 时灰禁〔接入完成〕）。
- **auth_flow 子表单**（§4.4）：FormDrawer 内的分段表单 + 只读 `vault://` 引用 + 一次性 OTP 输入。

---

## 六、正常与异常预期（对照场景规格）

### 6.1 正常（场景 §4.1 逐步对齐）

| 场景步骤 | 本页动作 | 预期结果 |
|---|---|---|
| 步骤1 声明资源+transport/adapter | §4.2 接入 | `POST /v1/resources` 写 `resources` 表，回 `code` + `version`（乐观锁基线）；`db-main` engine_enforced=true；资源**无凭据无授权、默认拒绝一切**；一条 `policy_change` 审计可跳 |
| 步骤2 录真实地址（匿名化） | §2.2②地址输入 | 提交后回显 `vault://db-main/target`，**明文不入 policy.db、不出导出**；UI 永不回显明文地址 |
| 步骤4 声明 tier | §2.2③ tiers | 写 `resource_credential_tiers`，`(resource,tier)` 未删集内唯一；**每资源≥1 只读 tier**（前端 Zod 校验提示，后端权威） |
| 步骤5 配 auth_flow（含 2FA） | §4.4 | 逐 tier 非敏感配置入 `auth_flow`，敏感项 `vault://` 引用；第①档零交互，第③档接入期一次性 OTP |
| 步骤6 discover | §4.3 | 回 `CapabilitySurface`（capabilities+objects）只读事实；圈选跳 08 配细则；**不进数据面** |
| 步骤7 tier⊆真实校验 | §4.5 | 全 PASS 方可完成接入；结果呈现 |
| 步骤9 红队自检 | §五入口跳 04 | 经本页入口触发 `POST /v1/verify`，逐项 PASS/FAIL（本页不渲染九项细节，引用 04-verify） |

### 6.2 异常/边界（一律 fail-closed）

| 编号 | 触发 | 本页预期（fail-closed） |
|---|---|---|
| 加载失败 | `GET /v1/resources` 失败 | ErrorState，**不显伪表/不显旧缓存**；错误经脱敏 |
| 权限/不可达 | 控制面认证失败 | 整页受限态，不乐观假装；引导重连 daemon |
| 分页越界 | `page_no` 超末页 / `page_size`>200 | 后端钳制（缺省20/钳200）；前端按返回 `Page` 渲染，不本地造页 |
| **E1** 会话登录失败 | auth_flow 账号/流程错 | 接入**中止**，提示"会话登录失败，账号或认证流程配置不正确"，**不回显后端原始错误串/地址**；已录明文不生效 |
| **E2** 需 2FA 无人值守 | 第③档强制 2FA 未完成 OTP | 接入**不完成**，提示"该资源强制二次验证，须人在场完成 OTP"；不假装绕过、不静默降级 |
| **E3** 探测通路不可建 | discover 端口未发布/目标不可达 | discover 返回**失败+具体缺口**（脱敏，无真实端口/IP），可发"代修"；缺口未消解资源不进可用态 |
| **E4** 真实地址泄漏尝试 | 误把明文塞非引用字段 / 导出 / 回显诱导 | 非引用字段明文不被解析为地址（不进通路）；UI 永不显明文；回显诱导经 ScrubSet 擦净（数据面，verify 第8项覆盖） |
| **E5** tier⊄真实权限 | 声明动词>账号实权 | §4.5 **阻断接入完成**，报缺口（"ro 含 mutate 但账号无写权限"），须改声明/换账号 |
| **E6** vault locked | 录凭据/探测时保险箱未解锁 | 相关步骤 **fail-closed 失败**，提示"机密保险箱未解锁，无法写入/校验凭据"；不退化为无凭据接入 |
| **E7** 并发冲突（乐观锁） | 二人同改同一资源 | 后写者收 **409 Conflict**，提示"他人已改、请刷新重试"；不静默覆盖（见 §七） |
| **E8** discover 误当授权 / 数据面试探 | 以为探到即可用 / Agent 试触 discover | UI 横幅明示"发现≠授权"，未圈选默认拒绝；本页无任何把 discover 推数据面的路径 |
| **E9** daemon 重启 | 接入后重启 | 资源/tier/auth_flow/匿名化映射持久恢复；保险箱未解锁前数据面 fail-closed（同 E6）|
| **E10** 接入期审计写失败 | `policy_change`/`credential_event` 写失败 | 接入写**不被确认为成功**（三联动），提示"审计降级，接入未完成"，**不改本地视图**，无半态 |

---

## 七、与后端契约对齐

- **分页**：`GET /v1/resources` 用 `page_no/page_size`，**缺省 20、钳 200**（对齐 `page_query` / `DB_PAGINATION_MANDATORY`）；前端页大小选项止于 200，集合查询一律分页，不本地全量。
- **雪花 id 字符串**：资源 `id` 全程**字符串**，前端**绝不**当 number 解析（>2^53 丢精度）；详情展示按基座 3.4 等宽截断 + 悬浮全展 + 复制。管理员寻址用 `code`，写 `discover` 走 `{code}` 路径参数。
- **乐观锁版本**：编辑/停用/改 auth_flow 等写端点携带读取所得 `version`；不匹配返回 **409**，UI 明确"他人已改、请刷新重试"，期望 version 唯一来源是先前读取值（场景 E7 / F-6 / L-15）。
- **写三联动**：每次 `POST /v1/resources` = 事务 COMMIT + 快照重建 + `policy_change` 审计（L-14）；审计写失败 → 整体 fail-closed，UI 不显成功、不改视图（E10）。成功 toast 提示 `policy_rev` 前进且可跳对应 audit 事件。
- **匿名化/脱敏不变量**：`transport_config` 仅 `vault://{code}/target` 引用；UI **永不显真实地址、凭据明文、secret_hash**；tier 的 `secret_ref` 只显引用串不可点开；一切错误/discover 缺口文案先脱敏再呈现（不泄露 IP/端口/instance-id/域名）。
- **discover = 控制面专属**：`POST /v1/resources/{code}/discover` 返回 `CapabilitySurface{capabilities, objects}`，**只产事实不授权**；本页不暴露任何使其结果进入数据面/Agent 视野的路径（CONS-20）。
- **不暴露 Scope 外存在性**：列表/筛选只呈现控制面回报的资源；越权/缺失数据一律 fail-closed 不显伪条目，不旁证某资源是否存在。
- **fail-closed 全覆盖**：加载/错误/权限/校验缺口/409/vault locked/审计降级 一律呈现为受限/拒绝态，绝不乐观假装成功。
