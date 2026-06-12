# 场景 4 · Agent 数据面操作与使用业务系统

> Agent 经网关正常干活：查 `db-main` 数据(只读)、用业务系统 `svc-order` 的 HTTP 接口/页面下单(网关用 app 账号登录、维持会话、代 Agent 操作)、看 `docker-A` 的容器日志——全程只见资源代号、响应脱敏、会话复用不每次重登。本场景把"求值管线 [0]~[10] 全程跑通 + tier 选择 + 连接管理 + 传输 + 会话来源运行期物化 + 双侧匿名化/脱敏 + 入口对称"钉死成可验收预期。

本场景的资源代号、Agent、角色、tier 沿用 [01-多资源多Agent授权示例](01-多资源多Agent授权示例.md)。授权前置由 [02-资源接入与认证配置](02-资源接入与认证配置.md)(接入 + auth_flow)与 [03-权限分配与角色管理](03-权限分配与角色管理.md)(角色/绑定/细则)完成;本场景只覆盖**运行期数据面**。

---

## 1. 用户需求

> "我想让配好权限的 Agent 真正开始干活,而不是只配好了躺在那里。三件代表性的事,各由有对应权限的 Agent 来做:
>
> 1. **查库(`agent2` 做)**:`agent2` 是'B 数据库只读'那个,让它经网关读 `db-main` 上的业务数据(只读),比如查已支付订单——它只能看到 `db-main` 这个代号,不许知道库在哪台机器、什么 IP、用哪个账号连。
> 2. **用业务系统下单(`agent3` 做)**:`agent3` 管业务服务,让它经网关使用 `svc-order` 这个订单系统的 HTTP 接口/页面提交订单。网关替它用 app 账号登录订单系统、维持登录态、代它操作——`agent3` **永远拿不到也看不到那个 app 账号**,而且别每发一个请求就重登一次,太蠢也太慢。
> 3. **看日志(`agent3` 做)**:让 `agent3` 看 `docker-A` 上业务容器(`app-` 前缀)的日志排查问题——日志里如果带了内网 IP、客户邮箱手机号这类东西,**返回给它之前必须擦掉**。"

> 选 `agent2` 查库、`agent3` 下单与看日志,是为了让每条主线都**落在 01/03 既有授权矩阵内**(`agent2` 在 01 §3.2 对 `db-main` 有 `observe`+`query`;`agent3` 在 01 §3.3 对 `svc-order` 有业务操作、对 `docker-A` 有 `observe`),无需任何追加授权——本场景只演示运行期数据面,不引入新授权。

**痛点与覆盖的核心功能**:

- 三件事分属三种适配器(`postgres` / `http` 业务系统 / `docker`)、三种动词(`query` / `mutate` / `observe`),却必须走**同一条求值管线**(公理七)——本场景验证管线 [0]~[10] 对三类入口与三种适配器一致。
- "网关用 app 账号登录、维持会话、代操作、Agent 看不到账号"= **资源系统认证的运行期物化**(详细设计 6.13):live-session 缓存复用、续会话(账号密码重登 / OAuth 刷新)、single-flight、硬过期 fail-closed。
- "只见代号 + 响应擦敏感"= 请求侧**匿名化**(技术设计 9.1)+ 响应侧**脱敏**(系统级 ScrubSet + 声明级 mask_fields,技术设计 9.2 / 详细设计 6.4)。
- "动词选对账号、只读连接绝不被升格写"= **tier 选择在策略引擎**(管线 [6] 产出 `Allow{tier}`)、**连接管理只按已选 tier 建连**(管线 [7b]),职责分离 + 引擎兜底。

---

## 2. 预期实现路径

本场景三件事分由两个 Agent 发起(`agent2` 查库、`agent3` 下单与看日志,各落在 01/03 既有矩阵内)。每个数据面请求,无论经 MCP 工具还是 HTTP 外壳进入,都在 [0] 归一化后与外壳无关,完整走《详细设计文档》6.1 管线 `[0]→[10]`(对齐技术设计 5.1);任一步判定拒绝即短路进入拒绝响应组装。三件事的差异只在 [1] 认证出的 `PrincipalId`、[2] 归类结果、[6] 选中的 tier、[7b] 取连接的方式,**骨架完全一致**(公理七、详细设计 6.1)。

**控制面 vs 数据面**:本场景全部是**数据面动作**(求值 + 执行)。授权、接入、auth_flow 写入、细则声明都是控制面动作,已在场景 02/03 完成,本场景不重复;运行期一切凭据物化、会话续期均**无人值守**(详细设计 6.13 运行期物化、技术设计公理二"人不在环是常态")。

**逐件路径**:

1. **查库(`agent2` · `db-main` query)** — 管线 [2] `postgres` 适配器 `classify` 把 SQL 经**语法树解析**归为 `Query`(技术设计 5.2 协议感知,非文本匹配);[3] RBAC 命中 `agent2` 在 `db-main` 上的 `(db-main, query)` 格(`agent2`'B 数据库只读',01 §3.2 对 `db-main` 授 `observe`+`query`——该格在既有矩阵内,无需追加授权);[6] 策略引擎按动词 `query` 选 `ro`(SELECT-only 账号);[7b] `connpool.acquire(db-main, ro)`,池键 `(db-main, ro)`,经 `ssm` Transport 建到 B 的本地 socket 通路(技术设计第四部分·⑥传输层"把远端接入抽象为本地 socket"、详细设计 6.3);[8] 以 SELECT-only 账号执行;[9] `Sanitizer::scrub` 系统级 ScrubSet 擦真实地址/凭据 + 声明级 `column_mask` 在结果带出 `public.customers.email/phone` 时掩码这些 PII 列(详细设计 6.4);[10] 结果审计落 **outcome**(query 为只读动词,沿用执行后单次审计,不走两阶段 intent,详细设计 6.1)。
2. **下单(`agent3` · `svc-order` mutate)** — 管线 [2] `http` 适配器 `classify` 把 `POST /api/orders` 归为 `Mutate`;[4] 细则 `http_route` 白名单校验路由(01 §3.3 b3-1);[5] 条件 `time_window` 校验工作时段(01 §4 conditions);[6] 策略引擎选 `op`(订单系统业务操作账号);[7a] 有副作用动词先落 **intent** 审计(详细设计 6.1 两阶段);[7b] 取连接时经 `CredentialProvider.credential_for(svc-order, op)` 查 **live-session 缓存**——命中且未临近过期则**直接复用(不重登)**,临近过期则 single-flight 续会话,硬过期则 fail-closed(详细设计 6.13 运行期物化);活跃会话以"请求注入描述"(Cookie / CSRF 回填)交 `http` 适配器贴到转发请求;[8] 转发到订单系统;[9] 脱敏(`mask_fields` 擦响应里 `customer.email`/`phone`);[10] 落 **outcome** 审计(与 intent 同请求 id 关联)。
3. **看日志(`agent3` · `docker-A` observe)** — 管线 [2] `docker` 适配器 `classify` 把"取容器日志"归为 `Observe`;[3] RBAC 命中 `agent3` 对 `docker-A` 的 observer 绑定(01 §3.3 b3-2);[4] 细则 `container_prefix=app-` 校验容器名前缀;[6] 策略引擎选 `logs`(只读日志 API,非裸 socket);[7b] `connpool.acquire(docker-A, logs)`,经 `ssh` Transport 建通路;[8] 经只读日志端点取日志(流式大输出);[9] **流式脱敏**——跨 chunk 滑动重叠窗口擦内网 IP/PII,有界缓冲与背压(详细设计 6.4 流式脱敏模型);[10] 审计。

**匿名化贯穿请求侧**:每个 Agent 在数据面只能以自身 Scope 内的资源代号发起——`agent2` 见 `db-main`、`agent3` 见 `svc-order`/`docker-A`;代号↔真实地址映射只存机密面,数据面无读取路径(技术设计 9.1、详细设计 9.1);二者均**无法枚举** Scope 外资源、无法持有任何 tier 账号(技术设计 9.3 硬保证)。

---

## 3. 页面与操作

### CLI(`postern ...`,人/脚本用)

数据面发起本身**不是** CLI 职责(CLI 是控制面瘦客户端,模块 07 §4"不承载数据面外壳形态")。与本场景相关的 CLI 仅为**人侧旁观/排障**:

- `postern grants agent2` / `postern grants agent3` — 看授权视图(与 Agent 收到的 `your_grants` 同源,控制面 `GET /v1/grants/{principal}`),确认 `agent2` 有 `db-main:query`、`agent3` 有 `svc-order:mutate`、`docker-A:observe`。
- `postern audit --principal agent3 --since 1h` — 查 `agent3` 本场景请求的审计事件(`GET /v1/audit`,分页倒序);mutate 应见 intent + outcome 一对。`agent2` 的查库审计同理(`--principal agent2`)。
- `postern mcp-stdio` — 仅供只支持 stdio 的 MCP 宿主作字节桥转接到 `data.sock` 的 `/mcp`(模块 07 §6,零逻辑搬运,不解析/不脱敏)。

### 管理前端 SPA(人用,控制面 API 的视图)

- **授权视图页**:分别渲染 `GET /v1/grants/agent2` 与 `GET /v1/grants/agent3`,以 (资源×动词) 表展示各 Agent 能干什么——给运维"`agent2` 能查 `db-main`、`agent3` 能下单 `svc-order` 并看 `docker-A` 日志"的确认面。
- **审计流页**:渲染 `GET /v1/audit?principal=agent2` 与 `?principal=agent3`,人可见每条请求的 decision/tier/capability/objects/response_digest(**不含响应内容明文**),mutate 行可下钻看 intent↔outcome 配对。
- **拒绝聚合页**:渲染 `GET /v1/denials/summary`,本场景若触发异常(见 §4.2)会在此聚合显形。
- SPA **不发起数据面请求**——它是控制面 API 的视图,数据面发起者只能是 Agent(公理七、控制面/数据面隔离,技术设计 2.3)。

### 数据面(Agent 用,MCP 工具 / HTTP 外壳)

每个 Agent 经**同一套固定动词工具面**(详细设计 6.8)发起,工具集不随授权增删、对所有 Agent 一致、描述只含事实(本场景三件事各由有对应授权格的 Agent 调,无授权格者调同一工具在 [3] RBAC 被拒):

- `postern_query(resource="db-main", request="<SQL>")` — 查库(本场景 `agent2` 调)。
- `postern_mutate(resource="svc-order", request={method, path, body})` — 下单(本场景 `agent3` 调)。
- `postern_observe(resource="docker-A", request={action:"logs", container:"app-order", ...})` — 看日志(本场景 `agent3` 调)。
- `postern_grants()` — 取自身 `your_grants` 事实(自助分流用)。
- `postern_surface()` — 取自身 Scope 内**已授权能力面**(授权快照投影,**禁止触达** `Adapter::discover`、不触底层资源,详细设计 6.8 CONS-20)。

**HTTP 外壳同管线**:同一请求经 `data.sock` 的 HTTP 数据面端点(axum router 挂 `data.sock`,模块 06 §3.3)进入,[0] 归一化为同一 `NormalizedRequest` 后与 MCP 完全一致(公理七)——本场景任何一件事经 MCP 工具或 HTTP 外壳发起,鉴权/匿名化/脱敏/审计结果**完全一致**。

---

## 4. 使用方式与预期结果

### 4.1 正常操作

> 三条 trace 各走 `[0]→[10]`,每步给"操作 → 可验收的预期结果"。

#### Trace ① · `agent2` 查 `db-main`(query · tier=ro · SSM)

操作:`agent2` 经 MCP 调
`postern_query(resource="db-main", request="SELECT id, status, customer_id FROM public.orders WHERE status='paid'")`。

| 步骤 | 操作 → 预期结果 |
|---|---|
| [0] 外壳归一化 | MCP 工具调用 → `NormalizedRequest{ presented=agent2 凭证, origin=UnixPeer{uid,gid}, resource="db-main", intent=SQL 文本 }`。**预期**:自此请求与外壳无关(公理七);`origin` 取自网关可观测的连接来源,不采信请求自报字段(技术设计 5.1[1])。 |
| [1] 认证+可信域 | 校验 `agent2` 凭证有效、未过期/未吊销,`origin` 在可信域内。**预期**:得 `PrincipalId(agent2)`;任一不成立 → deny(fail-closed),不进 [2]。 |
| [2] 语义归一化 | `postgres` 适配器语法树解析:纯只读 `Query`(无写节点、无 `INTO`)。**预期**:`Capability=Query, objects=["public.orders"]`;无法可靠归类 → deny(技术设计 5.2)。 |
| [3] RBAC | 快照查表 `(db-main, query)` 格:`agent2`'B 数据库只读'在 01 §3.2 对 `db-main` 授 `observe`+`query`,该格在既有矩阵内。**预期**:命中 `(db-main, query)` 格则继续;无格 → deny(公理一,fail-closed)。 |
| [4] 细则 | kernel 先跑 `Adapter::check_constraint`:`db-main` 的 `table_allow` 含 `public.orders`、`column_mask` 标记 PII 列。**预期**:`ConstraintCheck{passed=true}`;查询触及白名单外表 → deny。 |
| [5] 条件 | `db-main` query 无附加条件谓词。**预期**:通过(若有 time_window 等谓词,任一 false → deny)。 |
| [6] 动作分流 | `Decision::Allow{ grant, tier }`:策略引擎按动词 `query` 选承载它的 `tier=ro`(SELECT-only 账号)。**预期**:产出 `Allow{tier=ro}`;**tier 选择落在策略引擎,不在连接层**(速查表职责分离)。 |
| [7a] 意图审计 | `query` 是只读动词,无副作用。**预期**:沿用执行后单次审计,不走两阶段 intent(详细设计 6.1)。 |
| [7b] 取连接 | `connpool.acquire(db-main, ro)`,池键 `(db-main, ro)`;机密面解析 `(target, credential[ro])` → `ssm` Transport 建到 B 的本地 socket 通路 → `Channel`;**凭据引用即时释放**。**预期**:返回到 `db-main` 的 `ro` 通路(池中复用或新建);无承载 query 的 tier → deny;不可建 → deny(fail-closed)。 |
| [8] 执行 | `postgres` 适配器 over `Channel` 以 SELECT-only 账号执行 → `RawResponse`。**预期**:返回已支付订单行;即便 SQL 被误归类,`ro` 账号在引擎层只有 SELECT,写被引擎直接拒(`engine_enforced=true` 兜底)。 |
| [9] 脱敏 | `Sanitizer::scrub`:系统级 ScrubSet 擦任何真实地址/凭据回显;声明级 `column_mask` 在结果触及 `public.customers.email/phone` 时掩码这些列(本查询仅取 `public.orders` 列,column_mask 对其为无操作;若查询联表带出 customers PII 则掩码生效)。**预期**:返回的订单行无任何真实 IP/连接串;凡带出的 PII 列一律掩码。 |
| [10] 结果审计 | `AuditSink::record`:`kind=request, decision=allow, tier="ro", capability=query, objects=["public.orders"], response_digest=sha256(...)`(**不含内容**)。**预期**:`postern audit --principal agent2` 可见此条。 |

**可验收净结果**:`agent2` 收到一组脱敏后的已支付订单数据(PII 列掩码),全程只见 `db-main` 代号,审计留痕。

#### Trace ② · `agent3` 用 `svc-order` 下单(mutate · tier=op · 会话复用/续期 · 脱敏)

操作:`agent3` 经 HTTP 外壳调
`postern_mutate(resource="svc-order", request={method:"POST", path:"/api/orders", body:{...}})`。

| 步骤 | 操作 → 预期结果 |
|---|---|
| [0]~[1] | 归一化 → `NormalizedRequest{ presented=agent3 凭证, resource="svc-order", intent=HTTP 负载 }`;认证得 `PrincipalId(agent3)`。**预期**:同 Trace ①,入口对称。 |
| [2] 语义归一化 | `http` 适配器:`POST /api/orders` → `Capability=Mutate, objects=[route:/api/orders]`。**预期**:正确归 Mutate。 |
| [3] RBAC | `agent3` —operator→ scope(`env:prod AND kind:business` 含 `svc-order`,01 §3.3 b3-1)→ (svc-order, mutate) 格存在。**预期**:命中 ✅。 |
| [4] 细则 | `http_route` 白名单含 `{POST /api/orders}`。**预期**:`passed=true`;白名单外路由(如 `POST /api/admin/*`)→ deny。 |
| [5] 条件 | `svc-order:mutate` 挂 `time_window=Mon-Fri 09:00-18:00`;当前在窗口内。**预期**:通过;窗口外 → deny(公理二)。 |
| [6] 动作分流 | `Decision::Allow{ tier=op }`:策略引擎按动词 `mutate` 选业务操作账号 `tier=op`。**预期**:产出 `Allow{tier=op}`。 |
| [7a] 意图审计 | `mutate` 有副作用 → 两阶段审计:先落 **intent** 事件(执行前)。**预期**:intent 写成功才继续;intent 写不进 → 执行前 deny(**确未执行**,详细设计 6.1)。 |
| [7b] 取连接(会话复用) | `connpool.acquire(svc-order, op)` → `CredentialProvider.credential_for(svc-order, op)` 查 **live-session 缓存**(进程内、Zeroizing、键 `(svc-order,op)`):**命中且未临近过期 → 直接复用缓存会话(★不重登)**;活跃会话以"请求注入描述"(`Set-Cookie:JSESSIONID` + 回填 CSRF 头)交 `http` 适配器。**预期**:绝大多数下单**复用现有登录态**,不触发任何登录请求(详细设计 6.13 运行期物化);`agent3` 全程无从看见/持有 app 账号(公理四)。 |
| [7b'] 取连接(临近过期续会话) | 若缓存会话临近过期(`expiry−skew=120s`):single-flight 触发续会话(第①档账号密码 → 用 vault 账号密码无人值守重登;第③档 → refresh token 刷新),**旧会话在新会话就绪前仍有效(重叠窗口)**,在用请求不中断,续成功回填缓存。**预期**:续期对 `agent3` 透明,本次下单仍成功;并发下单复用同一在途续会话,无登录风暴(详细设计 6.13 单飞)。 |
| [8] 执行 | `http` 适配器贴 Cookie/CSRF,转发 `POST /api/orders` 到订单系统 → `RawResponse`。**预期**:订单创建成功,返回订单凭据(如订单号);引擎兜底落在**订单系统自身权限体系**——`op` 账号本就只能下单/改单(详细设计 6.13、技术设计 10.6)。 |
| [9] 脱敏 | `Sanitizer::scrub`:系统级擦真实地址/凭据;`mask_fields` 擦响应里 `customer.email/phone`。**预期**:返回的下单结果中 PII 已掩码。 |
| [10] 结果审计 | 落 **outcome** 事件(与 intent 同请求 id 关联):`decision=allow, tier="op", capability=mutate`。**预期**:`postern audit` 可见 intent↔outcome 一对;**已执行的请求绝不返回 deny**,outcome 写失败 → 返回"已执行但审计降级"错误码而非 deny(详细设计 6.1)。 |

**可验收净结果**:订单成功创建,返回脱敏后的下单结果;网关用 app 账号代操作且会话被复用(典型路径不重登),`agent3` 自始至终不接触账号;审计有 intent+outcome 两痕。

#### Trace ③ · `agent3` 看 `docker-A` 容器日志(observe · tier=logs · 流式脱敏)

操作:`agent3` 经 MCP 调
`postern_observe(resource="docker-A", request={action:"logs", container:"app-order", tail:200})`。

| 步骤 | 操作 → 预期结果 |
|---|---|
| [0]~[1] | 归一化 + 认证 → `PrincipalId(agent3)`。**预期**:同上,入口对称。 |
| [2] 语义归一化 | `docker` 适配器:取容器日志 → `Capability=Observe, objects=[container:app-order]`。**预期**:归 Observe。 |
| [3] RBAC | `agent3` —observer→ resource `docker-A`(01 §3.3 b3-2)→ (docker-A, observe) 格存在。**预期**:命中 ✅;注意 `agent3` 对 `docker-A` **无 manage/destroy 格**(只能看日志,不能重启,与 `agent2` 精确区分)。 |
| [4] 细则 | `container_prefix=app-` 校验 `app-order` 前缀匹配。**预期**:`passed=true`;请求非 `app-` 前缀容器(如 `app-` 外的系统容器)→ deny。 |
| [5] 条件 | `docker-A` observe 无附加条件谓词。**预期**:通过(若有 time_window 等谓词,任一 false → deny)。 |
| [6] 动作分流 | `Decision::Allow{ tier=logs }`:策略引擎选 **只读日志 API**(`logs`,非裸 socket)。**预期**:产出 `Allow{tier=logs}`——`docker.sock` 即 root,本路径绝不走全 socket(01 §1.2、5.2bis ④)。 |
| [7a] 意图审计 | `observe` 是只读动词,无副作用。**预期**:沿用执行后单次审计,不走两阶段 intent(详细设计 6.1)。 |
| [7b] 取连接 | `connpool.acquire(docker-A, logs)`,经 `ssh` Transport 建到 A 的本地 socket 通路。**预期**:到 `docker-A` 的 `logs` 通路;不可建 → deny。 |
| [8] 执行 | `docker` 适配器经**只读日志端点**取 `app-order` 日志(**流式大输出**)。**预期**:返回该容器尾部日志流。 |
| [9] 流式脱敏 | `Sanitizer::scrub` 按**流式模型**:跨 chunk 滑动重叠窗口(保留上一 chunk 尾部 N 字节参与下一 chunk 匹配,N=ScrubSet 最长模式长度上界),擦日志里的内网 IP/真实主机名/PII;有界缓冲与背压(详细设计 6.4)。**预期**:返回日志中任何内网 IP、真实地址、凭据回显已擦除,**敏感串不因恰好跨 chunk 边界而逃逸**。 |
| [10] 审计 | `AuditSink::record`:`decision=allow, tier="logs", capability=observe, objects=["container:app-order"]`。**预期**:审计留痕,不含日志正文。 |

**可验收净结果**:`agent3` 收到 `app-order` 容器的脱敏日志流,只见 `docker-A` 代号、只触及 `app-` 前缀容器、走只读日志端点;审计留痕。

---

### 4.2 异常场景与预期结果

> 每条写"触发条件 → 系统行为 → 预期结果",一律 fail-closed:不确定即拒绝、凭据零接触、只说事实、匿名化代号、响应脱敏。

#### A. SQL 伪装写被归类拦截(`engine_enforced` + 归类双防线)

- **触发**:`agent2`(持 `db-main:query`)经 `postern_query(resource="db-main", request="WITH x AS (DELETE FROM public.orders RETURNING *) SELECT * FROM x")`——把 `DELETE` 用公共表表达式(CTE)包进 `query` 外壳里,企图借只读权限偷偷写。
- **系统行为**:管线 [2] `postgres` 适配器语法树解析识破 CTE 内的写节点,按**最高危写节点**归为 `Destroy`(不是 Query,详细设计 6.7 第 2 项、技术设计 5.2);[3] RBAC 查 `(db-main, destroy)` 格——`agent2` 仅有 `query`、无 `destroy` 格 → deny(公理一);即便归类被绕过,[6]/[7b] 选中的 `ro` 是 SELECT-only 账号,引擎层也拒任何写(`engine_enforced=true` 兜底)。**短路,不执行 [8]**——确未触及数据库,无任何删除副作用。
- **预期结果**:返回结构化拒绝 `decision=deny, stage=rbac, reason="语句含写节点,归类为 destroy;role 不含 db-main:destroy"`;`public.orders` **零行被删**;deny 进审计(`postern audit` 可见 `kind=request,decision=deny`);拒绝响应经同一 Sanitizer 出口、不泄露真实地址。

#### B. 业务系统会话过期——可续 vs 硬过期

- **触发 B1(临近/已软过期,可续)**:`agent3` 下单时 live-session 缓存会话临近或刚过期,但持久凭据(账号密码 / refresh token)仍有效。
  - **系统行为**:[7b] single-flight 触发续会话——第①档用 vault 账号密码无人值守重登、第③档用 refresh token 刷新,旧会话在新会话就绪前仍有效(重叠窗口),续成功回填缓存(详细设计 6.13)。
  - **预期结果**:本次下单**仍成功**,续期对 `agent3` 透明,无人值守完成;`credential_event` 记一条续会话(只记"哪个会话/账号/tier",**绝不记账号密码或令牌值**)。
- **触发 B2(硬过期,不可续)**:账号密码已失效 / refresh token 失效 / 系统每次登录强制 2FA 且无可刷新长效会话。
  - **系统行为**:[7b] 续会话失败 → **fail-closed**:该请求 deny;**绝不在数据面静默重登、绝不触发 2FA**(2FA 只在接入期控制面、人在场,详细设计 6.13)。
  - **预期结果**:返回 `decision=deny, reason="资源会话不可建立/已过期,需经控制面更新凭据(账号密码失效则更新,OAuth/2FA 系统则重新接入)"`,附 `operator_note`;`agent3` **未拿到任何账号信息**,只得到代号化的拒绝事实;运维在 SPA/审计可见提示去控制面重接。

#### C. 响应里真实 IP / PII 被脱敏

- **触发**:`agent3` 看 `docker-A` 日志,日志正文里业务程序打了内网 IP `10.x.x.x`、客户邮箱手机号;或诱导回显——故意查一条回显连接串的请求。
- **系统行为**:[9] 系统级 ScrubSet 擦真实地址/凭据(流式滑动窗口防跨 chunk 逃逸);声明级 `mask_fields`/`column_mask` 擦 PII;拒绝响应、错误信息同样过 Sanitizer(技术设计 9.2、详细设计 6.4、红线 7.2-3)。
- **预期结果**:返回内容中真实 IP、连接串、密钥/令牌回显已擦除,PII 字段掩码;**诚实边界**——系统级脱敏是黑名单尽力而为,真正硬保证来自请求侧匿名化(Agent 无从指定真实地址)与凭据零接触(技术设计 9.3);高敏感字段以白名单输出更优。`agent3` 拿不到任何可还原拓扑/凭据的字节。

#### D. 连接不可建 → deny

- **触发**:`agent2` 查 `db-main`,但到 B 的 `ssm` 通路建立失败(网络不可达 / 会话通道开启失败 / 健康检查失败)。
- **系统行为**:[7b] `connpool.acquire` 无法建立到 `(db-main, ro)` 的通路 → deny(fail-closed,技术设计 5.1[7]、详细设计 6.3);**不降级、不静默重试到其他通路**。
- **预期结果**:返回 `decision=deny, stage=connect, reason="资源连接不可建立"`(脱敏,不含真实地址/失败主机名);deny 进审计;`agent2` 只知"暂不可达"事实,不知拓扑。

#### E. 超并发上限 → 背压或 deny

- **触发**:`agent3` 对 `svc-order`、`agent2` 对 `db-main`(或多 Agent 叠加)并发请求超过每资源/全局并发上限。
- **系统行为**:连接管理层**有界排队**或超限 deny(fail-closed,详细设计 6.3);observe 类大流(看日志)按**有界缓冲 + 背压**——下游消费慢时对上游施压而非无界缓冲(详细设计 6.4)。
- **预期结果**:超限请求要么有界排队后服务、要么得确定的 `decision=deny, reason="并发超限"`,**绝不无界堆积导致内存爆**;背压可观察(不丢正确性)。

#### F. 跨 Principal 会话不串味(会话净化)

- **触发**:`agent2` 的 `db-main:ro` 请求执行后连接归池;随后另一 Principal(或 `agent2` 另一请求)复用同一 `(db-main, ro)` 池连接。
- **系统行为**:归池前**强制会话净化**——PostgreSQL 类跑 `DISCARD ALL`、重置 `search_path`、回滚未决事务、清临时表/会话变量;净化失败的连接**销毁不归池**(fail-closed);对会话副作用无法可靠净化的形态禁用复用(即建即用即弃);对会话态敏感适配器评估把 `PrincipalId` 纳入池键隔离(详细设计 6.3)。
- **预期结果**:复用连接**不携带**上一请求遗留的会话态/临时表/事务,跨请求/跨 Principal **零串味**;不同 tier 连接绝不共享(只读连接绝不被升格执行写)。

#### G. Agent 试图直接拿 app 账号(拿不到)

- **触发**:`agent3` 诱导网关交出 `svc-order` 的 app 账号——如 `postern_query` 伪造一条"回显认证配置/Cookie/账号"的请求,或 `postern_surface` 想读 tier 账号,或请求 Scope 外资源代号探测。
- **系统行为**:tier 账号、auth_flow 敏感项、代号↔真实地址映射全存机密面,**数据面无读取路径**(技术设计 9.1、详细设计 6.13/8.8);`postern_surface` 只返回授权快照投影、**禁止触达** `Adapter::discover`、不触底层资源(详细设计 6.8 CONS-20);凭据零接触贯穿正常/错误/拒绝/日志全部出口(公理四);任何回显凭据/账号的内容被 [9] ScrubSet 擦除;Scope 外资源代号访问 → deny 且**不泄露该资源是否存在**(详细设计 6.7 第 4 项)。
- **预期结果**:`agent3` **永远拿不到 app 账号/密码/Cookie/token/真实地址**——无论正常响应、错误信息、`surface` 投影还是诱导回显;只得到代号与已授权动词面;越界探测得脱敏拒绝且不泄露存在性。

#### H. daemon 重启后数据面恢复

- **触发**:操作期间 daemon 重启,随后 `agent2` 再发 `postern_query`(db-main)、`agent3` 再发 `postern_mutate`(svc-order)。
- **系统行为**:重启后 boot 序列重建 `PolicySnapshot`、解锁保险箱、开放数据面(模块 06 §3.1);live-session 缓存(内存、Zeroizing)随进程丢失 → 下次建连按持久凭据**重新登录/刷新**回填(svc-order 会话需重建;db-main 的 `ro` 是 SELECT-only 账号、无业务系统会话态,直接按凭据建连)(详细设计 6.13);TTL/临时授权从权威库恢复并续计时(技术设计 6.2);**任何挂起审批恒 deny**(fail-closed,详细设计 6.10)。
- **预期结果**:重启后两个 Agent 的请求均按恢复后的权威策略正常裁决并执行;`agent3` 的 `svc-order` 会话凭据自动重建(对 `agent3` 透明,典型档无需人介入);**绝无**跨重启复活的危险挂起操作。

#### I. 工具/资源代号不存在或拼错

- **触发**:`agent3` 调 `postern_query(resource="db-secret", ...)`——`db-secret` 不在 `agent3` Scope 内(可能根本不存在,也可能存在但未授予)。
- **系统行为**:[3] RBAC 无对应授权格 → deny;拒绝响应**只述 `agent3` 自身授权世界**(`DENY_RESPONSE_SCOPE_BOUNDED`),不区分"不存在"与"无权"——两者返回不可分(详细设计 6.7 第 4 项、技术设计 9.1 不可枚举)。
- **预期结果**:返回 `decision=deny`,`your_grants` 只含 `agent3` 自己的资源/动词,`request_hint` 为策略机械生成(若适用);**不泄露** `db-secret` 是否存在、不泄露信任域内其他资源(公理六、匿名化)。

---

> **小结**:本场景把"Agent 正常干活"的三件事(查库 query / 下单 mutate / 看日志 observe)钉死为可验收预期——管线 [0]~[10] 全程、tier 选择(策略引擎产出 `Allow{tier}`)、连接管理(池键 `resource×tier`、归池前会话净化)、传输(SSH/SSM 建本地 socket)、会话来源运行期物化(live-session 复用 / 续会话单飞 / 硬过期 fail-closed)、双侧匿名化+脱敏(系统级 ScrubSet + 声明级 mask_fields)、入口对称(MCP/HTTP 同管线)。异常一律 fail-closed,凭据零接触,响应脱敏,代号化,只说事实。
