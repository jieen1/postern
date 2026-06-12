# postern-cli 模块详细设计

> 本篇是 `postern-cli`（二进制 `postern`）的模块级详细设计，在《详细设计文档》第八部分 8.12「外壳层（客户端）」的领域裁决之上展开。结构遵循《索引与规约》（00-...）规定的七小节。**纯设计，不含实现代码、阶段划分或进度状态。** 与本篇冲突时，以《技术设计文档》七公理与《详细设计文档》第八部分为准。

---

## 1 · 定位（一句话）

`postern-cli` 是控制面的**瘦客户端**——把人类操作者的管理意图翻译为一次控制面 HTTP/JSON over UDS 调用，并把结构化响应渲染给人；它不持任何本地状态、不含任何安全逻辑，崩溃或缺席都不改变 `posternd` 的任何安全行为。

---

## 2 · 承载领域与职责范围

**承载领域**：8.12 外壳层中的**控制面客户端**侧（与 SPA/桌面壳并列，同属"翻译与事实采集层"，自身不做安全决策）。本 crate 不承载数据面外壳（数据面 MCP/HTTP 外壳归 `postern-daemon::shells`）。

**职责范围**（封闭列举）：

1. **命令解析**：以 clap（derive）把命令行参数解析为一组管理意图，做语法层校验（参数缺失、互斥、格式不合法的本地拒绝），不做任何安全判断。
2. **请求构造**：把每条命令映射为对应控制面端点（见 6.5）的一次 HTTP/JSON 请求；填充路径参数、查询参数（含分页 `page_no/page_size`）与请求体（含写端点的期望 `version`，原样取自先前读取响应）。
3. **传输**：经 `hyper + hyperlocal` 建立 HTTP-over-UDS 连接到 `control.sock`，发起单次请求并接收响应。
4. **结果渲染**：把 `Page<T>` 信封、单条响应、统一 `{error:{code,message}}` 错误信封渲染为人类可读输出（表格/文本）或机器可读形态（如 `audit --format jsonl`）。雪花 id 在 JSON 中恒为字符串，渲染时原样作为字符串展示，绝不数值化。
5. **接入向导编排（`init`）**：把"资源接入"这一多步流程编排为一串控制面调用（建资源 → 触发 `discover` 探测 → 呈现候选对象供人圈选 → 写入 tier/细则/绑定）；其中 tier 子集校验与探测执行均在 daemon 侧完成（见 6.3、第 4 节），CLI 仅发起与呈现。向导可附带在客户端本地**生成 `CLAUDE.md` 片段**（把"该 Principal 经哪个 MCP 端点、有哪些已授权动词"渲染为可粘贴文本）——此为纯客户端文本渲染便利，零安全逻辑。
6. **MCP stdio 桥（`mcp-stdio`）**：把仅支持 stdio 的 MCP 宿主的 stdin/stdout 字节流，零逻辑转接到 daemon 数据面 `data.sock` 的 MCP 端点；桥本身不解析、不归类、不脱敏、不增删字节（搬运者）。

CLI 的统一形态：**每条命令 = 一次控制面 HTTP 调用 + 结果渲染**（`mcp-stdio` 是例外形态——它是数据面字节桥，见第 6 节）。

---

## 3 · 支持的功能

按命令组组织（与 4.6、6.5 端点一一对应；每条命令落到一个或一串控制面端点）：

| 命令组 | 子命令 | 对应控制面端点（6.5） | 说明 |
|---|---|---|---|
| `daemon` | `status` / `stop` | `GET /v1/health` · `POST /v1/shutdown` | daemon 状态查询与停机（启动经 `posternd` 或系统服务，不在 CLI 职责内） |
| `init` | —（交互向导） | 编排 `POST /v1/resources` → `POST /v1/resources/{code}/discover` → `POST .../constraints`/`bindings` 等 | 资源接入向导：探测 + 代修（缺口由 daemon 报回，向导呈现并发起修正调用）+ 可选 `CLAUDE.md` 片段生成（本地渲染） |
| `resource` | `add` / `list` / `disable` / `discover` | `POST/GET /v1/resources` · `POST /v1/resources/{code}/discover` | 资源接入与能力面探测（控制面 `discover`，发现≠授权，见 CONS-20） |
| `principal` | `add` / `list` / ... | `POST/GET /v1/principals` | 主体管理 |
| `role` | `add` / `list` / ... | `POST/GET /v1/roles` · `/v1/bindings` | 角色与绑定 |
| `credential` | `add` / `revoke` / `rotate` / ... | `POST /v1/credentials` · `POST /v1/credentials/{id}/revoke` · `.../rotate` · `PUT .../trust-domain` | 凭证签发/吊销/重叠期轮换/可信域设定 |
| `grants` | `<principal>` | `GET /v1/grants/{principal}` | 授权视图（与 your_grants 同源） |
| `elevate` | `<principal> --cap <res:verb> --ttl <dur>` | `POST /v1/grants/temp` | 临时授权 |
| `revoke-grant` | `<id>` | `DELETE /v1/grants/temp/{id}` | 撤销临时授权 |
| `mode` | `set <observe\|maintain\|freeze\|normal> [--resource <code>] [--ttl <dur>]` | `PUT /v1/mode` | 模式切换（全局/单资源，可带 ttl） |
| `freeze` | —（= `mode set freeze` 全局） | `PUT /v1/mode` | 全局冻结的便捷别名 |
| `constraint` | `add` / `list` / `rm <res> ...` | `POST/GET/DELETE /v1/resources/{code}/constraints` | 细则（grant_constraints）管理 |
| `condition` | `add` / `list` / `rm <res> ...` | `POST/GET/DELETE /v1/resources/{code}/conditions` | 条件谓词（grant_conditions）管理 |
| `deny-note` | `set` / `list` / `rm <res> <verb>` | `POST/GET/DELETE /v1/resources/{code}/deny-notes` | 拒绝注记（deny_notes，公理六） |
| `settings` | `get` / `set <key> [<value>]` | `GET/PUT /v1/settings/{key}` | 设置项读写（已知 key 注册表见 5.2） |
| `approvals` | `list` / `approve` / `deny <id>` | `GET /v1/approvals` · `POST /v1/approvals/{id}/approve\|deny` | 审批挂起队列查询与裁决（见 6.10） |
| `denials` | `[--window 7d]` | `GET /v1/denials/summary?window=7d` | 拒绝聚合分析（见 6.9） |
| `audit` | `[--principal ...] [--since ...] [--format jsonl]` | `GET /v1/audit?since&principal&kind&decision&page_no&page_size` | 审计查询（分页，倒序） |
| `verify` | — | `POST /v1/verify` | 红队自检触发（执行在 daemon，见 6.7） |
| `export` / `import` | `<file.toml>` | `POST /v1/export` · `POST /v1/import` | 声明式导出/导入（见 6.6） |
| `mcp-stdio` | — | —（数据面 `data.sock` 的 `/mcp` 端点，非控制面端点） | stdio↔UDS MCP 桥 |

通用渲染能力（覆盖所有集合命令）：

- **分页参数透传**：集合命令把 `page_no/page_size` 作为查询参数透传给控制面，缺省由控制面取默认值（20，上限 200 由 daemon 钳制）；CLISide 不做"取回全量再切片"，分页由后端执行（与契约 `DB_PAGINATION_MANDATORY` 一致——客户端不持有分页职责）。
- **雪花 id 字符串渲染**：响应中的 `id`/`principal_id`/`resource_id`/`credential_id` 等均为雪花 id 的字符串序列化（见 5.1-⑥、5.3），CLI 原样以字符串渲染，绝不解析为整数（避免精度丢失）。
- **乐观锁版本透传**：读端点响应携带的 `version` 由 CLI 渲染给人并在后续更新/删除命令中原样回传（期望 version 的唯一来源是先前读取值，见 5.1-②、6.5）。
- **结构化错误原样呈现**：统一 `{error:{code,message}}` 信封原样渲染，CLI 不改写、不推测、不补充话术（公理六的客户端侧延续）。

---

## 4 · 明确边界（不做什么）

每项指明归属域：

- **不做任何安全决策**：认证、归类、RBAC、细则/条件求值、tier 选择、脱敏、审批裁决逻辑一律不在本 crate。决策归**策略引擎**（`postern-core::eval`），执行编排归**数据面内核**（`postern-daemon::kernel`），裁决入口归**控制面**（`postern-daemon::control`）。
- **不持久化、不读策略库**：不触 `policy.db`、不含 rusqlite、不含任何 SQL 字符串。策略状态的持久化与读写归**存储层**（`postern-store`）；禁止依赖 `postern-store`（契约 `ARCH_FORBIDDEN_EDGES`）。
- **不触机密**：不读保险箱、不解析 `vault://`、不构造或持有 `ResolvedTarget`/`ResourceCredential`/`PresentedCredential`、不持 ScrubSet 句柄。机密归**机密面**（`postern-secrets`）；禁止依赖 `postern-secrets`（契约 `ARCH_FORBIDDEN_EDGES`）。`credential add` 提交的凭据录入也只是把人类输入转发给控制面端点，由 daemon 经机密面写入 `vault://` 引用；CLI 不在本地经手凭据明文的存储或派生。
- **不执行探测、不做 tier 子集校验**：`init`/`resource discover` 只**触发** `POST /v1/resources/{code}/discover`；真实连上资源探测能力面（`Adapter::discover`）与"tier 声明 ⊆ 底层账号真实权限"的校验（见 6.3）都在 daemon 侧执行。CLI 只发起、呈现缺口、再发起修正调用——"代修"是把 daemon 报回的缺口转译为后续控制面写调用，修正动作的语义合法性裁决仍在 daemon。
- **不触达数据面能力发现**：CLI 的 `discover` 是**控制面**接入侧探测；与数据面 Agent 可见的 `postern_surface`（授权快照投影）无关，二者术语边界由 CONS-20 固化，CLI 不借用。
- **不生成业务建议文案**：`CLAUDE.md` 片段只渲染"已授权动词、MCP 端点位置"这类**事实**；不代 daemon 编造引导话术（公理六）。片段内容来源是控制面回报的授权事实，不是 CLI 的推测。
- **不承载数据面外壳形态**：MCP/HTTP 数据面外壳归 `postern-daemon::shells`；CLI 仅以 `mcp-stdio` 作字节桥，不在本地实现 MCP 协议语义或工具面。
- **不自启 daemon、不管理生命周期编排**：`daemon status/stop` 只调用 `GET /v1/health`/`POST /v1/shutdown`；启动 `posternd`、解锁保险箱、注册插件等启动序列归 `postern-daemon::boot`。

---

## 5 · 对外接口

`postern-cli` 是二进制 crate，**不向工作区内其他 crate 暴露任何库接口**——它处于依赖图末端，无人依赖它。其"接口"是面向人的命令行契约与面向 daemon 的网络协议：

- **对人**：第 3 节命令面（clap derive 定义的命令树）；输出形态为人类可读渲染与可选机器形态（`--format jsonl`）。
- **对 daemon（控制面）**：HTTP/JSON over `control.sock`，端点契约即 6.5 表（设计承诺：CLI 不得调用 6.5 未列出的端点，不得自定义私有控制协议）。
- **对 daemon（数据面，仅 `mcp-stdio`）**：HTTP-over-UDS 到 `data.sock` 的 MCP 端点，作字节级转接。

**类型消费（来自 `postern-core`）**：CLI 复用 core 定义的**共享请求/响应/分页类型**作为 JSON 序列化的两端契约，确保客户端与服务端对同一信封的形态一致：

- `PageQuery` / `Page<T>`（分页查询与信封，见 5.1-⑤）。
- `DenyResponse` 及其内嵌的 `DeniedFacts`（当 CLI 经审计/查询展示拒绝事实时，按同一结构反序列化与渲染）。
- 控制面请求/响应 DTO（principals/roles/resources/grants/mode/settings 等端点的入参与出参结构，定义权在 core 的共享类型，避免 CLI 与 daemon 各自定义导致漂移）。

**说明（定义 vs 实现）**：上述类型的**定义**全部归 `postern-core`（领域核心模型，8.1）；CLI 只**消费**（反序列化与渲染），不定义任何新的跨平面协议类型。CLI 自身的内部类型（clap 参数结构、渲染辅助）不构成对外接口。

---

## 6 · 与相邻模块的交互

依据《索引与规约》权威依赖图，`postern-cli` 只有两条允许的依赖边：`cli → core`（共享类型，库级编译期依赖）与 `cli → daemon::control`（运行期网络依赖，非编译期 crate 依赖）。**禁止边** `cli ↛ store` 与 `cli ↛ secrets` 由契约 `ARCH_FORBIDDEN_EDGES` 强制，本节绝不描述任何此类交互。

### 6.1 与 `postern-core`（共享类型）— 编译期库依赖

- **方向**：`cli` → `core`（消费，单向）。
- **内容**：分页类型 `PageQuery`/`Page<T>`、`DenyResponse`/`DeniedFacts`、控制面请求/响应 DTO、`Capability` 枚举（用于 `elevate --cap <res:verb>` 的本地参数校验，仅做"是否合法动词字面量"的语法层判断，不做授权判断）。雪花 id 在协议中恒为字符串，CLI 不依赖 core 的 `IdGen`（id 由 daemon 生成，CLI 永不生成 id）。
- **时机**：编译期链接；运行期在请求构造（序列化入参）与结果渲染（反序列化出参）两点使用。这是**客户端流程**，不在数据面求值管线内（CLI 不参与 [0]~[10] 任一步）。
- **失败语义**：反序列化失败（响应不符合共享类型契约）按本地错误向人报告，绝不"猜测性补全"或忽略字段；解析不出的响应一律呈现为错误而非静默成功（fail-closed 的客户端延续）。CLI 端的失败只影响该命令的呈现，绝不影响 daemon 安全状态。

### 6.2 与 `postern-daemon`（控制面 `control` 域）— 运行期 HTTP/JSON over `control.sock`

这是 CLI 唯一的运行期安全相关交互边，对应跨模块交互矩阵中「cli → daemon::control / 每条管理命令」一行。

- **方向**：`cli`（客户端）→ `daemon::control`（服务端）。请求由 CLI 发起，daemon 处置并回应；CLI 从不被 daemon 反向调用。
- **内容**：
  - **请求**：第 3 节命令对应的 HTTP 方法/路径（6.5 端点）+ 路径参数 + 查询参数（含分页 `page_no/page_size`）+ JSON 请求体（写端点含期望 `version`；凭据录入含人类输入的凭据材料，由 daemon 转交机密面，CLI 不在本地落地）。
  - **响应**：`Page<T>` 信封 / 单条响应（含 `version`）/ 统一 `{error:{code,message}}` 错误信封 / 4xx-5xx 状态。读端点统一回 `version`，供 CLI 渲染与后续乐观锁回传。
- **时机**：在**客户端命令流程**内——一条命令一次往返（`init` 是同一流程内的多次顺序往返：建资源 → 触发 discover → 圈选回写）。这些都发生在 daemon 启动序列之后、控制面 router 已挂载 `control.sock` 之时；CLI 不参与 daemon 的启动序列（boot）或数据面求值管线（kernel）。`daemon stop`/`POST /v1/shutdown` 触发 daemon 的优雅停机，但停机编排归 daemon。
- **失败语义**（fail-closed 的客户端表达）：
  - **连接失败**（`control.sock` 不存在/无权连/daemon 未运行）：CLI 报明确错误（daemon 不可达），**绝不**回退到任何本地策略路径或本地缓存决策——CLI 无本地安全逻辑可回退，无路径即拒绝是唯一诚实结果。
  - **`409 Conflict`**（乐观锁版本不匹配）：CLI 原样呈现冲突，提示重新读取最新 `version`；绝不自动重试覆盖（与 5.1-② 不静默重试一致）。
  - **错误信封**：`{error:{code,message}}` 原样渲染，`message` 是 daemon 侧已脱敏的常量安全文案，CLI 不展开、不补全、不推测底层原因（公理四/六的客户端延续）。
  - **写命令失败**：一切写入的事务、快照重建、审计三联动都在 daemon 内完成；CLI 收到失败即如实呈现，不假定任何部分生效，不在本地补偿。
  - **`control.sock` 权限边界**：`control.sock` 为 `0600`（仅属主）并叠加控制面认证（见 5.5、8.10），CLI 以操作者本人身份连接；连不上的隔离侧（如 Agent uid）从 CLI 这条路径同样无法触达控制面——这是部署前置条件，不是 CLI 设防。

### 6.3 与 `postern-daemon`（数据面 `shells/mcp` 端点）— 仅 `mcp-stdio` 字节桥

- **方向**：`mcp-stdio` 子命令在 MCP 宿主（stdin/stdout）与 daemon 数据面 `data.sock` 的 MCP 端点之间双向转接字节。
- **内容**：未经解释的 MCP 协议字节流（请求与响应原样搬运）。CLI **不**构造 `NormalizedRequest`、**不**解析 Intent、**不**脱敏——归一化（步骤 [0]）、求值（[1]~[6]）、执行（[8]）、脱敏（[9]）全部在 daemon 数据面内核完成（公理七：经 stdio 桥进入与直连 `data.sock` 的请求走完全相同的管线，得到一致语义）。
- **时机**：宿主进程生命周期内持续转接；不属控制面命令的"一次往返"模型。
- **失败语义**：`data.sock` 不可连或转接中断时，桥按错误终止该会话；桥的中断不产生任何本地决策，不绕过 daemon。`data.sock` 才是 Agent 可达入口（`0660`/专用组，见 5.5），`mcp-stdio` 只是把 stdio 宿主接到这一既有入口，不改变其权限边界。

### 6.4 被禁止的交互（显式声明，绝不实现）

- `cli ↛ postern-store`：CLI 永不读写 `policy.db`、永不含 SQL 字符串或 rusqlite 依赖（违反即触发 `DB_NO_RAW_SQL_OUTSIDE_STORE` 与 `ARCH_FORBIDDEN_EDGES`）。一切策略状态读写经控制面 API。
- `cli ↛ postern-secrets`：CLI 永不读保险箱、永不持机密类型、永不解析 `vault://`。凭据录入只把输入转交控制面端点，由 daemon 经机密面落地。
- CLI 不直接依赖 `postern-adapters`/`postern-transports`：协议解释与通路建立均在 daemon 内；`init` 的探测经控制面 `discover` 间接触发，CLI 不直连资源。

---

## 7 · 必守不变量

| 不变量 | 强制手段 |
|---|---|
| **零本地安全逻辑**：认证/归类/RBAC/细则/条件/tier 选择/脱敏/审批裁决一律不在 CLI；一切安全决策在 daemon。 | 8.12 领域裁决（外壳不做安全决策）；缺失 store/secrets 依赖使 CLI 无路径实现这些逻辑（结构性保证）。 |
| **零本地状态**：CLI 不持久化、不缓存策略/凭据/审计/决策；每条命令是无状态的一次往返。 | 8.12「客户端零状态」；无 store/secrets 依赖。 |
| **不含任何策略/机密/审计的直接触达路径**：策略只经控制面 API 读写，机密永不经手，审计只经 `GET /v1/audit` 查询。 | 契约 `ARCH_FORBIDDEN_EDGES`（`cli ↛ store/secrets`）；契约 `DB_NO_RAW_SQL_OUTSIDE_STORE`（CLI 中出现裸 SQL 即红，build.rs 反例夹具含 `postern-cli` 内的违规样本）。 |
| **绝不本地构造机密类型与来源**：`ResolvedTarget`/`ResourceCredential` 只在 secrets 构造，`ConnOrigin` 只在 daemon shells 构造，CLI 无构造权。 | 契约 `SEC_CONSTRUCTION_SITES` + `SEC_SECRET_TYPE_DISCIPLINE`；CLI 无 secrets 依赖。 |
| **雪花 id 恒以字符串呈现**：id 永不在 CLI 端数值化，CLI 永不生成 id。 | 5.1-⑥/5.3 字符串序列化约定；id 生成唯一来源是 daemon 的 `core::id::IdGen`。 |
| **分页交给后端**：集合命令透传 `page_no/page_size`，不取回全量再切片，不在客户端分页。 | 契约 `DB_PAGINATION_MANDATORY`（分页后端执行，禁前端分页）。 |
| **乐观锁版本只透传不自造**：期望 `version` 原样取自先前读取响应，冲突不静默重试。 | 5.1-②、6.5 端到端乐观锁语义。 |
| **fail-closed 的客户端延续**：daemon 不可达/响应不可解析时报错，绝不回退本地决策或猜测性补全。 | 公理二；CLI 无可回退的本地安全路径（结构性）。 |
| **输出只转述事实**：错误信封、拒绝响应、`CLAUDE.md` 片段只渲染 daemon 给出的事实，不补全、不推测、不编造话术。 | 公理六；daemon 侧已脱敏文案 + 常量化 `message`。 |
| **客户端崩溃不影响 daemon 安全行为**：CLI 缺席或异常退出，daemon 的求值/连接/审计照常。 | 8.12 必守不变量（客户端崩溃或缺席不影响 daemon 任何安全行为）。 |

---

> **一致性声明**：本篇所述交互严格限于权威依赖图允许的两条边（`cli → core`、`cli → daemon::control`，外加 `mcp-stdio` 对 `data.sock` 的字节桥），与跨模块交互矩阵「cli → daemon::control」一行一致，未描述任何被 `ARCH_FORBIDDEN_EDGES` 禁止的依赖边（`cli ↛ store/secrets`）。领域归属（tier 选择归策略引擎、探测与 tier 子集校验归 daemon 侧、机密落地归机密面、discover 两术语边界 CONS-20）一律以《详细设计文档》第八部分为准。

---

## 8. 验收标准

> 本节是 `postern-cli` 的验收基准——每条给「输入 → 可观察结果」判据 + 验证方式（统一词汇）。逐维度对应前文：**A** 对应 §3 功能、**B** 对应 §5 对外接口、**C** 对应 §4 边界、**D** 对应 §7 不变量、**E** 对应 §6 交互、**F** 失败与边界行为。`postern-cli` 是依赖图末端的瘦客户端，其全部安全价值在于「零安全逻辑、零本地状态、每条命令=一次控制面往返+渲染」——验收据此判定。

### 8.A 功能完整性（对应 §3 每项功能）

| # | 功能（§3） | 输入 → 可观察结果判据 | 验证方式 |
|---|---|---|---|
| A1 | 命令解析（语法层本地校验，§3-1） | 缺失必填参数 / 互斥参数同时给出 / 格式非法（如 `--ttl` 非时长）→ 命令在**本地**以非零退出码拒绝并打印用法，**不发起任何网络请求**；语义合法性判断一律不在本地 | `单元测试`（clap 解析层）；`构造签名审查`（参数结构无任何授权/归类字段） |
| A2 | 请求构造映射到端点（§3-2） | 每条子命令产出的 HTTP 方法/路径与 §3 表 = 6.5 端点逐行一致；路径参数、查询参数（含 `page_no/page_size`）、请求体（写端点含期望 `version`）按约定填充 | `单元测试`（命令→请求映射快照）；`场景规格 docs/examples/03 §3.1`、`场景规格 docs/examples/06 §3.1`、`场景规格 docs/examples/07 §4.1` |
| A3 | 传输：HTTP-over-UDS 单次往返（§3-3） | 一条命令对 `control.sock` 发起恰好一次请求并接收响应；无连接池常驻、无后台保活 | `集成测试(内存Fake：UDS 上的伪控制面)`；`构造签名审查` |
| A4 | 结果渲染（`Page<T>`/单条/错误信封，§3-4） | 给定 `Page<T>` JSON → 渲染表格且原样回显 `version`；给定 `{error:{code,message}}` → 原样渲染 code/message，不改写不补全；`audit --format jsonl` → 输出逐行 JSON（机器可读） | `单元测试`（渲染快照）；`场景规格 docs/examples/07 §4.1`（A 步 `--format jsonl`） |
| A5 | 雪花 id 字符串渲染（§3-4、§3 通用） | 响应中 `id`/`*_id` 为字符串 → CLI 原样以字符串展示，**绝不**解析为整数；超出 2^53 的 id 渲染后无精度丢失 | `单元测试`（大数 id 往返）；`构造签名审查`（DTO 中 id 字段类型为字符串，非整型） |
| A6 | 分页参数透传（§3 通用） | 集合命令把 `page_no/page_size` 作为查询参数透传；缺省不传由后端取默认；CLI **不**取回全量再本地切片 | `Stele契约 DB_PAGINATION_MANDATORY`；`构造签名审查`（无客户端切片代码路径）；`场景规格 docs/examples/07 §4.1`（`--page-no/--page-size`） |
| A7 | 乐观锁版本透传（§3 通用、§3-2） | 读响应 `version` 渲染给人 → 后续写命令原样回传；CLI 自身从不生成/递增 `version` | `单元测试`（version 取自读响应、回传一致）；`场景规格 docs/examples/06 §3.1`（写命令携带期望 version） |
| A8 | 接入向导编排 `init`（§3-5） | 同一流程内顺序发起：建资源 `POST /v1/resources` → 触发 `POST /v1/resources/{code}/discover` → 呈现 daemon 报回的候选/缺口 → 圈选回写 tier/细则/绑定；tier 子集校验与探测执行**不在** CLI | `集成测试(内存Fake)`；`场景规格 docs/examples/02 §4.1`（步骤 1→8）；`场景规格 docs/examples/02 §4.2`（E3 代修、E5 tier 校验由 daemon 报缺口） |
| A9 | `CLAUDE.md` 片段生成（§3-5，纯本地渲染） | 片段内容只由控制面回报的授权事实（已授权动词、MCP 端点位置）渲染；**不**含 CLI 自造的引导话术 | `单元测试`（片段输入=授权事实、无额外文案）；`场景规格 docs/examples/02 §4.1`（步骤 8） |
| A10 | `mcp-stdio` 字节桥（§3-6） | 把宿主 stdin/stdout 与 `data.sock` MCP 端点双向逐字节转接；**不**解析、不归类、不脱敏、不增删字节；进桥与直连 `data.sock` 走同一管线得一致语义 | `集成测试(内存Fake：回声 MCP 端点)`（字节进出逐字节相等）；`构造签名审查`（桥路径无 `NormalizedRequest`/Intent/Sanitizer 引用） |
| A11 | 命令组完整性（§3 全表） | 命令树覆盖 `daemon`/`init`/`resource`/`principal`/`role`/`credential`/`grants`/`elevate`/`revoke-grant`/`mode`/`freeze`/`constraint`/`condition`/`deny-note`/`settings`/`approvals`/`denials`/`audit`/`verify`/`export`/`import`/`mcp-stdio`，每组子命令落到 §3 所列端点 | `单元测试`（命令树枚举 vs §3 表）；`场景规格 docs/examples/06 §3.1`、`场景规格 docs/examples/07 §4.1` |

### 8.B 对外接口契约（对应 §5）

`postern-cli` 是二进制 crate，**不向工作区暴露任何库接口**（处于依赖图末端，无人依赖）；其「对外接口」是面向人的命令行契约与面向 daemon 的网络协议契约。

| # | 接口面（§5） | 判据 | 验证方式 |
|---|---|---|---|
| B1 | 对人：clap 命令树 | 命令树形状（命令组/子命令/参数）稳定，与 §3 一致；`--help` 输出可被快照锁定 | `单元测试`（命令树快照）；`构造签名审查` |
| B2 | 对 daemon（控制面）：只调 6.5 端点 | CLI 发起的端点集合 ⊆ §3=6.5 列出端点；**不**调用未列端点、**不**自定义私有控制协议 | `单元测试`（端点白名单断言）；`构造签名审查` |
| B3 | 共享类型消费（来自 core，§5） | CLI 复用 core 的 `PageQuery`/`Page<T>`/`DenyResponse`/`DeniedFacts`/控制面 DTO 作序列化两端契约；**不**定义任何新的跨平面协议类型 | `构造签名审查`（CLI crate 内无新协议类型定义，只 `use` core 类型）；`单元测试`（信封反序列化一致性） |
| B4 | 错误路径语义 | 反序列化失败 / 4xx-5xx → 以本地错误向人报告并非零退出；绝不静默成功 | `单元测试`（畸形响应→报错）；见 8.F |

### 8.C 边界——禁止项（对应 §4「不做什么」，机器可验者优先）

| # | 禁止项（§4） | 判据（确实没做 X / 无 X 路径） | 验证方式 |
|---|---|---|---|
| C1 | 不依赖 store、不读策略库、不含裸 SQL | `postern-cli` 的 `Cargo.toml` 无 `postern-store`/`rusqlite` 依赖；CLI 源码内无 SQL 字符串 | `Stele契约 ARCH_FORBIDDEN_EDGES`（`cli ↛ store`）；`Stele契约 DB_NO_RAW_SQL_OUTSIDE_STORE`（CLI 内裸 SQL 即红，build.rs 反例夹具含 `postern-cli` 违规样本）；`cargo tree`（确认无 store 边） |
| C2 | 不触机密、不依赖 secrets、不构造机密类型 | `Cargo.toml` 无 `postern-secrets` 依赖；CLI 内无 `ResolvedTarget`/`ResourceCredential`/`PresentedCredential` 构造点、不持 ScrubSet 句柄、不解析 `vault://` | `Stele契约 ARCH_FORBIDDEN_EDGES`（`cli ↛ secrets`）；`Stele契约 SEC_CONSTRUCTION_SITES`（机密类型只在 secrets 构造）；`Stele契约 SEC_SECRET_TYPE_DISCIPLINE`（机密类型无 Clone/Serialize，CLI 即便误引用也无法持有）；`cargo tree` |
| C3 | 不做任何安全决策（认证/归类/RBAC/细则/条件/tier 选择/脱敏/审批裁决） | CLI 无上述决策的代码路径；缺失 store/secrets 依赖使其结构性无路径实现 | `构造签名审查`（参数结构与渲染层无决策字段）；由 C1/C2 的依赖缺失结构性保证 |
| C4 | 不执行探测、不做 tier 子集校验 | `init`/`resource discover` 只发起 `POST /v1/resources/{code}/discover`；CLI 内无连资源探测能力面、无「声明 ⊆ 真实权限」比对逻辑 | `集成测试(内存Fake)`（CLI 仅发请求、呈现 daemon 报回缺口）；`场景规格 docs/examples/02 §4.2`（E3、E5 校验由 daemon 执行）；`构造签名审查` |
| C5 | 不直连数据面能力发现 / 不借用 `postern_surface` | CLI 的 `discover` 是控制面接入侧；CLI 内无 `postern_surface` 投影逻辑（数据面术语） | `场景规格 docs/examples/02 §4.2`（E8，CONS-20 术语边界）；`构造签名审查` |
| C6 | 不生成业务建议文案（`CLAUDE.md` 片段只渲染事实） | 片段输入仅授权事实，CLI 无话术生成路径 | `单元测试`（同 A9）；`构造签名审查` |
| C7 | 不承载数据面外壳形态 | CLI 仅以 `mcp-stdio` 作字节桥，本地不实现 MCP 协议语义/工具面 | `构造签名审查`（桥路径无 MCP 协议解析）；见 A10 |
| C8 | 不自启 daemon、不管理生命周期编排 | `daemon status/stop` 只调 `GET /v1/health`/`POST /v1/shutdown`；CLI 内无解锁保险箱、注册插件、boot 序列代码 | `单元测试`（`daemon` 子命令仅两端点）；`构造签名审查` |

### 8.D 必守不变量（对应 §7，沿用 §7 已标强制手段）

| # | 不变量（§7） | 判据 | 强制手段 / 验证方式 |
|---|---|---|---|
| D1 | 零本地安全逻辑 | CLI 无认证/归类/RBAC/细则/条件/tier/脱敏/审批裁决路径 | `Stele契约 ARCH_FORBIDDEN_EDGES`（无 store/secrets 边使其结构性无路径）；`构造签名审查` |
| D2 | 零本地状态 | CLI 不持久化、不缓存策略/凭据/审计/决策；每条命令无状态一次往返 | `构造签名审查`（无持久化/缓存字段或文件路径）；`集成测试(内存Fake)`（两次同命令互不依赖前次状态） |
| D3 | 无策略/机密/审计直接触达路径 | 策略只经控制面 API；机密永不经手；审计只经 `GET /v1/audit` | `Stele契约 ARCH_FORBIDDEN_EDGES`；`Stele契约 DB_NO_RAW_SQL_OUTSIDE_STORE` |
| D4 | 绝不本地构造机密类型与来源 | CLI 无 `ResolvedTarget`/`ResourceCredential`/`ConnOrigin` 构造权 | `Stele契约 SEC_CONSTRUCTION_SITES`；`Stele契约 SEC_SECRET_TYPE_DISCIPLINE` |
| D5 | 雪花 id 恒以字符串呈现、CLI 永不生成 id | id 渲染为字符串、无数值化；CLI 不依赖 `IdGen` | `构造签名审查`（DTO id 字段为字符串、无 `IdGen` 引用）；`单元测试`（同 A5） |
| D6 | 分页交给后端 | 透传 `page_no/page_size`、不取回全量再切片 | `Stele契约 DB_PAGINATION_MANDATORY`；`构造签名审查`（同 A6） |
| D7 | 乐观锁版本只透传不自造 | 期望 `version` 原样取自先前读取响应；冲突不静默重试 | `单元测试`（同 A7）；`场景规格 docs/examples/02 §4.2`（E7 `409 Conflict` 不静默覆盖） |
| D8 | fail-closed 客户端延续 | daemon 不可达 / 响应不可解析 → 报错，绝不回退本地决策或猜测性补全 | 见 8.F（F1/F2）；CLI 无可回退本地安全路径（结构性，由 D1 保证） |
| D9 | 输出只转述事实 | 错误信封/拒绝响应/`CLAUDE.md` 片段只渲染 daemon 给出事实，不补全/推测/编造 | `单元测试`（错误信封原样回显）；见 A9 |
| D10 | 客户端崩溃不影响 daemon 安全行为 | CLI 缺席/异常退出时，daemon 求值/连接/审计照常 | `集成测试(真实资源：杀死 CLI 进程后 daemon 行为无变化)`（**运行期不变量**，需起真实 daemon 观察） |

### 8.E 与相邻模块交互（对应 §6，方向/类型/时机/失败语义可验）

| # | 交互边（§6） | 判据（方向 / 类型 / 时机 / fail-closed 失败语义） | 验证方式 |
|---|---|---|---|
| E1 | `cli → core`（共享类型，编译期，§6.1） | 单向消费；序列化入参 + 反序列化出参用 core 类型；反序列化失败 → 本地报错，不猜测性补全、不忽略字段；不依赖 `IdGen` | `构造签名审查`；`单元测试`（畸形响应→报错） |
| E2 | `cli → daemon::control`（控制面 HTTP/JSON over `control.sock`，§6.2） | 方向：CLI 发起、daemon 处置、CLI 从不被反向调用；时机：一命令一往返（`init` 多次顺序往返），均在 daemon 启动序列之后、`control.sock` 已挂载之时；CLI 不参与 boot/kernel | `集成测试(内存Fake)`（CLI 主动发起、无反向回调入口）；`场景规格 docs/examples/03 §3.1`、`docs/examples/06 §3.1` |
| E2a | E2 失败语义：连接失败 | `control.sock` 不存在/无权连/daemon 未运行 → 明确报错（daemon 不可达），**绝不**回退本地策略或本地缓存决策 | `集成测试(内存Fake：无 socket)`；`场景规格 docs/examples/02 §4.2`（E6 相关 fail-closed 控制面错误） |
| E2b | E2 失败语义：`409 Conflict` | 乐观锁版本不匹配 → 原样呈现冲突、提示重读最新 `version`，**不**自动重试覆盖 | `单元测试`；`场景规格 docs/examples/02 §4.2`（E7） |
| E2c | E2 失败语义：错误信封 | `{error:{code,message}}` 原样渲染；`message` 是 daemon 侧已脱敏常量文案，CLI 不展开/补全/推测底层原因 | `单元测试`；`场景规格 docs/examples/02 §4.2`（E1/E3/E4 回报经脱敏、CLI 不外泄真实地址） |
| E3 | `cli → daemon::shells/mcp`（`mcp-stdio` 字节桥，§6.3） | 双向逐字节转接；CLI 不构造 `NormalizedRequest`、不解析 Intent、不脱敏；归一化/求值/执行/脱敏全在 daemon；`data.sock` 不可连或转接中断 → 按错误终止会话、不产生本地决策、不绕过 daemon | `集成测试(内存Fake)`（字节相等 + 中断即终止）；`构造签名审查`（同 A10/C7） |
| E4 | 被禁止的交互（§6.4，绝不实现） | 无 `cli ↛ store`、`cli ↛ secrets`、`cli → adapters/transports` 任何边 | `Stele契约 ARCH_FORBIDDEN_EDGES`；`Stele契约 DB_NO_RAW_SQL_OUTSIDE_STORE`；`cargo tree`（确认依赖只含 core + HTTP/UDS 客户端栈） |

### 8.F 失败与边界行为（关键 fail-closed 路径逐条可验）

| # | 路径 | 输入 → 可观察结果 | 验证方式 |
|---|---|---|---|
| F1 | daemon 不可达即拒绝（无路径回退） | `control.sock` 缺失/无权/daemon 停 → 非零退出 + 「daemon 不可达」错误；**无**任何本地决策输出 | `集成测试(内存Fake：移除 socket)`；§7「fail-closed 客户端延续」 |
| F2 | 响应不可解析即报错（不静默成功） | 响应不符合共享类型契约（缺字段/类型错）→ 本地报错并非零退出，不猜测性补全 | `单元测试`（畸形 JSON→报错） |
| F3 | 写失败不本地补偿 | 写命令收到失败 → 如实呈现，不假定任何部分生效、不在本地补偿（事务/快照/审计三联动全在 daemon） | `集成测试(内存Fake：写端点返回 5xx)`；`场景规格 docs/examples/06 §3.1`（写命令乐观锁 + 失败呈现） |
| F4 | 红队自检结果原样呈现（不代判防线） | `postern verify` → CLI 原样渲染 daemon 回报的逐项 PASS/FAIL + 缺口说明（6.7 九项执行在 daemon），CLI 不自行判定「严格与否」 | `postern verify`（6.7 红队自检九项，执行在 daemon）；`场景规格 docs/examples/07 §4.1`（C 步九项表）；`场景规格 docs/examples/02 §4.1`（步骤 9 九类逐条 PASS 且进审计） |
| F5 | 接入向导缺口呈现即停（不自行修补语义） | `init` 中 daemon 报回缺口（端口不可达 / tier 名实不符 / 2FA 须人在场）→ CLI 呈现缺口并发起「代修」控制面调用，缺口未消解前资源不进入可用状态；修正动作语义合法性裁决仍在 daemon | `场景规格 docs/examples/02 §4.2`（E2/E3/E5）；`集成测试(内存Fake)` |

### 8.G 不适用维度说明

- **对外库接口的 SemVer/下游兼容**：不适用——`postern-cli` 是依赖图末端二进制，无工作区下游 crate（§5）；其稳定性承诺落在命令行契约（8.B）与控制面端点白名单（B2），不在库 API。
- **数据面求值管线参与**：不适用——CLI 不参与求值步骤 [0]~[10]（§6.1、§6.2 时机；`mcp-stdio` 仅作字节桥，管线在 daemon 内）。

### 8.H 完成定义（Definition of Done）

> 当 `postern-cli` 同时满足 **8.A 功能完整性、8.B 对外接口契约、8.C 边界（禁止项）、8.D 必守不变量、8.E 相邻交互、8.F 失败与边界行为** 全部验收项（不适用维度按 8.G 注明），即视为该模块**完成**——即：命令组完整且每条命令=一次 6.5 控制面往返+忠实渲染，雪花 id 恒字符串、分页透传后端、版本只透传不自造，`mcp-stdio` 为零逻辑字节桥，结构化拒绝/错误信封原样呈现，且依赖图中确无 `cli → store/secrets` 边、CLI 无任何本地安全逻辑与本地状态。

> **审查项标注（无法机器验证、须人工/运行期审查者）**：① **D10 / F1 客户端崩溃或 daemon 不可达不影响 daemon 安全行为**——属运行期不变量，需起真实 daemon 杀 CLI 进程观察，由 `集成测试(真实资源)` 覆盖而非静态契约；② **A9 / C6 `CLAUDE.md` 片段「只渲染事实、不编造话术」**——「是否编造话术」是语义判断，机器仅能验证输入=授权事实、无额外固定文案，**剩余主观余量须构造签名审查 + 人工评审**；③ **A10 / C7 `mcp-stdio` 字节级零逻辑**——可用回声 Fake 验证字节相等，但「桥未来不偷偷新增解析/脱敏」须靠 `构造签名审查` 持续守门，非一次性测试可终结。其余各项均可挂到 Stele 契约（`ARCH_FORBIDDEN_EDGES`/`SEC_CONSTRUCTION_SITES`/`SEC_SECRET_TYPE_DISCIPLINE`/`DB_NO_RAW_SQL_OUTSIDE_STORE`/`DB_PAGINATION_MANDATORY`）、场景规格（docs/examples/02·03·06·07）、`postern verify` 或单元/集成测试机器验证。
