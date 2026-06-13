# 05 · 授权矩阵 Grants

> 本页是 postern 控制台「修订规则」的核心页：把一个 Principal 的**生效授权**展开成 `Resource × Capability` 网格，让运维一眼看清「谁能在哪做什么、走哪个 tier、由哪条角色/临时授权赋予」，并在此页直接做**临时提权（elevate，带 TTL，到期自动回收）**与**临时授权吊销（revoke）**。本页严格复用 `00-设计系统与信息架构.md` 的令牌、组件与统一交互模式，不重定义、不偏离。纯设计，不含实现代码。

---

## 一、页面定位

**一句话**：以某个 Principal 为视角，把其「当前生效授权」展开为 `Resource × Capability` 判定矩阵（含角色展开格与临时授权格），并就地做临时提权 / 吊销——这是运维确认授权意图、收紧/放宽口子的核心页。

- **对应场景**：`docs/examples/03-权限分配与角色管理.md`（§3.2「Principal 授权页」展开矩阵与 your_grants 同源、§4 逐格验收）+ `docs/examples/06-动态权限调整与失控切断.md`（§2.1 临时提权带 TTL、§3.2「临时授权管理页」、§4.1-A 提权→热生效→到期回收、§4.2 异常）。
- **主要 API**：
  - `GET /v1/grants`（读：某 Principal 的展开授权矩阵 + 当前生效的临时授权清单；与 Agent 侧 `your_grants` 同源，零安全逻辑在前端）。
  - `POST /v1/grants/temp/elevate`（写·扩权：临时提权，**必带 TTL**）。
  - `POST /v1/grants/temp/revoke`（写·收权：主动吊销一条临时授权，`end_reason='revoked'`）。
- **本页不做**：不创建/编辑 binding、role、constraint（那是 06/07/08 页的事）。本页只**读全量展开** + **写临时授权两动作**。矩阵里的「持久格」是只读展开结果，要改持久授权需跳到对应页（提供跳转链接，但不在本页编辑）。

---

## 二、布局

本页是**「单 Principal 详情视图」**而非普通列表页——顶部是 Principal 选择器（聚焦一个主体），主体是授权矩阵（详情页骨架），底部是临时授权清单（带 TTL 倒计时的小型 DataTable）。主操作「Elevate（提权）」在右上。

```
┌────────────────────────────────────────────────────────────────────────────┐
│ AppShell 顶栏：品牌 · [GlobalEmergencyBar: Freeze 开关 + 全局模式徽章] · 健康灯 · 主题 │  ← freeze 生效时整页顶部红色脉冲横幅
├──────────┬─────────────────────────────────────────────────────────────────┤
│ 左侧导航  │  授权矩阵 Grants                                  [ + Elevate 提权 ] │  ← 标题 + 主操作(右上)
│ …        │                                                                   │
│ ▸授权     │  ┌─ Principal 选择 ──────────────────────────────────────────┐    │
│  ·Grants◀│  │ Principal: [ agent2 ▾ ]   policy_rev: 184   [⟳ 刷新]        │    │  ← 选定主体 + 当前快照 rev
│  ·Roles  │  └────────────────────────────────────────────────────────────┘    │
│  ·Bind…  │                                                                   │
│  …       │  ┌─ 生效授权矩阵 (Resource × Capability) ──────────────────────┐    │
│          │  │                                                              │   │
│          │  │  筛选: [资源代号⌕] [只看已授格☑] [含临时授权☑]  图例:✅持久 ⏱临时 ❌默认拒│   │
│          │  │ ┌──────────────┬──────┬──────┬──────┬──────┬──────┬──────┐  │   │
│          │  │ │ Resource ↓   │obser │query │mutate│execu │manage│destr │  │   │ ← 列=6 能力动词(CapabilityBadge 色温递增)
│          │  │ │  \  Capab.   │ -ve  │      │      │ -te  │      │ -oy  │  │   │
│          │  │ ├──────────────┼──────┼──────┼──────┼──────┼──────┼──────┤  │   │
│          │  │ │ ⬡docker-A    │ ✅ro │  ❌  │  ❌  │  ❌  │✅adm │  ❌  │  │   │ ← 行=Scope 内资源(ResourceCodeBadge)
│          │  │ │ ⬡db-main     │ ✅ro │ ✅ro │  ❌  │  ❌  │  ❌  │  ❌  │  │   │   格内: 决策 + 所选 tier 简称
│          │  │ │ ⬡redis-main  │ ✅ro │ ✅ro │✅rw  │  ❌  │✅adm │ ⏱adm│  │   │   ⏱=临时授权格(带 TtlBadge)
│          │  │ └──────────────┴──────┴──────┴──────┴──────┴──────┴──────┘  │   │
│          │  │                                                              │   │
│          │  │  〔点击任一格 → 右侧抽屉：该格的 provenance〕                  │   │
│          │  └──────────────────────────────────────────────────────────────┘   │
│          │                                                                   │
│          │  ┌─ 当前生效临时授权 (temp_grants) ────────────────────────────┐    │
│          │  │ ┌────────┬───────────┬──────┬──────────┬──────────┬───────┐│    │
│          │  │ │ id     │ resource  │ cap  │granted_at│ TTL 剩余 │ 操作  ││    │  ← 小型 DataTable(强制分页)
│          │  │ ├────────┼───────────┼──────┼──────────┼──────────┼───────┤│    │
│          │  │ │7300…23 │ redis-main│destroy│13:04:11 │⏱ 24:18  │[吊销] ││    │   TtlBadge: 临近过期转琥珀
│          │  │ └────────┴───────────┴──────┴──────────┴──────────┴───────┘│    │
│          │  │              ‹ page_no 1 · page_size 20 · 共 N ›             │    │
│          │  └──────────────────────────────────────────────────────────────┘   │
└──────────┴─────────────────────────────────────────────────────────────────┘

右侧抽屉(点格触发) ─ 格 provenance:                右侧 FormDrawer(点 Elevate 触发):
┌─────────────────────────────┐                  ┌─────────────────────────────┐
│ redis-main × destroy         │                  │ 临时提权 Elevate             │
│ 决策: ⏱ 临时授权 (allow)      │                  │ Principal: agent2 (锁定)     │
│ tier:  admin                 │                  │ Resource:  [ redis-main ▾ ] │
│ 来源:  temp_grant 7300…0123  │                  │ Capability:[ destroy   ▾ ] │
│ 授予:  13:04:11  by 运维X     │                  │ TTL:      [ 30 ] [分钟▾] *必填│
│ 到期:  ⏱ 13:34:11 (剩 24:18) │                  │ ───── 摘要预览 ─────         │
│ [立即吊销 revoke]            │                  │ 将给 agent2 在 redis-main 上 │
│                              │                  │ 临时授予 destroy，30 分钟后  │
│ (若为持久格则显示:)           │                  │ 自动回收(expires_at=墙钟)。  │
│ 决策: ✅ 持久 (allow)         │                  │ [取消]      [提权…(危险确认)]│
│ 角色: maintainer             │                  └─────────────────────────────┘
│ 细则: container_prefix=…(只读)│
│ → 去 Bindings 页修订          │
└─────────────────────────────┘
```

**骨架要点**：
- 顶部 **Principal 选择器**是本页的「主键」——切换主体即重查矩阵。当前 `policy_rev` 常驻显示（可对账、是乐观锁/刷新提示的锚点）。
- 矩阵为**密集网格**：行=Scope 内资源（`ResourceCodeBadge` 等宽代号 + adapter 图标），列=6 个固定能力动词（表头用 `CapabilityBadge` 色温递增 observe→destroy）。格内只放「决策符号 + tier 简称」，详情进抽屉，保持密度。
- 临时授权清单是独立的小型 `DataTable`（强制分页），与矩阵里的 `⏱` 格一一对应。

---

## 三、数据与状态

### 3.1 展示字段（全部来自 daemon，前端零推导）

**矩阵格**（每个 `(resource, capability)` 单元）来源于 `your_grants` 同源投影（`GET /v1/grants`），后端 `DenyResponse.your_grants` 的类型是 `BTreeMap<ResourceCode, Vec<String>>`（资源代号 → 能力名字符串列表）。本页扩展读到的每格携带：
| 字段 | 来源 | 呈现 |
|---|---|---|
| `resource`（代号） | snapshot 展开 | `ResourceCodeBadge`，等宽，**永不显真实地址** |
| `capability`（observe…destroy） | 固定 6 列 | `CapabilityBadge`，固定色温配色，只读 |
| 决策（持久 allow / 临时 allow / 默认拒绝） | 格存在性 + 是否来自 temp_grant | `DecisionBadge` 风格符号：`✅`持久 / `⏱`临时 / `❌`默认拒绝（缺格） |
| `tier`（所选引擎账号简称，如 ro/rw/adm） | snapshot tier 选择（步骤 [6]） | 格内等宽小字；**只显 tier 名，不显账号/凭据** |
| provenance（role 或 temp_grant id） | `GrantCell.role` / temp_grants 行 | 仅在抽屉里展开 |

**临时授权行**（`temp_grants` 表，§schema 5.2）：`id`（雪花字符串，等宽截断）、`resource`（代号）、`capability`、`granted_at`、`expires_at`（绝对墙钟，渲染为 `TtlBadge` 倒计时「剩余」）、（`ended_at`/`end_reason` 仅对已结束行——本清单默认只列**当前生效**：`expires_at >= now 且 ended_at 为空`）。每行携带 `version`（乐观锁，吊销时回传）。

**Principal 维度**：当前选中的 `principal`、当前 `policy_rev`（快照修订号，对账锚点）。

### 3.2 三态呈现（fail-closed）

| 状态 | 呈现 | fail-closed 立场 |
|---|---|---|
| **加载中** | 矩阵区与临时授权清单各用 `LoadingSkeleton`（网格骨架/表格骨架）。**绝不**先渲染上一主体的旧矩阵冒充新主体。 | 不确定 → 不展示授权事实 |
| **错误**（`GET /v1/grants` 失败 / daemon 不可达 / 健康灯灭） | `ErrorState`，红色，明文「无法读取授权矩阵：<错误事实>」。**不显任何格、不显伪 ✅**——整页授权视为「未知=不可信」，不得让运维据此误判某主体已被收权。`Elevate`/吊销按钮禁用。 | 读失败 → 整矩阵不可见，绝不显伪数据（原则 1） |
| **空**（该 Principal 无任何授权格） | `EmptyState`：「agent? 当前无任何生效授权（默认拒绝世界）」。这是**合法的安全态**（公理一：无授权格=全拒），非错误。仍显主操作引导（可对其 Elevate）。 | 空集=最安全，如实呈现 |
| **临时授权清单空** | 清单区 `EmptyState`：「当前无生效临时授权」。矩阵照常显示持久格。 | — |

特别地，矩阵中**某些资源整行不出现**是正常的：未被任一 scope 选中的资源、或已逻辑删除的资源（悬挂引用），daemon 的 `your_grants` 根本不返回它们（场景 03 §4.1 步 3、异常 G）。前端**不补行、不显「该资源不存在」**——这正是 `DENY_RESPONSE_SCOPE_BOUNDED` 在 UI 的体现：「Scope 外但存在」与「根本不存在」对前端不可区分（见 §七）。

---

## 四、交互流

### 4.1 读路径

1. **选 Principal** → 系统 `GET /v1/grants`（principal 维度）→ 渲染矩阵 + 临时授权清单 + `policy_rev`。切换主体即重查（清空旧视图走骨架）。
2. **筛选/排序**（前端纯展示层，不触安全逻辑）：按资源代号筛行、「只看已授格」隐藏全 `❌` 行、「含临时授权」开关是否高亮 `⏱` 格。分页只作用于临时授权清单（矩阵行数 = Scope 内资源数，通常可一屏；如超量按资源代号分页，仍走 `page_no/page_size`）。
3. **点矩阵格** → 右侧抽屉显示该格 **provenance**（持久格：role 名 + 只读细则摘要 + 跳 Bindings/Constraints 页的链接；临时格：temp_grant id + granted_at/expires_at + 立即吊销）。抽屉**只读展开 daemon 回报的事实**，不在此编辑持久授权。
4. **TTL 倒计时**：`TtlBadge` 前端按 `expires_at`（绝对墙钟）本地倒数仅作**呈现**；**正确性不依赖前端计时**——临近 0 自动转琥珀，到点显示「已过期」并提示「刷新以同步」。前端到 0 不擅自把格改判为拒绝（求值生效以 daemon 墙钟为准，前端只提示刷新）。

### 4.2 写路径（走基座统一写流程：表单 → 摘要预览 → 危险确认 → 刷新 → 成功/失败/409）

本页有**两个写动作，均为危险动作**（扩权 / 收权），一律经 `ConfirmDialog`。

**动作 A · Elevate（临时提权）— 扩权，高危**
1. 右上「+ Elevate」→ 打开 `FormDrawer`。Principal 锁定为当前选中主体。
2. 表单（RHF+Zod）：`Resource`（下拉，限该 Principal Scope 内可选资源）、`Capability`（6 动词下拉）、**`TTL`（必填**，数值 + 单位；Zod 校验 `>0`，**缺 TTL 不可提交**——对齐场景 06 异常 13「提权必须带 TTL」）。
3. **摘要预览**（提交前）：「将给 `<principal>` 在 `<resource>` 上临时授予 `<capability>`，于 `<expires_at 预估墙钟>`（now+TTL）后自动回收。这会**扩大**该主体的授权面。」
4. **危险确认** `ConfirmDialog`：因是扩权动作（尤其 `destroy`/`manage`），需显式勾选「我确认临时扩大授权」或输入资源代号确认。
5. 提交 `POST /v1/grants/temp/elevate` → 成功（`201`）：提示「已临时提权，policy_rev 前进至 N、可跳 audit（`kind=policy_change`）」，失效刷新矩阵（新增 `⏱` 格）+ 临时授权清单（新增行带倒计时）。
6. 失败：红色 `ErrorState`，**不改本地视图**（矩阵不新增伪格）。

**动作 B · Revoke（主动吊销临时授权）— 收权，需确认**
1. 入口二选一：临时授权清单行的「吊销」按钮，或点 `⏱` 格 → 抽屉「立即吊销」。
2. **摘要预览**：「将立即吊销 `<principal>` 在 `<resource>` 上的临时 `<capability>`（id `7300…0123`），`end_reason='revoked'`（人工，区别于到期 expired）。该口子立即关闭。」
3. **危险确认** `ConfirmDialog`：收权虽是安全方向，但属破坏性变更（影响在跑/将跑请求），需确认。
4. 提交 `POST /v1/grants/temp/revoke`（**携带该行读取时的 `version`**）→ 成功：提示 policy_rev 前进 + 可跳 audit；失效刷新（该 `⏱` 格回落为 `❌`、清单行移除或标记已结束）。
5. 失败/409：见 §六。

**本页危险动作清单及确认方式**：
| 危险动作 | 类别 | 确认方式 |
|---|---|---|
| Elevate 临时提权 | 扩权（最高危，尤其 destroy/manage） | 摘要预览 + `ConfirmDialog`（勾选/输代号确认）+ TTL 必填 |
| Revoke 临时授权 | 收权（破坏性，影响在飞） | 摘要预览 + `ConfirmDialog` 确认 |

> freeze / 吊销凭证等更全局的应急动作不在本页发起（freeze 在顶栏 `GlobalEmergencyBar`，吊销凭证在 10 主体与凭证页），本页只处理**临时授权**这一粒度。

---

## 五、复用的基座组件

本页**不引入任何自定义安全逻辑组件**，全部用基座组件拼装（SPA 零安全逻辑，daemon 回报什么渲染什么）：

| 基座组件 | 本页用途 |
|---|---|
| `AppShell` | 全局骨架（顶栏 + 左导航「授权 › Grants」高亮 + 内容区 + 右抽屉位） |
| `GlobalEmergencyBar` | 顶栏常驻 Freeze 开关 + 全局模式徽章；freeze 生效时整页红色脉冲横幅（本页只读它、不在此切模式） |
| `DataTable` | 临时授权清单（强制分页 `page_no/page_size` 缺省 20 钳 200、空态、加载骨架、行操作「吊销」、雪花 id 列等宽截断） |
| `ResourceCodeBadge` | 矩阵行首资源代号（等宽 + adapter/transport 图标，永不显真实地址） |
| `CapabilityBadge` | 矩阵列表头 6 动词（固定色温配色，只读） |
| `DecisionBadge` | 格内/抽屉内决策呈现（allow 语义色 + 图标；本页扩展出「持久 ✅ / 临时 ⏱ / 默认拒绝 ❌」三态符号，色不仅靠色还带图标，AA 可达） |
| `TtlBadge` | 临时授权 TTL 倒计时（`expires_at` 绝对墙钟，临近过期转琥珀）——矩阵 `⏱` 格与清单「TTL 剩余」列 |
| `FormDrawer` | 「Elevate 提权」表单（RHF+Zod、TTL 必填校验、提交前摘要预览、409 提示、成功提示 policy_rev 前进） |
| `ConfirmDialog` | Elevate（扩权）与 Revoke（收权）的二次危险确认 |
| `EmptyState / ErrorState / LoadingSkeleton` | 矩阵与临时授权清单的三态（fail-closed，错误态不显伪数据） |

**本页特有的组件构成（用基座组件拼，非新安全逻辑）**：
- **GrantMatrix（授权矩阵网格）**：一个二维网格，行用 `ResourceCodeBadge`、列头用 `CapabilityBadge`、格用 `DecisionBadge`(三态) + 等宽 tier 简称 +（临时格）`TtlBadge`。纯展示组件，渲染 `GET /v1/grants` 回报的格集合，**不计算任何授权**——缺格即渲染 `❌`（缺格=默认拒绝，公理一，无需推导）。
- **GrantCellDrawer（格 provenance 抽屉）**：用基座抽屉位拼，只读展开一格的来源（role 名 / temp_grant id / 只读细则摘要 / 时间字段 / 跳转链接 / 吊销入口）。

---

## 六、正常与异常预期

### 6.1 正常操作（对照场景 03 §4.1 / 场景 06 §4.1-A）

| 步骤 | 操作 | 预期结果 |
|---|---|---|
| 看 agent2 矩阵 | 选 `agent2` | 矩阵逐格与场景 03 §4.1 步 3 一致：`docker-A`={observe→ro✅, manage→adm✅}、`db-main`={observe✅, query→ro✅}、`redis-main`={observe✅,query✅,mutate→rw✅,manage→adm✅}；`svc-*`/`mq-main` **整行不出现**（未被 scope 选中=默认拒绝）。 |
| 同资源差异化 | 对比 agent2/agent3 的 `docker-A` 行 | agent2 该行有 `manage✅`，agent3 该行只有 `observe✅`、`manage` 列为 `❌`——「管理」在两主体处精确正交（场景 03 需求 1、异常 D）。 |
| 提权 | Elevate `agent2` / `redis-main` / `destroy` / TTL `30m` | `201`，矩阵 `redis-main × destroy` 出现 `⏱adm` 格、清单新增行 `TTL 剩余 29:5x` 倒数；policy_rev 前进、可跳 `policy_change` audit（场景 06 §4.1-A 步 1）。 |
| TTL 倒计时 | 等待 | `TtlBadge` 临近过期转琥珀；到 0 显「已过期·请刷新」，刷新后该 `⏱` 格回落 `❌`（场景 06 §4.1-A 步 3：到期即不可见）。 |
| 主动吊销 | 对该临时行点「吊销」 | 摘要预览→确认→`200`，该格回落 `❌`、清单行标 `revoked`；policy_rev 前进（场景 06 §4.1-A 步 5）。 |

### 6.2 异常与边界（一律 fail-closed）

| 场景 | 触发 | 预期（fail-closed） |
|---|---|---|
| **加载失败** | `GET /v1/grants` 失败 / daemon 不可达 | 整矩阵 `ErrorState`，**不显任何格**；不得让运维据此误判主体已被收权；Elevate/吊销禁用（原则 1）。 |
| **权限/越权数据缺失** | 某资源被逻辑删除（悬挂引用，场景 03 异常 G） | 该资源行**完全不出现**，前端不补行、不显「曾存在」（不暴露存在性）。 |
| **空集是安全态** | 该 Principal 无授权格 / 某 scope 展开为空（场景 03 异常 B） | `EmptyState`「无生效授权（默认拒绝）」，**非错误、不报红**；空=最安全。 |
| **分页** | 临时授权清单 / 资源行超量 | 强制分页，`page_no/page_size` 缺省 20、钳 200（见 §七）。 |
| **TTL 缺失** | Elevate 表单未填 TTL | Zod 前端拒提交「提权必须带 TTL」；即便绕过，daemon 端点亦拒（场景 06 异常 13）。前端只是便利，硬拒在 daemon。 |
| **乐观锁 409** | 吊销时携带过期 `version`（他人/另一标签页已改） | 原样呈现 `409 Conflict`：「他人已改该临时授权，请刷新重读最新 version 再试」，**不静默重试、不静默覆盖、不改本地视图**（场景 06 异常 10）。 |
| **提权后到期但 sweeper 未跑** | 到 `expires_at` 后刷新 | daemon 求值以墙钟二次校验，该格已不可见 → 矩阵刷新即回落 `❌`（场景 06 异常 1：不依赖 sweeper 时序）。前端只如实刷新，不自行判定。 |
| **freeze 期间** | 顶栏 freeze 生效 | 本页矩阵照常显示（授权矩阵是 RBAC 展开，与 mode 正交）；红色脉冲横幅提示全局冻结。提权仍可写入（口子写入），但 freeze 下求值一切被拒——本页不替系统解释，只如实显模式横幅 + 授权事实。 |
| **写失败不改视图** | Elevate/Revoke 任一三联动失败 | 红色错误，本地矩阵/清单**不变**（绝不乐观假装成功，原则 1）。 |

---

## 七、与后端契约对齐

| 契约硬约束 | 本页落地 |
|---|---|
| **分页** `page_no/page_size`（缺省 20、钳 200，`DB_PAGINATION_MANDATORY`） | 临时授权清单 `DataTable` 强制分页；资源行超量亦走分页。前端不发未分页的集合查询。 |
| **雪花 id 恒字符串、不丢精度** | temp_grant `id`、principal/resource id 全程当**字符串**处理，等宽截断（`7300…0123`）+ 悬浮全展 + 复制；前端**绝不**当 number 解析。 |
| **写端点乐观锁版本** | Revoke 携带读取时的临时授权 `version`；409→提示刷新重读，不静默重试（schema `temp_grants.version`）。Elevate 为插入新行，无前置 version。 |
| **deny 逐字段、不泄露存在性**（`DENY_RESPONSE_SCOPE_BOUNDED`） | 矩阵即 `your_grants` 同源投影：只显该 Principal **自身**授权世界（`BTreeMap<ResourceCode, Vec<String>>`），**绝不**枚举他人/全局资源、绝不查目标是否存在。Scope 外/不存在资源对前端不可区分（不补行、不显存在性）。 |
| **凭证/资源脱敏** | 矩阵只显资源**代号**与 tier **名**；**永不**显真实地址、凭据明文、`secret_hash`、引擎账号细节。 |
| **TTL=绝对墙钟 `expires_at`** | `TtlBadge` 按 `expires_at`（24 长度时间串）倒数，仅呈现；正确性以 daemon 求值时刻墙钟为准，前端到 0 只提示刷新、不擅自改判。 |
| **temp_grants 终态字段二分** | 吊销=`end_reason='revoked'`（人工），到期=`expired`（sweeper）；本页吊销动作语义对应 revoked，清单仅列**生效**行（`ended_at` 为空）。 |
| **三联动 + 热生效** | 每个写成功后提示「事务 COMMIT + 快照重建（policy_rev 前进）+ 审计（`policy_change`）」三联动，并提供跳 audit 链接（对账锚点 policy_rev）。 |
| **写入仅控制面** | 本页所有写经 `/v1/grants/temp/*` 控制面端点；SPA 经桌面壳 over UDS，浏览器从不直连 `control.sock`；前端零安全逻辑（8.12）。 |
