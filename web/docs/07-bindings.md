# 07 · 绑定 Bindings

> 本页详细设计在《设计系统与信息架构》（`00-...md`）基座之上展开：复用其令牌、核心组件库与统一交互模式，**不重定义、不偏离**。本页只描述绑定页的布局、状态、交互流与所用组件/端点。**纯设计，不含实现代码。**

---

## 一、页面定位

**一句话**：绑定页把"Principal —绑定→ Role × Scope"落成可查、可建的授权辖区清单——运维在此声明"哪个主体、以哪个角色、管哪片资源"，并即时预览选择器在快照构建时展开为哪些具体资源代号。

- **对应场景**：`docs/examples/03-权限分配与角色管理.md`（步骤 2/3/4 给 agent1/2/3 挂 binding；异常 B 空集、C 选择器不可解析、F 乐观锁 409、G 悬挂引用、H 主体/角色缺失）。
- **主要 API**：
  - `GET /v1/bindings`（列表，强制分页）
  - `POST /v1/bindings`（创建一条 binding = role × scope，携带乐观锁 version）
  - `GET /v1/grants`（展开预览 / 跳转到 Principal 的展开矩阵，与 `your_grants` 同源）
- **心智边界**：绑定页是"**规则编辑器**"——绑定是持久策略，写入即热生效（事务 COMMIT → 快照重建 → policy_rev 前进）。本页**不展开**对象细则（constraint）与条件（condition），那些在 `08-constraints-conditions.md`；本页**不直接选 tier**（tier 由引擎按动词自动落点，仅在展开预览处只读呈现）。SPA 自身**零安全逻辑**：所有展开、合法性裁决都在 daemon，前端只渲染 daemon 回报的事实。

---

## 二、布局

遵循基座"列表页统一骨架"：标题 + 主操作（右上）+ 筛选条 + DataTable（强制分页）+ 行操作 + 右侧 FormDrawer 创建。绑定的"创建即预览展开"特性，体现在 FormDrawer 内的**展开预览块**与详情抽屉的**展开结果**。

### 2.1 列表页骨架

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ AppShell 顶栏：品牌 · daemon 健康灯 · 全局模式徽章 · [Freeze] · 主题切换         │
├────────────┬─────────────────────────────────────────────────────────────────┤
│ 左侧导航    │  绑定 Bindings                                  [ + 新建绑定 ]     │
│  观测       │  Principal —绑定→ Role × Scope。选择器在快照构建时展开为资源集。  │
│  授权 ▸     │ ┌─筛选条────────────────────────────────────────────────────────┐ │
│   授权矩阵   │ │ [Principal ▾] [Role ▾] [Scope 类型 ▾ selector|resource] [/搜索]│ │
│   角色       │ └───────────────────────────────────────────────────────────────┘ │
│  ▸绑定◂     │ ┌─DataTable────────────────────────────────────────────────────┐ │
│   细则       │ │ id        Principal  Role        Scope            展开  ver ⋮ │ │
│   条件       │ │ 7300…0a1  agent2     maintainer  selector all=host:A,        │ │
│   拒绝指引   │ │                                  kind:docker      1 资源  3  ⋮ │ │
│  接入 ▸     │ │ 7300…0a2  agent2     observer    resource db-main 1 资源  0  ⋮ │ │
│  系统 ▸     │ │ 7300…0a3  agent2     maintainer  resource redis-main 1资源 0  ⋮ │ │
│            │ │ 7300…0b7  agent1     observer    selector all=env:prod 7资源 1 ⋮ │ │
│            │ │ 7300…0c4  agent3     log-observer selector all=host:B  4资源 0 ⋮ │ │
│            │ │ 7300…0c9  agent3     observer    selector all=env:staging 0资源 2⋮│ │  ← 空集，琥珀标注
│            │ ├───────────────────────────────────────────────────────────────┤ │
│            │ │ 共 N 条   ◂ 1 2 3 ▸   每页 [20 ▾]                              │ │
│            │ └───────────────────────────────────────────────────────────────┘ │
└────────────┴─────────────────────────────────────────────────────────────────┘
```

- **DataTable 列**：`id`（雪花，等宽截断 `7300…0a1` + 悬浮全展 + 复制）、`Principal`（代号，可跳 `10-principals`）、`Role`（角色名，可跳 `06-roles`）、`Scope`（类型徽章 + spec 文本，selector 等宽显示 spec、resource 显示 `ResourceCodeBadge` 列表）、`展开`（展开后资源**数量**徽章，0 资源标琥珀"无匹配"）、`ver`（当前 version，供修改回传乐观锁）、`⋮`（行操作菜单）。
- **行操作菜单**（`⋮`）：`查看展开`（打开详情抽屉）、`查看 grants`（跳 `05-grants` 该 Principal 的展开矩阵）、`删除绑定`（危险动作，逻辑删除 `delete_flag=1`，ConfirmDialog）。
- **筛选条**：按 `Principal` / `Role` / `Scope 类型`（selector | resource）过滤；`/` 聚焦搜索框（渐进增强）。筛选只改查询参数、不改本地视图。

### 2.2 创建抽屉（FormDrawer · role × scope · 实时展开预览）

```
                         ┌── 新建绑定 ─────────────────────────────────┐
                         │ Principal *  [ agent2                    ▾ ] │
                         │ Role *       [ maintainer                ▾ ] │
                         │ Scope 类型 * (○) selector   ( ) resource     │
                         │ ─ selector 模式 ───────────────────────────  │
                         │   匹配标签（全部满足 all）                    │
                         │   [ host  ][ A        ] [×]                   │
                         │   [ kind  ][ docker   ] [×]                   │
                         │   [ + 增加一行 host:/env:/kind: ]            │
                         │   规约预览（机器事实，等宽）：                 │
                         │   {all:[{key:"host",value:"A"},              │
                         │         {key:"kind",value:"docker"}]}         │
                         │ ─ resource 模式（切换后显示）──────────────  │
                         │   选择资源代号（多选）                         │
                         │   [ db-main ×] [ + 添加 ]                     │
                         │ ─ 展开预览（只读，daemon 回报）────────────  │
                         │   ⟳ 当前匹配 1 个资源：                       │
                         │   [docker-A]  ← ResourceCodeBadge            │
                         │   ▸ 在该 Role 下将授予的 (资源×动词) 预览     │
                         │     docker-A: observe(logs) · manage(admin)  │
                         ├───────────────────────────────────────────  │
                         │           [ 取消 ]   [ 预览摘要并创建 ]       │
                         └─────────────────────────────────────────────┘
```

- **selector 行编辑器**：每行一个 `key:value`（key 限 `host` / `env` / `kind` 三类语义前缀，下拉受控；value 自由文本）。多行 = `{all:[...]}`（全部满足）。下方"规约预览"以等宽只读呈现将提交的 JSON spec（`JsonViewer`）——所见即所发。
- **resource 模式**：多选资源代号（`ResourceCodeBadge`），对应 `Scope::Resources`。
- **展开预览块**：键入/切换 scope 后，前端经只读探测把当前 spec 交 daemon 预演展开，回报"当前匹配 N 个资源 + 资源代号列表"，并可展开"在该 Role 下将授予的 (资源×动词)"预览（与 `your_grants` 同源、只读）。展开为 **0** 资源时此处明示"展开为 0 个资源（无匹配标签）"（异常 B 的 fail-closed 提示，非报错）。

### 2.3 详情抽屉（查看展开）

行 `查看展开` 打开右侧只读抽屉：binding 元数据（id / principal / role / scope 类型 + spec / version / 创建审计指针）+ **展开结果**（当前快照下该 binding 贡献的资源代号集，`ResourceCodeBadge` 列表）+ "贡献的授权格"（资源 × 动词，每格注所选 tier，只读，与 `GET /v1/grants` 同源）。空集时显示 `EmptyState`："展开为 0 个资源（无匹配标签）——该 binding 当前不授予任何资源"。

---

## 三、数据与状态

### 3.1 展示字段（均为 daemon 回报的事实，前端零推导）

| 字段 | 来源 | 呈现 |
|---|---|---|
| binding `id` | 雪花字符串 | 等宽截断 + 复制；**绝不当 number** |
| `principal` | 代号 | 文本 + 跳转 `10-principals` |
| `role` | 角色名 | 文本 + 跳转 `06-roles`；角色有效动词集（含继承展开）在详情处只读 |
| `scope.kind` | `selector` / `resource` | 类型徽章（info 蓝 / 中性） |
| `scope.spec` | selector 的 `{all:[{key,value}]}` 文本 / resource 代号集 | selector 等宽 `JsonViewer`；resource 用 `ResourceCodeBadge` |
| 展开资源数 / 资源集 | 快照构建展开结果（daemon） | 数量徽章 + 代号列表；**0 = 琥珀"无匹配"** |
| 贡献的 (资源×动词) + tier | 与 `your_grants` / `GET /v1/grants` 同源 | `CapabilityBadge`（固定色温）+ tier 名（只读） |
| `version` | 乐观锁版本 | 数字；写回传的唯一来源 |
| 创建审计指针 | `policy_change` 事件 | 可跳 `02-audit`（按 `policy_change` 过滤） |

**永不展示**：真实地址、`secret_hash`、凭据明文（绑定页本就不涉及凭据，但纪律一致）。资源一律以代号呈现。

### 3.2 三态（fail-closed）

- **加载中**：`LoadingSkeleton` 占位表格行；展开预览块显示"展开计算中…"骨架，**绝不**先乐观显示一个推测的资源集。
- **错误态**：`ErrorState`（红），列表区**不显伪数据**；展开预览拿不到 daemon 回报时显示"无法计算展开——拒绝按未授权对待"，**不**回退成"展示全部资源"（fail-closed：不确定即受限）。
- **空态**：
  - 列表无 binding → `EmptyState` + 主操作引导（"还没有绑定，新建第一条绑定为主体分配辖区"）。
  - 单 binding 展开为 0 资源 → 非错误，琥珀"无匹配标签"事实提示（异常 B）。
- **权限/越权数据缺失**：若某 Principal/资源对当前操作者不可见，列表只呈现可见行，**不**提示"还有 N 行被隐藏"（不泄露存在性，与 deny 逐字段不泄露存在性同纪律）。

---

## 四、交互流

所有写操作走基座"写操作统一流程"：表单（RHF+Zod）→ 提交前**摘要预览** →（危险则）确认 → 失效刷新 → 成功提示（policy_rev 前进 + 可跳 audit）/ 失败红色错误（不改视图）/ 409 冲突（提示刷新）。

### 4.1 创建绑定（主路径）

1. **动作**：点 `+ 新建绑定` → FormDrawer。选 Principal、Role、Scope 类型；selector 模式逐行填 `host:/env:/kind:` 标签，或 resource 模式多选代号。
2. **实时展开预览**：每次 spec 变化，前端经只读探测请求 daemon 预演展开 → 回报"匹配 N 个资源 + 代号列表 + 贡献的 (资源×动词) 预览"。**系统响应**：N>0 显示资源集；N=0 显示琥珀"展开为 0 个资源（无匹配标签）"；spec 不可解析显示红色"选择器语法不可解析——该绑定将不授予任何资源"（异常 C，预演即 fail-closed）。
3. **提交前摘要预览**：点 `预览摘要并创建` → 摘要卡：`principal=agent2`、`role=maintainer`、`scope=selector {all:[host:A,kind:docker]}`、`展开=[docker-A]`、`将新增授权格 docker-A:{observe→logs, manage→admin}`。**预期结果**：运维确认意图与展开一致后才落库。
4. **落库**：确认 → `POST /v1/bindings`（携带本次 Principal 当前 `version` 参与乐观锁）。**系统响应**：
   - **成功** → 绿色提示"已创建，policy_rev 前进至 R"，列表失效刷新、新行出现、可跳对应 `policy_change` audit；FormDrawer 关闭。
   - **失败** → 红色 `{error:{code,message}}`（如 422 主体/角色不存在、选择器非法），**不改本地视图**、抽屉保留输入。
   - **409 冲突** → "他人已改、请刷新重试"——不静默覆盖、不静默重试，提示重新读取最新 version 再提交（异常 F）。

### 4.2 删除绑定（危险动作）

1. **动作**：行 `⋮ → 删除绑定`。
2. **危险确认**（`ConfirmDialog`）：摘要"将删除 binding `7300…0a1`（agent2 · maintainer · selector all=host:A,kind:docker），删除后 agent2 在 docker-A 上的 manage/observe 授权随之消失（缩权方向）"。需**显式勾选**"我已确认缩权范围"或输入确认。逻辑删除（`delete_flag=1`），非物理删。
3. **系统响应**：`POST`/`DELETE` 语义携带 `version` → 成功提示 policy_rev 前进 + 失效刷新；409 提示刷新；失败红色不改视图。

### 4.3 本页危险动作清单（一律 ConfirmDialog）

| 危险动作 | 确认方式 | 方向 |
|---|---|---|
| **删除绑定** | ConfirmDialog + 显式勾选/输入；摘要列出受影响的 (资源×动词) | 缩权（仍需确认，避免误删扩大或意外撤销） |
| **创建绑定（扩权）** | 提交前摘要预览 + 展开结果确认（默认非"高危红框"，但 selector 展开较宽时摘要醒目提示"将新增 N 个资源的授权格"） | 扩权 |

> freeze / shutdown / 吊销凭证等全局高危动作不在本页发起（在顶栏应急区与对应页），但顶栏 `GlobalEmergencyBar` 在本页仍常驻可达。

---

## 五、复用的基座组件

| 组件 | 本页用途 |
|---|---|
| **AppShell** | 全局骨架；本页挂在"授权"导航组下 |
| **GlobalEmergencyBar** | 顶栏常驻（Freeze / 全局模式徽章） |
| **DataTable** | 绑定列表：列排序/筛选、**强制分页**（20/钳 200）、空态、加载骨架、行操作菜单；雪花 id 列等宽截断 |
| **FormDrawer** | 新建绑定：RHF+Zod 校验、提交前摘要预览、扩权摘要、409 冲突提示、成功提示 policy_rev 前进 |
| **ConfirmDialog** | 删除绑定的危险确认（显式勾选/输入） |
| **ResourceCodeBadge** | 展开结果与 resource 模式的资源代号（等宽 + adapter/transport 小图标；永不显真实地址） |
| **CapabilityBadge** | "贡献的 (资源×动词)"预览中的动词徽章（固定色温，只读） |
| **JsonViewer** | selector spec 的 `{all:[{key,value}]}` 等宽只读预览（所见即所发） |
| **EmptyState / ErrorState / LoadingSkeleton** | 三态：列表空/单 binding 空集/加载/错误，fail-closed 不显伪数据 |
| **TtlBadge** | 不直接使用（绑定是持久策略，无 TTL；临时授权/destroy 单格在 `05-grants`） |

### 本页特有的构成（用基座组件拼，非新组件）

- **ScopeEditor**（FormDrawer 内）：selector 行编辑器（受控 `host:/env:/kind:` 下拉 + value 输入 + 增删行）切换 resource 多选；底部挂 `JsonViewer` 实时显示将提交的 spec。**零展开逻辑**——展开由 daemon 回报。
- **ExpansionPreview**（FormDrawer / 详情抽屉内）：`ResourceCodeBadge` 列表 + 数量徽章 + 可折叠的 `CapabilityBadge` 矩阵；N=0 显示琥珀"无匹配"，不可解析显示红色 fail-closed 文案。
- **ScopeCell**（DataTable 列内）：selector 用截断 `JsonViewer`、resource 用 `ResourceCodeBadge` 列表，统一列宽。

---

## 六、正常与异常预期

### 6.1 正常操作（对照场景 §4.1 步骤 2/3/4/7）

| 操作 | 预期结果 |
|---|---|
| 给 agent1 建 `observer × selector all=env:prod`（步骤 2） | 创建成功、policy_rev 前进；列表新行展开数 = 7（带 `env:prod` 的全部资源）；详情/grants 预览每资源仅 {observe} 或 {observe,query}（docker-A 无 query → 仅 observe） |
| 给 agent2 建三条 binding（步骤 3：maintainer×selector A 上 docker、observer×resource db-main、maintainer×resource redis-main） | 三行写入；展开：docker-A=1 资源 {observe,manage}、db-main {observe,query}、redis-main {observe,query,mutate,manage}；未被任一 scope 选中的 `svc-*`/`mq-main` **不出现**在 agent2 的 grants（默认拒绝） |
| 给 agent3 建三条（步骤 4：operator×业务、observer×docker-A、log-observer×host:B） | docker-A 对 agent3 仅 {observe}（与 agent2 的 {observe,manage} 精确正交，"管理"不串味）；B 上 log-observer 各资源仅 {observe}、query 标 ❌（与 agent2 db-main 的 query 正交） |
| 修改/收窄某 binding（步骤 7，携带读取时 version） | 事务 COMMIT → 快照重建 → policy_rev 前进；grants 立即按新策略；成功提示可跳 `policy_change` audit；**无需重启、无需 Agent 配合** |
| 展开预览与 `GET /v1/grants` 核对（步骤 6） | FormDrawer/详情的展开矩阵与 `05-grants` 该 Principal 矩阵逐格一致（✅ 注 tier、❌ 默认拒绝） |

### 6.2 异常 / 边界（对照场景 §4.2，一律 fail-closed）

| 触发 | 预期呈现（fail-closed） |
|---|---|
| **加载失败**（列表 / 展开预览拿不到 daemon 回报） | `ErrorState` 红、不显伪数据；展开预览显示"无法计算展开——按未授权对待"，**不**回退成"展示全部资源" |
| **B · 选择器展开为空集** | 写入仍成功（语法合法）；该 binding 展开数 = 0，列表/详情琥珀"展开为 0 个资源（无匹配标签）"；**不报错、不放行**；后续有资源被打上该标签则下次快照自动纳入 |
| **C · 选择器语法不可解析** | 预演阶段即红色"选择器语法不可解析——将不授予任何资源"；逐条 API 提交回 422、**不落库**；绝不"尽力解析"出一个意外宽的授权面 |
| **F · 乐观锁 409** | 第二个携过期 version 的写入**不生效**、不静默覆盖、不静默重试；UI 原样呈现 409 + "重新读取最新 version 再改"；提示刷新后重试 |
| **G · 绑定引用已逻辑删除的资源（悬挂引用）** | 该资源在展开结果中**完全不出现**（不进快照、不放行）；不暴露其曾存在；无 undelete |
| **H · 主体/角色不存在** | `POST` 回 422/明确错误、**不落库**；FormDrawer 红色提示，输入保留；不"尽力创建" |
| **分页越界 / 大结果集** | 强制分页（缺省 20、钳 200）；翻页只换查询参数；末页空白显示"无更多" |
| **越权 / 不可见行** | 列表只呈现可见行，**不**提示被隐藏的行数（不泄露存在性） |
| **权限不足（操作者无写权）** | 写按钮/抽屉提交回受限态（红），**不**乐观假装成功；本地视图不变 |

---

## 七、与后端契约对齐

| 契约硬约束 | 本页落地 |
|---|---|
| **分页 `page_no/page_size`**（缺省 20、钳 200，`DB_PAGINATION_MANDATORY`） | `GET /v1/bindings` 一律分页；DataTable 页号/页大小映射到这两个参数，页大小上限钳 200 |
| **雪花 id 字符串不丢精度** | binding id / principal id 全程当**字符串**处理，等宽截断显示 + 复制全值；**绝不**当 number 解析（>2^53 丢精度） |
| **Scope 两种 kind** | `selector` ⇄ `Scope::Selector(String)`（提交 `{all:[{key,value}]}` 原文）、`resource` ⇄ `Scope::Resources(Vec<ResourceCode>)`；展开在 daemon 快照构建，前端只渲染回报的资源集 |
| **空集 / 不可解析 = fail-closed 授予空** | 空集不放行（异常 B）、不可解析不落库（异常 C）；前端预览与提交均不"尽力放宽" |
| **写端点乐观锁 version** | `POST /v1/bindings` 与删除携带读取时的 `version`；409 → 提示刷新重试；期望 version 唯一来源是先前读取值，前端不自造 |
| **deny 逐字段不泄露存在性** | 展开结果与 grants 预览只导出该 Principal 自身授权世界；Scope 外 / 不存在的资源**不区分**（与 `your_grants` 同源、scope-bounded） |
| **写后三联动可追溯** | 成功提示 policy_rev 前进，并提供跳转到对应 `policy_change` audit（`02-audit` 按 kind 过滤） |
| **匿名化 / 脱敏** | 资源只显代号（`ResourceCode`），永不显真实地址；本页不涉及 `secret_hash` / 明文（纪律一致） |
| **零安全逻辑** | 展开、合法性裁决、tier 落点全在 daemon；SPA 只发起与渲染，绝不前端推断授权面 |
| **`admin` 不可授予** | Role 选择器的角色集来自 daemon，`admin` 不在可选项（前端便利）；真正硬拒在 daemon（异常 A，模型层 `SEC_ADMIN_NOT_GRANTABLE`），前端不依赖前端拦截 |
