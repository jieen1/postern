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

以上为"做什么"的封闭列举。下列子小节把每类功能的**怎么做（数据流/关键结构/时序/取舍）**讲透，达到 02-store/03-secrets §3 同一深度——只述方案，不贴代码。已在上表与上列说清的"做什么"不复述。

### 3.1 命令 → 控制面调用的统一翻译管线（怎么做）

每条命令（`mcp-stdio` 除外）走同一条无分支主干，命令间只在"端点/参数/请求体/渲染器"四处不同，公共流程只有一份：

1. **解析 → 意图结构**：clap（derive）把命令行解析为一个**强类型管理意图**（每个命令组一个枚举变体，携已校验的本地参数）。语法层校验（缺参/互斥/格式）在此阶段由 clap 与少量本地字面量校验完成；任何语义合法性判断一律不在此（取舍：本地只判"形态合法"，不判"是否被允许"，否则就成了客户端安全逻辑——见 §4、L-1）。
2. **意图 → 请求规格**：每个意图变体映射到一个**请求规格**`(method, path_template, query, body)`——这是命令与 6.5 端点之间唯一的映射表，集中一处声明，杜绝散落拼 URL。`path_template` 的路径参数（如 `{code}`/`{id}`/`{principal}`）由意图字段填充；`query` 收集分页与过滤键（`page_no/page_size/since/principal/kind/decision/window` 等）；`body` 仅写端点有，由 core 共享 DTO 序列化（见 §5、6.1），写端点的期望 `version` 字段取自先前读取响应、原样落入 body。
3. **序列化 → 发送 → 反序列化**：请求规格交给传输层（§3.4）发起一次往返；响应字节按"先判信封类别再选目标类型"反序列化——HTTP 状态与响应体顶层形状共同决定走 `Page<T>` / 单条 DTO / `{error:{code,message}}` 三条渲染分支之一（见 §3.3）。
4. **渲染 → 退出码**：选定渲染器输出，并据成败置进程退出码（成功 0、本地拒绝/daemon 错误/不可达非零，见 §3.6）。

> 关键取舍：**意图与请求规格分离**而非"每个命令各自拼 HTTP"。理由有二——①映射表集中后，"命令 ⊆ 6.5 端点"这一设计承诺（§5）可在一处审视，不会某条命令偷偷调私有端点；②公共主干只有一份，使"一条命令恰一次往返、无隐式重试/保活"（F-3）成为结构性事实而非每命令各自保证。

**`export`/`import` 是主干上"请求体来源/渲染落点改址"的唯一变体（怎么做）**：绝大多数命令的 `body` 来自内存 DTO、渲染落 stdout；`import <file.toml>` 与 `export <file.toml>` 把这两端各改址一次，仍走同一主干、仍是一次往返——

- **`import`**：把人指定的本地 `<file.toml>` 读入作请求体直送 `POST /v1/import`（声明式"期望状态 apply"，6.6）。CLI **不**在本地解析 TOML 语义、**不**校验"声明 ⊆ 真实权限"、**不**做部分 apply 拆分——文件原样作 body，TOML 语义校验与"导入失败整体拒绝（无部分 apply）"全在 daemon（6.6）。CLI 至多做"文件可读、非空"这类本地形态检查（属 §3.6 本地语法拒绝类）。
- **`export`**：`POST /v1/export` 的响应体（TOML 文本，永不含明文地址/凭据，只元数据 + `vault://` 引用，6.6）由 CLI 写入人指定的 `<file.toml>`（或 stdout）。这是"渲染落点改址"，不是新增渲染分支。
- **为什么这不算"本地状态"**：导入读入文件、导出写出文件都是**人显式指定的一次性 I/O**，非 CLI 自留的策略/凭据/审计缓存——与 §7「零本地状态」、L-11 对"人显式 `--format`/`export` 指定目标文件"的豁免一致；命令结束 CLI 不持有该文件内容、不据其改变下条命令行为。

### 3.2 hyperlocal 连 control.sock（怎么做）

传输层把"HTTP over UDS"封装为一次性客户端调用，不持久、不池化：

- **连接构造**：以 `hyperlocal` 的 UDS 连接器把目标 `control.sock` 路径包装成 hyper 可用的 `Uri`（UDS 路径以 hyperlocal 约定编码进 URI 的 host 段，真实请求行仍用 6.5 的 `/v1/...` 路径）。每条命令新建连接、发一次请求、读完整响应、关闭——**无连接复用、无后台保活**（取舍：CLI 是一次性短命进程，连接池只增复杂度与"≥2 次连接"的误判风险，与 F-3"恰一次往返"冲突；瘦客户端的正确形态是无状态单发）。
- **不可达的诚实失败**：`control.sock` 不存在 / 无权连（`0600` 属主外）/ daemon 未监听 → 连接阶段即失败，直接转为"daemon 不可达"本地错误（§3.6、L-2）。**绝不**因连不上而走任何本地策略/缓存路径——CLI 结构上无此路径可走（无 store/secrets 依赖），无路径即拒绝是唯一诚实结果（公理二的客户端延续）。
- **权限边界是部署前置，非 CLI 设防**：`control.sock` 的 `0600` + 控制面认证决定"谁能连"；CLI 以操作者本人 uid 连接，隔离侧（如 Agent uid）从这条路径同样连不上。CLI 不自行校验来源、不补认证逻辑（那是 daemon 侧职责）。

### 3.3 信封三分支渲染与 Page<T> 表格化（怎么做）

渲染器按信封类别分三支，每支只转述、不加工：

- **`Page<T>`**：先反序列化为 core 的 `Page<T>`（`items` + 分页元信息 + 每条携 `version`），默认渲染为对齐表格（列取自 DTO 字段），并把分页游标信息（当前页/页大小/是否有下一页）作为页脚提示给人，便于人决定下一条命令的 `--page-no`。集合的"下一页"靠人再发一条命令携新 `page_no`，**不在客户端续抓**（§3.5）。
- **单条 DTO**：键值/纵向字段表展示，并显式回显该响应的 `version`（供乐观锁回传，§3.5）。
- **`{error:{code,message}}`**：原样输出 `code` 与 `message`，**逐字符不增删**（L-4）。`message` 是 daemon 侧已脱敏的常量安全文案，CLI 不展开底层原因、不补"建议这样做"之类话术（公理六）。`DenyResponse`/`DeniedFacts`（经审计或查询展示拒绝事实时）按同一"只转述字段值"的纪律渲染。
- **机器形态 `--format jsonl`**：审计等流式集合命令按行输出逐条 JSON（每行独立可解析），雪花 id 仍为字符串。此形态是把后端已分页的 `items` 逐行打印，不做客户端重排或聚合。
- **反序列化失败即报错**：响应不符合 core 共享类型契约（缺字段/类型错）→ 本地报错并非零退出，**不**猜测性补全、**不**忽略字段、**不**当成功（L-3，fail-closed 的客户端延续）。

### 3.4 雪花 id 字符串渲染与分页参数透传（怎么做）

这两条是上列"通用渲染能力"的实现要点补充：

- **雪花 id 恒字符串**：CLI 端 DTO 把所有 id 字段静态类型定为字符串（而非整型），从**类型层**杜绝 JSON 数字解析路径——根因是雪花 id 超 `2^53`，任何把它读入 IEEE-754 双精度或 64 位整型再格式化的路径都可能丢精度或变科学计数。CLI 永不生成 id、永不解析为整数、渲染即原样透传字符串（F-5、§7"雪花 id 恒以字符串呈现"）。
- **分页交给后端**：集合命令把 `--page-no/--page-size` 直接落进 query string 透传；**不给则不带该键**，由 daemon 取默认（20，上限 200 由 daemon 钳制）。CLI 端**不存在**"取回全量再本地切片"的代码路径（构造签名可核，F-6）——分页职责整体在后端（契约 `DB_PAGINATION_MANDATORY` 的客户端侧表达），客户端只透传游标、不持有分页语义。

### 3.5 乐观锁版本透传链路（怎么做）

`version` 在 CLI 端只"搬运"、绝不"产生"，形成一条人可见的读—改链：

- **读取阶段**：任一读端点响应携 `version`，渲染时显式回显给人（单条直接展示，集合每行附带）。
- **回传阶段**：后续 `update`/`delete`/`disable` 等写命令的请求体期望 `version` 字段，**唯一来源是先前读取值**——由人从上一条读命令的输出取得并作为参数提供，CLI 不自读自比、不自增、不自造（与 02-store §3.1 乐观锁"期望 version 唯一来源是调用方读取值"端到端贯通；F-7、§7）。
- **`409 Conflict` 处置**：写命令收到 `409`（版本不匹配）→ 原样呈现冲突并提示"重新读取最新 version"，**绝不**自动重试覆盖（L-5）。取舍：自动重试 = CLI 替人做了"用我的值盖掉别人的改动"的语义决策，这越过了"零安全逻辑"红线；冲突必须回到人面前重读重改。

### 3.6 错误与退出码（怎么做）

CLI 把三类失败映射为非零退出 + 明确呈现，不在本地补偿：

- **本地语法拒绝**（缺参/互斥/格式/`--cap` 非合法动词字面量）：clap 或本地字面量校验直接非零退出 + 打印用法，对 `control.sock` **零请求**（L-1）。这是唯一"未发请求即失败"的类别。
- **daemon 不可达**：连接失败 → 非零退出 + "daemon 不可达"，输出**无**任何决策结论（无 allow/deny/授权视图，L-2）。
- **daemon 返回错误信封 / 4xx-5xx**：原样呈现 `{error:{code,message}}` 或 `DenyResponse`，写端点 5xx 即如实呈现失败、**不**假定部分生效、**不**本地补写/回滚/重试（L-7）——一切事务/快照重建/审计三联动都在 daemon 内，CLI 收到失败即终止于呈现。

### 3.7 init 接入向导的交互时序（怎么做）

`init` 是 §3 唯一"一条命令、多次顺序往返"的形态，其编排是一台**呈现—圈选—回写**的人机状态机，全部判定权在 daemon，CLI 只发起与呈现：

1. **建资源**：`POST /v1/resources` 落资源骨架（codename/adapter/transport 等人类输入），拿回资源 `code` 与 `version`。
2. **触发探测**：`POST /v1/resources/{code}/discover` 让 **daemon 侧**真实连资源、跑 `Adapter::discover`（CLI 不直连资源、不解析协议——§4）；daemon 回报**候选对象**（可绑定的能力面/账号/库表等）与**缺口清单**（端口不可达 / tier 名实不符 / 须人在场的 2FA 等，对齐 §8 L-8）。
3. **呈现候选 + 缺口**：CLI 把 daemon 回报渲染给人，缺口逐条列出。**缺口未消解前 CLI 不标资源可用**——"是否消解"由 daemon 在后续调用里裁决，CLI 不自行比对"声明 ⊆ 真实权限"（那是 daemon 的 tier 子集校验，§4、L-8）。
4. **圈选回写**：人圈选后，CLI 把选择转译为后续控制面写调用（写 tier `.../constraints`、绑定 `/v1/bindings`、细则等）——这就是"代修"：把 daemon 报回的缺口转译为修正调用，**修正动作的语义合法性仍由 daemon 裁决**，CLI 只发起。
5. **（可选）生成 `CLAUDE.md` 片段**：向导收尾时把"该 Principal 经哪个 MCP 端点（`data.sock` 的 `/mcp`）、有哪些已授权动词"渲染为可粘贴文本。片段内容**只来自控制面回报的授权事实**——已授权动词集为空就如实写"暂无已授权动词"，非空就只列该集合，**绝不**附任何固定引导话术或编造建议（公理六、F-9、L-6）。这是纯客户端文本渲染便利，零安全逻辑。

> 关键取舍：向导**把多步编排放在客户端、把每步判定放在 daemon**。理由——编排（先建后探再圈选）是纯流程顺序、无安全语义，放客户端省一个专用服务端会话端点；而每步的"能不能/对不对"（探测结果、tier 子集、缺口是否真消解）一旦放客户端就成了本地安全逻辑，违反 §7"零本地安全逻辑"。故向导的状态机只管"下一步发哪个已有控制面调用"，从不自行下安全结论。

### 3.8 mcp-stdio 字节桥（怎么做）

`mcp-stdio` 是 §3 唯一**非"一次往返 + 渲染"**的形态——它是数据面字节搬运者，不是控制面客户端：

- **拓扑**：一端是宿主进程的 `stdin`/`stdout`（仅支持 stdio 的 MCP 宿主），另一端是 daemon 数据面 `data.sock` 的 `/mcp` 端点（经 hyperlocal 连 `data.sock`，注意是数据面 socket，权限 `0660`/专用组，与控制面 `control.sock` 不同）。桥在两端之间**双向逐字节转接**。
- **零逻辑搬运**：桥**不**构造 `NormalizedRequest`、**不**解析 Intent、**不**归类、**不**脱敏、**不**增删任何字节——归一化（步骤 [0]）、求值（[1]~[6]）、执行（[8]）、脱敏（[9]）全部在 daemon 数据面内核完成（公理七：经 stdio 桥进入与直连 `data.sock` 的请求走完全相同的管线、得到一致语义）。设计上桥的代码路径里**不出现** `NormalizedRequest`/`Intent`/`Sanitizer` 任何引用（F-10 构造签名可核）。
- **双向并发转接**：stdin→sock 与 sock→stdout 是两个方向独立的字节流，须并发搬运（一端阻塞不得卡死另一端）——以两个方向各一个异步拷贝任务实现，任一方向 EOF/错误即收束整个会话。
- **中断即终止、不绕过**：`data.sock` 不可连或转接中断 → 桥按错误终止该会话并非零退出，**不**产生任何本地构造的 MCP 响应、**不**伪造、**不**绕过 daemon（L-10）。取舍：桥宁可"断"也不"补"——一旦桥在 daemon 缺席时自造响应，就等于在数据面外造了一条无策略、无审计、无脱敏的旁路，直接违反公理七与公理二。

### 3.9 实现要点与工程约束

本小节集中本模块的横切工程要求；与全局工程规范（详细设计 7.x）一致处仅引用、不重抄。

- **并发/线程模型**：CLI 是短命进程，**每条命令一次往返后即退出**，无常驻、无后台任务、无跨命令共享状态。控制面命令既可同步实现（一次阻塞往返足矣），亦可在 tokio 单运行时上以 async 实现（与 `hyper`/`hyperlocal` 的异步栈天然契合）——无论哪种，**不持连接池、不开保活任务**（F-3）。唯一需要真正并发的是 `mcp-stdio`：其 stdin↔sock 双向拷贝须并发（§3.8），以 tokio 两个拷贝任务承载，任一方向终止即取消另一方向、收束会话。命令路径无锁（无共享可变状态可争）。
- **错误处理与传播**：每个面向人的失败都映射到非零退出码 + 明确呈现；CLI 自身错误用本 crate 的 `thiserror` 错误枚举建模，`anyhow` 仅允许出现在 `postern` 二进制的 `main`（详细设计 7.1 错误模型）。CLI **不**复用 core 的"错误变体 → 拒绝阶段"穷尽 match——那是数据面求值路径的产物，CLI 不在求值管线内（[0]~[10] 任一步都不参与），无"拒绝阶段"概念；CLI 端的失败只分"本地语法拒绝 / daemon 不可达 / daemon 返回错误信封"三类（§3.6）。**fail-closed 的客户端延续**：任何不确定（连不上、解不出、缺字段）一律报错非零退出，绝不静默成功或猜测补全（L-2、L-3）。**panic 政策**：CLI 不在数据面外壳的 CatchPanic 覆盖内（那是 daemon 数据面 handler 的层，详细设计 7.1），但同样遵 workspace 级 lint 红线——`unwrap_used`/`expect_used`/`panic`/`unsafe_code` 等 deny（B-5、详细设计 7.1）；CLI 崩溃只影响本进程呈现，对 daemon 安全行为零影响（L-9，结构性——无 store/secrets 依赖、无本地状态）。
- **性能/资源边界**：单命令复杂度 = O(一次 HTTP 往返 + 一页结果渲染)，无客户端聚合、无全量拉取（分页强制后端执行，§3.4）。内存上界由"一页响应体大小"决定（页大小经 daemon 钳制 ≤200），不随集合总量增长。连接数恒为"每命令 1 条、命令结束即关"（`mcp-stdio` 为 1 条长连，随宿主生命周期）。可设连接/读取超时，超时按 daemon 不可达类报错；无客户端重试（含 `409` 不自动重试，§3.5）。
- **测试策略**：核心打法是**对内存 Fake 控制面（UDS）做命令渲染测试**——起一个监听 `control.sock`（临时路径）的内存假服务端，对每条命令断言两侧：①**请求侧**（Fake 观测到的 method/path/query/body 恰为 6.5 对应行，含分页键与期望 `version`，对齐 F-2）；②**渲染侧**（喂定值 `Page<T>`/单条/`{error:..}`/含 `>2^53` 雪花 id 的响应，断言输出表格/字段/退出码恰为预期，对齐 F-4/F-5/L-3/L-4）。**不需要真实 daemon/容器**做绝大多数用例（CLI 无外部资源依赖）。少数运行期不变量是例外：L-9（崩溃不影响 daemon）须起**真实 daemon**、命令往返中途杀 CLI 进程、观察 daemon 行为无变化；`mcp-stdio` 字节保真（F-10）对**回声 Fake MCP 端点**做"写入字节序列 S = 读回 S 逐字节相等（含二进制/分片）"。`init` 多步编排（F-8）对 Fake 注入候选/缺口、断言 CLI 仅发起与呈现、调用序列 = 建资源→discover→回写。L-6 片段语义部分须人工评审（机器只能验"输入=授权事实、无额外固定文案"）。
- **可观测性**：CLI 自身运行日志取**最小**——只在生命周期/连接/异常层面记（如"连接 control.sock 失败"），逐请求细节不进 CLI 日志；**机密红线**：绝不记录凭据材料（`credential add` 的人类输入只转发、不落本地日志）、绝不记录真实地址（CLI 结构上无 `ResolvedTarget`/真实地址可触达），错误呈现只回显 daemon 已脱敏的 `message`（详细设计 7.5 内容红线的客户端侧落点）。CLI **不产出审计事件**——审计是 daemon 的 append-only 载体职责，CLI 只能经 `GET /v1/audit` 查询既有审计、从不写入。面向人的"事件"即命令的成败呈现与退出码，本身不是结构化指标流。

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

> 本节是 `postern-cli` 的**验收基准**：拿这份清单可逐条判定开发实现的"功能写全没、逻辑对不对"。每条 = **要求 + 通过判定**，通过判定对当前代码只有"通过/不通过"一个答案，无歧义、可复现；判定方式按条目而定（行为观察 / 接口存在 / Stele 契约绿红 / 结构检查），不强求都是单元测试。`postern-cli` 是依赖图末端的瘦客户端，其全部安全价值在于"零安全逻辑、零本地状态、每条命令=一次控制面往返+渲染"——验收据此判定。
>
> 说明：第 7 节"客户端崩溃不影响 daemon 安全行为"是**运行期不变量**，须起真实 daemon、杀 CLI 进程后观察 daemon 行为无变化判定（L-9，以行为观察判定，无对应静态契约）；`CLAUDE.md` 片段"只渲染事实、不编造话术"含语义判断，机器仅能验证"输入=授权事实、无额外固定文案"，剩余主观余量须**人工评审**（L-6 标注）。

### 一、功能完整性（判断：该有的功能都写了吗、行为对吗）

| 编号 | 要求（必须实现） | 通过判定（满足即过，否则不过） |
|---|---|---|
| F-1 命令组完整 | 命令树覆盖 §3 全表命令组：`daemon`/`init`/`resource`/`principal`/`role`/`credential`/`grants`/`elevate`/`revoke-grant`/`mode`/`freeze`/`constraint`/`condition`/`deny-note`/`settings`/`approvals`/`denials`/`audit`/`verify`/`export`/`import`/`mcp-stdio` | 枚举 clap 命令树 → 上述 22 个命令组全部存在且无遗漏；缺任一组即不过。场景规格 `docs/examples/06 §3.1`（elevate/revoke-grant/mode/freeze/credential revoke）、`docs/examples/07 §3`（audit/denials/verify/settings）、`docs/examples/02 §3`（init/resource/credential/export）中出现的命令逐条能在命令树找到对应入口 |
| F-2 命令=端点映射 | 每条子命令产出的 HTTP 方法/路径与 §3 表（=6.5）逐行一致；填充路径参数、查询参数（含 `page_no/page_size`）、请求体（写端点含期望 `version`） | 给 `postern elevate agent2 --cap redis-main:destroy --ttl 30m` → 产出请求恰为 `POST /v1/grants/temp` 且体含 `principal/capability/ttl`（对 `docs/examples/06 §3.1` 该行）；给 `postern audit --principal agent3 --page-no 1 --page-size 20` → 产出 HTTP 方法=`GET`、路径=`/v1/audit`、查询参数集恰含 `principal=agent3`/`page_no=1`/`page_size=20`（键值精确，顺序不限）（对 `docs/examples/07 §4.1-A`）；任一子命令映射到 6.5 未列端点即不过 |
| F-3 单次 HTTP-over-UDS 往返 | 一条命令对 `control.sock` 经 hyper+hyperlocal 发起**恰好一次**请求并接收响应；`mcp-stdio` 除外（数据面字节桥） | 对内存 Fake 控制面（UDS）跑一条集合命令 → Fake 侧观测到恰一次请求连接、无后台保活连接、命令结束连接关闭；观测到 0 次或 ≥2 次（隐式重试/保活）即不过 |
| F-4 信封与错误渲染 | 渲染 `Page<T>` 信封、单条响应、统一 `{error:{code,message}}` 错误信封为人类可读或机器形态（`--format jsonl`） | 喂 `Page<T>` JSON → 渲染出 items 表格且原样回显该响应携带的 `version`；喂 `{error:{code:"...",message:"..."}}` → 输出原样含该 `code`/`message`，不增删字符；`postern audit --format jsonl` → 输出逐行 JSON（每行可被 JSON 解析）。任一信封被改写/吞字段即不过 |
| F-5 雪花 id 字符串渲染 | 响应中 `id`/`principal_id`/`resource_id`/`credential_id` 等雪花 id 原样以字符串渲染，绝不数值化 | 喂含 id = `7300000000000000123`（>2^53）的响应 → 渲染输出该 id 字符串与输入逐字符相等（无精度丢失、无科学计数）；DTO 中 id 字段静态类型为字符串而非整型（构造签名检查），是整型即不过 |
| F-6 分页参数透传 | 集合命令把 `page_no/page_size` 作为查询参数透传给后端；缺省不传由后端取默认；CLI 不取回全量再本地切片 | 给 `--page-no 2 --page-size 50` → 产出查询串含 `page_no=2&page_size=50`；不给分页参数 → 查询串不含 `page_no/page_size`（由后端取默认 20）；CLI 源码内无"取回全量再切片"代码路径（构造签名检查）。契约 `DB_PAGINATION_MANDATORY` 绿 |
| F-7 乐观锁版本透传 | 读响应携带的 `version` 渲染给人，后续写/删命令原样回传；CLI 从不生成/递增 `version` | 读响应 `version=3` → 后续 `update`/`delete` 命令请求体期望 `version` 恰为先前读取的 3（原样回传）；CLI 源码无 `version` 自增/自造路径（构造签名检查）。对 `docs/examples/03 §4.2-F`、`docs/examples/06 §3.1` 写命令携带期望 version 的预期 |
| F-8 接入向导编排 `init` | 同一流程内顺序发起：建资源 `POST /v1/resources` → 触发 `POST /v1/resources/{code}/discover` → 呈现 daemon 报回候选/缺口 → 圈选回写 tier/细则/绑定 | 对内存 Fake，跑 `postern init` 主线观测到的控制面调用序列 = 建资源 → discover → 回写，与 `docs/examples/02 §4.1` 步骤 1→8 一致；CLI 仅发起与呈现，缺口/候选取自 Fake 回报。出现 CLI 自行探测或自行 tier 校验调用即不过 |
| F-9 `CLAUDE.md` 片段生成 | 片段内容只由控制面回报的授权事实（已授权动词、MCP 端点位置）渲染 | 喂授权事实 = {端点=`data.sock` 的 `/mcp`、已授权动词集=∅}（对 `docs/examples/02 §4.1` 步骤 8 尚未绑定时）→ 片段如实呈现"暂无已授权动词"且不含编造话术；喂非空动词集 → 片段列出且仅列出该动词集。片段含输入之外的任何固定引导话术即不过（语义部分见 L-6） |
| F-10 `mcp-stdio` 零逻辑字节桥 | 把宿主 stdin/stdout 与 `data.sock` MCP 端点双向**逐字节**转接，不解析、不归类、不脱敏、不增删字节 | 对回声 Fake MCP 端点，向桥写入字节序列 S → 从桥读回的字节序列与 S 逐字节相等（含任意二进制/分片）；桥代码路径无 `NormalizedRequest`/`Intent`/`Sanitizer` 引用（构造签名检查）。任一字节被改写/桥内出现解析即不过 |

### 二、逻辑正确性（判断：关键逻辑、边界、失败处理对不对）

| 编号 | 要求（行为必须正确） | 通过判定 |
|---|---|---|
| L-1 语法层本地拒绝、不误触网络 | 缺必填参数 / 互斥参数同给 / 格式非法（如 `--ttl` 非时长、`--cap` 非合法动词字面量）→ 本地拒绝；语义合法性判断一律不在本地 | 给 `postern elevate agent2 --cap redis-main:destroy`（缺 `--ttl`，对 `docs/examples/06 §4.2-13`）→ 非零退出 + 打印用法，且对 `control.sock` **零请求**；给 `--cap frobnicate`（非六动词，对 `docs/examples/03 §4.2-H`）→ 本地字面量校验拒绝、零请求。本地放过非法语法、或缺参却发起请求即不过 |
| L-2 daemon 不可达即拒绝（无路径回退） | `control.sock` 缺失 / 无权连 / daemon 未运行 → 报错，**绝不**回退本地策略或本地缓存决策 | 移除 socket 后跑任一命令（对 `docs/examples/06 §4.2-12`）→ 非零退出 + "daemon 不可达"错误，输出**无**任何决策结论（无 allow/deny/授权视图）。出现任何本地决策输出即不过 |
| L-3 响应不可解析即报错（不静默成功） | 响应不符合 core 共享类型契约（缺字段 / 类型错）→ 本地报错并非零退出，不猜测性补全、不忽略字段 | 喂缺 `decision` 字段的畸形 `DenyResponse` JSON → 非零退出 + 解析错误，**不**补默认值、**不**当成功渲染。静默成功或补全字段即不过 |
| L-4 结构化拒绝/错误原样转述 | `DenyResponse`/`{error:{code,message}}` 原样渲染；`message` 是 daemon 侧已脱敏常量文案，CLI 不展开、不补全、不推测、不重写 | 喂含 `reason`/`your_grants` 的 `DenyResponse` → 输出字段值与输入逐项相等，无 CLI 追加话术；喂含已脱敏 message 的错误信封 → 原样回显、不外泄真实地址（对 `docs/examples/02 §4.2` E1/E3/E4 回报经脱敏）。CLI 改写/补充/推测底层原因即不过 |
| L-5 `409 Conflict` 不静默重试 | 乐观锁版本不匹配 → 原样呈现冲突并提示重读最新 `version`，**不**自动重试覆盖 | 写命令收到 `409` → 呈现冲突 + "重新读取最新 version" 提示，且 Fake 侧观测到**无**后续自动重写请求（对 `docs/examples/02 §4.2-E7`、`docs/examples/03 §4.2-F`、`docs/examples/06 §4.2-10`）。出现自动重试覆盖即不过 |
| L-6 输出只转述事实（`CLAUDE.md` 片段不编造话术） | 错误信封、拒绝响应、`CLAUDE.md` 片段只渲染 daemon 给出的事实，不补全、不推测、不编造引导话术 | **机器部分**：片段输入=授权事实集，输出无输入之外的固定文案串（构造签名检查），有固定话术模板即不过；**人工部分**（标注：语义判断，须人工评审）：评审片段措辞确无编造引导话术——逐句 yes/no，全 yes 才过 |
| L-7 写失败不本地补偿 | 写命令收到失败（5xx / 三联动失败）→ 如实呈现，不假定任何部分生效、不在本地补偿（事务/快照/审计三联动全在 daemon） | 写端点返回 5xx → CLI 非零退出 + 如实呈现失败，且**无**任何本地"补写/回滚/重试"动作（对 `docs/examples/06 §4.2-14` 解冻写失败保持更严格侧）。CLI 自行补偿或假定部分生效即不过 |
| L-8 接入缺口呈现即停、不自行修补语义 | `init` 中 daemon 报回缺口（端口不可达 / tier 名实不符 / 2FA 须人在场）→ CLI 呈现缺口并发起"代修"控制面调用，缺口未消解前不标资源可用；修正动作语义合法性裁决仍在 daemon | 对 Fake 注入缺口（对 `docs/examples/02 §4.2` E3 端口不可达、E5 tier 名实不符、E2 2FA）→ CLI 呈现该缺口、"代修"仅转译为后续控制面写调用、CLI 不自行判定缺口是否消解；CLI 源码内无"声明 ⊆ 真实权限"比对或探测逻辑（构造签名检查，对 §4「不执行探测、不做 tier 子集校验」）。CLI 自行裁决缺口消解即不过 |
| L-9 客户端崩溃/缺席不影响 daemon 安全行为 | CLI 异常退出或缺席时，daemon 的求值/连接/审计照常 | （**运行期不变量，行为观察判定**）起真实 daemon，在一条命令往返中途杀掉 CLI 进程 → daemon 的求值/连接/审计行为与未杀 CLI 时**无差异**（同输入同决策、审计照常落、已建隧道不受 CLI 进程影响）。daemon 行为因 CLI 缺席而改变即不过 |
| L-10 `mcp-stdio` 中断即终止、不绕过 daemon | `data.sock` 不可连或转接中断 → 桥按错误终止该会话，不产生任何本地决策、不绕过 daemon | 转接中断 / `data.sock` 移除 → 桥终止会话并以错误退出，输出**无**任何本地构造的 MCP 响应或决策。桥伪造响应或绕过 daemon 即不过 |
| L-11 零本地状态、命令无状态 | 不持久化、不缓存策略/凭据/审计/决策；每条命令是无状态的一次往返，不读上次命令落地的本地状态（§7「零本地状态」） | 对内存 Fake 顺序跑两条读命令，命令间删除 CLI 进程的工作目录/缓存目录 → 第二条命令行为与单独首跑**完全一致**（不依赖任何上一命令落地的本地文件）；命令结束后磁盘上**无** CLI 写出的策略/凭据/审计/决策缓存文件（仅允许标准输出与人显式 `--format`/`export` 指定的目标文件）。CLI 落地任何本地状态缓存即不过 |
| L-12 接入侧 discover 不借数据面术语（CONS-20） | `init`/`resource discover` 只触发**控制面** `POST /v1/resources/{code}/discover`；CLI 不触达数据面 `postern_surface`、不在本地实现 MCP 工具面或能力发现语义（§4「不触达数据面能力发现」） | `postern resource discover <code>` 产出请求恰为 `POST /v1/resources/{code}/discover`（控制面端点），**不**产出任何 `postern_surface`/数据面 MCP 工具调用；CLI 源码内无 `postern_surface` 投影逻辑、无 `Adapter::discover` 直连（构造签名检查；`mcp-stdio` 外无任何数据面端点路径）。CLI 自行实现能力发现或调数据面 surface 即不过 |

### 三、边界与不变量（机器强制，绿/红即答案）

| 编号 | 要求 | 通过判定（机器） |
|---|---|---|
| B-1 不依赖 store/secrets（禁止边） | 依赖图无 `cli → store`、`cli → secrets` 边 | 契约 `ARCH_FORBIDDEN_EDGES` 绿；`cargo tree -p postern-cli -e normal` 无 `postern-store`/`postern-secrets`/`rusqlite` 边（依赖只含 `postern-core` + HTTP/UDS 客户端栈如 hyper/hyperlocal） |
| B-2 不含裸 SQL | CLI 源码内无 SQL 字符串、无 rusqlite | 契约 `DB_NO_RAW_SQL_OUTSIDE_STORE` 绿（`postern-cli` 内出现裸 SQL 字符串即红）；`DB_RAW_SQL_TEETH` 绿（store 外裸 SQL 反例被检出，证规则未空转） |
| B-3 不构造机密类型与来源 | CLI 内无 `ResolvedTarget`/`ResourceCredential`/`ConnOrigin` 构造点；机密类型不可 Clone/Serialize（即便误引用也无法持有） | 契约 `SEC_CONSTRUCTION_SITES` 绿（机密类型只在 secrets、`ConnOrigin` 只在 daemon shells 构造）；契约 `SEC_SECRET_TYPE_DISCIPLINE` 绿（`ResolvedTarget`/`ResourceCredential` 无 Clone/Serialize）；二者 `_TEETH` 反例自检均绿 |
| B-4 分页强制（不前端分页） | 集合命令透传分页、不取回全量再切片 | 契约 `DB_PAGINATION_MANDATORY`（+ `_TEETH`）绿 |
| B-5 lint 红线 | 无 unwrap/expect/panic/unsafe 等 | `cargo clippy -p postern-cli --all-features -- -D warnings` 退出码 0 |

### 通过定义（DoD）

`postern-cli` **算完成** ⟺ 一、二、三三组**每一条都通过**。任一条不过 = 不通过，必须修。F 类靠"枚举命令树 / 给定命令看产出请求是否符合判定"，L 类靠"触发某条件看行为是否恰为某可观察结果"（L-6 的人工部分逐句 yes、L-9 起真实 daemon 行为观察），B 类靠"跑契约/`cargo tree`/clippy 看绿红与退出码"。其中 **L-6（片段不编造话术）的语义余量须人工评审、L-9（崩溃不影响 daemon）须运行期行为观察**——已如实标注，仍以"满足某确切条件即过"的二元形式判定。
