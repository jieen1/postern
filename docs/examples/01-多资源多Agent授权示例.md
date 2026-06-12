# 示例一 · 多资源、多 Agent 的授权落地（worked example）

> 本篇是一份**端到端的设计示例**，把《技术设计文档》与《详细设计文档》的授权模型套用到一组具体场景上，展示"异构资源建模 → 业务系统账号认证声明 → 三个 Agent 的授权矩阵 → 声明式 TOML → 求值管线 trace → redis 只读落地"如何在 postern 的设计里自洽闭合。
>
> 本篇是**设计示例**，不含实现代码、不含阶段划分或进度。术语、字段名、约束语义全部对齐《详细设计文档》5.2 schema、5.2bis 授权落地、6.6 声明式导入/导出、6.13 资源系统认证与会话凭据生命周期、第八部分领域章。与上层冲突时以《技术设计文档》七公理与《详细设计文档》第八部分为准。
>
> 文中一切真实地址、账号、密码、令牌均以 `vault://` 引用出现，**绝不写明文**（11.2 不变量、公理四）。资源代号是 Agent 唯一可见的对象（3.2 匿名化）。

---

## 0 · 场景设定

两台服务器、两条 Transport：

| 服务器 | 到达方式（Transport） | 说明 |
|---|---|---|
| A | `ssh` | 业务前端机：跑 3 个业务服务（HTTP）+ docker 跑 2 个容器化服务 |
| B | `ssm`（AWS Systems Manager 会话） | 数据层机：数据库、redis、rabbitmq |

A 上的"业务服务"有两个互相正交的操作面（5.2bis ③ 同一物理服务 → 多资源）：

- **用它**：经业务系统账号调其 HTTP 接口/页面、提交表单、跑业务流程 → `http` 适配器资源 + **业务系统账号 tier**（6.13 会话来源）。
- **管它**：重启/扩缩/部署、看容器日志 → `docker` 适配器资源 + 基础设施凭据。

三个 Agent 的授权意图（用户给定）：

- **agent1**：A、B 上所有服务**只读**。
- **agent2**：A 的 docker **管理** + B 的数据库**只读** + B 的 redis **管理**。
- **agent3**：业务服务**管理（用系统跑业务流程）** + docker **查日志** + B 上所有服务**日志**。

> 注意"管理"一词在不同 Agent 处指向不同资源面：agent2 的"docker 管理"是 `docker` 资源的 `manage`（重启容器）；agent3 的"业务服务管理（用系统跑业务流程）"是 `http` 业务资源的 `mutate`（提交表单、推进流程），**不是**重启服务。"用/管拆多资源"正是为了让这两种"管"互不串味。

---

## 1 · 资源建模表

一个资源 = `(代号 + Adapter + Transport + 标签 + 凭据等级集合 + engine_enforced)`（3.5、10.5、5.2bis ③）。同一物理服务按操作面拆多资源。

### 1.1 资源清单

| 资源代号 | adapter | transport | 标签（host/env/kind） | engine_enforced | 物理目标（仅示意，明文只存机密面） |
|---|---|---|---|---|---|
| `svc-order` | http | ssh | host:A, env:prod, kind:business | 取决于业务系统 RBAC（声明） | A 上订单服务（80 端口经 ssh 转发） |
| `svc-billing` | http | ssh | host:A, env:prod, kind:business | 取决于业务系统 RBAC（声明） | A 上计费服务 |
| `svc-crm` | http | ssh | host:A, env:prod, kind:business | 取决于业务系统 RBAC（声明） | A 上 CRM 服务 |
| `docker-A` | docker | ssh | host:A, env:prod, kind:docker | true（账号即 socket 权限） | A 上 docker 运行时（含 2 个容器化服务 + 3 个业务服务容器） |
| `db-main` | postgres | ssm | host:B, env:prod, kind:database | true | B 上 PostgreSQL |
| `redis-main` | redis | ssm | host:B, env:prod, kind:cache | redis 6+ ACL=true；旧版=false（如实声明，见 §6） | B 上 redis |
| `mq-main` | rabbitmq | ssm | host:B, env:prod, kind:mq | 取决于 rabbitmq 用户权限（声明） | B 上 rabbitmq |

> **"用/管拆多资源"在表里的体现**：A 上 3 个业务服务的"用"= `svc-order`/`svc-billing`/`svc-crm`（http + 业务账号），"管"=同一物理目标的 `docker-A`（docker 资源）。一个 Agent 能 `query` 业务接口（`svc-order:query`）却不能重启它（`docker-A:manage` 未授）——精确正交（5.2bis ③）。
>
> **adapter 说明**：`rabbitmq` 适配器在《技术设计文档》10.2 的"已知形态"中以集群/消息类资源出现，本示例按其 management API 性质建模为 http 同构的应用层资源；其 `engine_enforced` 取决于 rabbitmq 用户/vhost 权限（与 http 业务系统同构）。`redis` 适配器见 §6。

### 1.2 凭据等级（tier）表

每个 tier = 一份资源凭据账号 + 它承载的动词集（capabilities）（10.5、5.2 `resource_credential_tiers`）。**策略引擎按本次动词选 tier**（动词→tier 映射归策略引擎，速查表 CredentialTier 选择），连接管理只按已选 tier 建连。

| 资源 | tier | 对应账号（机密面持有，Agent 永不见） | 承载动词（capabilities） | secret_ref |
|---|---|---|---|---|
| `svc-order` | `ro` | 订单系统**只读业务账号** | observe, query | `vault://svc-order/ro` |
| `svc-order` | `op` | 订单系统**业务操作账号**（可下单/改单） | mutate | `vault://svc-order/op` |
| `svc-billing` | `ro` | 计费系统只读账号 | observe, query | `vault://svc-billing/ro` |
| `svc-billing` | `op` | 计费系统操作账号 | mutate | `vault://svc-billing/op` |
| `svc-crm` | `ro` | CRM 只读账号 | observe, query | `vault://svc-crm/ro` |
| `svc-crm` | `op` | CRM 操作账号 | mutate | `vault://svc-crm/op` |
| `docker-A` | `logs` | docker **只读日志 API**（只读端点，非裸 socket） | observe | `vault://docker-A/logs` |
| `docker-A` | `admin` | docker **全 API**（socket 权限即 root，谨慎） | manage, destroy | `vault://docker-A/admin` |
| `db-main` | `ro` | PostgreSQL **SELECT-only 账号** | observe, query | `vault://db-main/ro` |
| `db-main` | `rw` | PostgreSQL 读写账号 | mutate | `vault://db-main/rw` |
| `redis-main` | `ro` | redis **只读 ACL 账号**（`+@read -@write -@dangerous`，见 §6） | observe, query | `vault://redis-main/ro` |
| `redis-main` | `admin` | redis **admin ACL 账号** | mutate, manage, destroy | `vault://redis-main/admin` |
| `mq-main` | `ro` | rabbitmq 只读监控账号（queue/状态只读） | observe, query | `vault://mq-main/ro` |

设计要点：

- **同一资源多 tier，动词不重叠**：`svc-order` 的 `ro`（observe/query）与 `op`（mutate）承载不同动词集；策略引擎依本次动词选 tier，无任何 tier 承载该动词 → deny（`NO_TIER_MATCH_DENIED`，公理二）。
- **tier 声明 ⊆ 账号真实权限**是需被验证的前提（6.3）：接入时探测账号实际权限并比对，`postern verify` 含该校验项。例如 `db-main/ro` 若实际有写权限须报缺口。
- **docker 的 observe 必须走只读端点**：裸 `docker.sock` 即 root，`docker-A/logs` 指向只读日志 API 而非全 socket（5.2bis ④）。
- 不同 tier 的连接不共享（6.3、8.5）：一条只读连接绝不被升格执行写。

---

## 2 · 业务系统账号的认证声明（auth_flow）

`svc-*` 是 http 适配器资源，其 tier 账号是业务系统账号，须经业务系统**登录**才能拿到会话。这正是 6.13 的"会话来源（Authenticated Session Source）"：**有状态**——登录一次→复用→刷新，绝不每请求重登；2FA 只在接入期。

会话来源的非敏感配置写入 `resource_credential_tiers.auth_flow`（JSON 列，5.2、6.13 schema 落点）；敏感项（账号、refresh token）一律 `vault://` 引用。

### 2.1 `svc-order` 的 `op` tier auth_flow 示意（表单登录 + Cookie 注入）

```jsonc
// resource_credential_tiers(resource=svc-order, tier=op).auth_flow （JSON，仅非敏感配置）
{
  "flow": "form_login",                       // 流程类型：表单登录（另有 oauth_code / oauth_client / token_direct）
  "auth_endpoint": "/api/login",              // 认证端点（相对资源代号内部路径，真实地址仍由机密面解析）
  "method": "POST",
  "credential_fields": {                       // 账号字段名（值在 vault，永不入此处）
    "username_field": "username",
    "password_field": "password"
  },
  "credential_ref": "vault://svc-order/op",    // 账号密码引用（明文永不出机密面）
  "csrf": {                                     // CSRF 处理：登录响应里提取，后续每请求回填
    "extract_from": "response_header:X-CSRF-Token",
    "inject_as": "header:X-CSRF-Token"
  },
  "session_injection": {                        // 会话注入方式：Cookie
    "kind": "cookie",                           // cookie / bearer / custom_header
    "cookie_name": "JSESSIONID"
  },
  "twofa": {                                    // 2FA：仅接入期、人在场（控制面），数据面无此路径
    "required": true,
    "stage": "onboarding_only",                 // onboarding_only：只在接入期完成，绝不进数据面
    "method": "otp"
  },
  "refresh": {                                  // 刷新策略
    "kind": "session_renew",                    // 复用长效会话；临近过期主动续期
    "renew_endpoint": "/api/session/renew",
    "skew_seconds": 120,                        // expiry − skew 主动刷新（重叠窗口，旧会话仍有效）
    "on_hard_expire": "fail_closed"             // 硬过期（refresh 失效/需重新 2FA）→ 数据面 deny，提示经控制面重新接入
  }
}
```

### 2.2 token 维护与 2FA 边界（对齐 6.13）

- **登录一次→复用→刷新，绝不每请求重登**（6.13 运行期物化）：连接管理建连时经 `CredentialProvider.credential_for(svc-order, op)` 查 **live-session 缓存**（进程内、Zeroizing、键=`(resource, tier)`）：
  - 命中且未临近过期 → 直接复用缓存会话（★不重登）；
  - 缺失/临近过期（`expiry − skew`）→ 触发刷新（single-flight 单飞，同 `(resource,tier)` 至多一个在途刷新），刷新成功回填缓存；
  - 刷新失败/硬过期 → **fail-closed**：该请求 deny，reason="资源会话已过期，需经控制面重新接入授权（可能涉及二次验证）"，附 `operator_note`；**绝不在数据面静默重登**。
- **2FA 只在接入期**（6.13 两阶段）：`postern resource add svc-order --auth session ...` 由运维在**控制面**发起，执行 auth_flow 描述符；`twofa.required=true` 时此刻人在场完成 OTP，捕获**可刷新的长效会话**写入 vault（加密，Agent 永不可见）。换取的是数据面可无人值守续期的长效凭证——"安全不依赖实时审批"在认证侧的落点。
- **载体分层**（6.13 刷新策略）：长效 refresh/会话凭据存 vault（加密、持久）；短效访问令牌只在 live-session 缓存（内存、Zeroizing），不落盘；刷新令牌轮换则事务性写回 vault（热生效）。
- **诚实边界**（6.13）：若某业务系统每次登录都强制 2FA 且不提供任何可刷新长效会话，网关只能在单次会话有效期内运作，硬过期后 fail-closed 拒绝并提示重新接入——设计不假装能绕过，把约束如实暴露给运维。
- **审计**（观测面）：接入引导、每次刷新、刷新失败均记 `credential_event`，只记"哪个会话/账号/tier"，**绝不记令牌值**。

> 三种业务账号 tier（只读/操作/管理）即映射为不同业务系统账号，**引擎兜底落在业务系统自身权限体系上**（10.6 会话来源），与数据库 SELECT-only 账号、redis 只读 ACL 同构。

---

## 3 · 三个 Agent 的完整授权矩阵

授权 = `(信任等级 Role) × (辖区 Scope) × (动词 Capability) × (对象细则 constraint) × (条件 condition) × (引擎账号 tier)` 的网格（5.2bis）。角色用资源类型无关的标准阶梯（observer⊂operator⊂maintainer，5.2bis ①），辖区用标签 Scope 选择器（5.2bis ②），差异化靠 Scope+constraint+tier。

标准角色阶梯（动词集，经 `role_inherits` 显式继承）：

| 角色 | 动词集（含继承） |
|---|---|
| `observer` | observe, query |
| `operator` | observer + mutate, execute |
| `maintainer` | operator + manage |

`destroy` 不进任何标准角色，按 `(资源 × destroy)` 单格 + TTL 显式授予（本示例三个 Agent 均**未**授予任何 destroy，全部默认拒绝）。

### 3.1 agent1：A、B 上所有服务只读

- **role**：`observer`（observe+query）。
- **binding scope（标签选择器）**：`{all:[{key:'env',value:'prod'}]}` —— 选中全部 prod 资源（A、B 全部）。新增一台同类资源打对 `env:prod` 标签即自动纳入（5.2bis ②）。
- **tier 选择**：各资源的 observe/query 自动落到只读 tier（`ro` / `logs`）。

展开成 `(资源 × 动词)`：

| 资源 | observe | query | mutate | execute | manage | destroy |
|---|---|---|---|---|---|---|
| svc-order | ✅ ro | ✅ ro | ❌ deny | ❌ deny | ❌ deny | ❌ deny |
| svc-billing | ✅ ro | ✅ ro | ❌ | ❌ | ❌ | ❌ |
| svc-crm | ✅ ro | ✅ ro | ❌ | ❌ | ❌ | ❌ |
| docker-A | ✅ logs | —（docker 无 query 语义） | ❌ | ❌ | ❌ deny | ❌ |
| db-main | ✅ ro | ✅ ro | ❌ deny | ❌ | ❌ | ❌ |
| redis-main | ✅ ro | ✅ ro | ❌ deny | ❌ | ❌ deny | ❌ |
| mq-main | ✅ ro | ✅ ro | ❌ | ❌ | ❌ | ❌ |

✅=授予，❌=默认拒绝（公理一）。agent1 任何写/管/毁动词均无格（RBAC 步骤[3]无格→deny），且无对应 tier（双重拦截）。

### 3.2 agent2：A 的 docker 管理 + B 的数据库只读 + B 的 redis 管理

三个辖区不同信任等级，用三条 binding 表达：

| binding | role | scope（选择器/枚举） | 细则要点 | tier |
|---|---|---|---|---|
| b2-1 | `maintainer` | `{all:[{key:'host',value:'A'},{key:'kind',value:'docker'}]}`（=`docker-A`） | `container_prefix`=`app-`（只限业务容器名前缀） | manage→`admin`，observe→`logs` |
| b2-2 | `observer` | resource: `db-main` | `table_allow`=业务表白名单；`column_mask`=PII 列脱敏 | observe/query→`ro` |
| b2-3 | `maintainer` | resource: `redis-main` | `key_prefix`=`cache:`；`command_class`=`@read,@write,@keyspace`（容许管理类） | observe/query→`ro`，mutate/manage→`admin` |

展开成 `(资源 × 动词)`（仅列被 scope 选中的资源；未选中资源全部默认拒绝）：

| 资源 | observe | query | mutate | execute | manage | destroy |
|---|---|---|---|---|---|---|
| docker-A | ✅ logs | — | ❌ | ❌ | ✅ admin（限 `app-` 前缀容器） | ❌ deny（destroy 不在 maintainer） |
| db-main | ✅ ro | ✅ ro（限白名单表，PII 列脱敏） | ❌ deny | ❌ | ❌ deny | ❌ |
| redis-main | ✅ ro | ✅ ro | ✅ admin（限 `cache:` 前缀） | ❌ | ✅ admin | ❌ deny |

被默认拒绝的部分（显式标明）：

- agent2 对 `svc-*`（业务服务）、`mq-main` **无任何 binding** → 全部默认拒绝（scope 未选中）。
- `db-main:mutate` 拒绝（agent2 在 db-main 上是 observer，不含 mutate；且即便误归类也无 `rw` tier 选择路径）。
- 所有 destroy 拒绝（destroy 不进 maintainer，须单格+TTL 显式授予，未授予）。
- redis 的 manage 虽授予，但 `command_class` 细则把可执行命令类收窄（见 §6）。

### 3.3 agent3：业务服务管理（用系统跑业务流程）+ docker 查日志 + B 上所有服务日志

"业务服务管理（用系统跑业务流程）"= http 业务资源的 `mutate`（提交表单/推进流程），用 `operator` 角色；"docker 查日志"和"B 上所有服务日志"= `observe`，用 `observer` 角色。

| binding | role | scope | 细则要点 | tier |
|---|---|---|---|---|
| b3-1 | `operator` | `{all:[{key:'env',value:'prod'},{key:'kind',value:'business'}]}`（=`svc-order`/`svc-billing`/`svc-crm`） | `http_route` 收窄到业务流程接口（如 `POST /api/orders`、`POST /api/orders/*/submit`）；`mask_fields`=PII | observe/query→`ro`，mutate→`op` |
| b3-2 | `observer` | resource: `docker-A` | `container_prefix`=`app-`（只看业务容器日志） | observe→`logs` |
| b3-3 | `observer` | `{all:[{key:'host',value:'B'}]}`（=`db-main`/`redis-main`/`mq-main`） | 各资源 observe 细则（如 db `table_allow` 不适用于纯 observe；redis observe=`INFO`/`DBSIZE` 类） | observe→`ro`/`logs` |

> b3-1 用 `operator`（含 execute），但 http 业务资源不暴露 `execute` 动词（http 适配器 capabilities 为 observe/query/mutate/destroy，无 execute）——`operator` 的 execute 在此资源上**无对应格**，自然不放行（动词正交、角色资源无关，多出的动词在不支持它的资源上无害地落空，5.2bis ①）。

展开成 `(资源 × 动词)`：

| 资源 | observe | query | mutate | execute | manage | destroy |
|---|---|---|---|---|---|---|
| svc-order | ✅ ro | ✅ ro | ✅ op（限 `http_route` 业务流程接口） | —（http 无 execute 语义） | ❌ deny | ❌ deny |
| svc-billing | ✅ ro | ✅ ro | ✅ op（限路由） | — | ❌ | ❌ |
| svc-crm | ✅ ro | ✅ ro | ✅ op（限路由） | — | ❌ | ❌ |
| docker-A | ✅ logs（限 `app-` 容器日志） | — | ❌ | ❌ | ❌ deny | ❌ |
| db-main | ✅ ro | ❌（b3-3 仅 observer 的 observe 落在 db 上=状态/连接信息；query 数据若需另授） | ❌ | ❌ | ❌ | ❌ |
| redis-main | ✅ ro（`INFO`/`DBSIZE` 类） | ❌ | ❌ | ❌ | ❌ deny | ❌ |
| mq-main | ✅ ro（queue/状态） | ❌ | ❌ | ❌ | ❌ | ❌ |

> b3-3 用 `observer`（observe+query），但其意图是"日志"，落到各资源的 observe 格。query（读数据）若不希望授予，可用更窄的 `(资源 × observe)` 单格绑定替代 `observer` 角色（角色阶梯与单格授予可混用，5.2bis ①/3.4）。上表按"只授 observe"的更严格意图展开（query 标 ❌），体现"日志=observe"与"读数据=query"的动词正交。

被默认拒绝的部分（显式标明）：

- agent3 对 docker-A 的 `manage`/`destroy` 拒绝（b3-2 仅 observer）——agent3 能查容器日志但**不能重启容器**，与 agent2 精确区分。
- agent3 对 B 上资源只有 observe（日志/状态），无 query 数据、无 mutate、无 manage。
- 所有 destroy 拒绝。

---

## 4 · 声明式 TOML 片段（对齐 6.6）

以下是 `postern export` 产物风格的声明式表示（6.6、11.2）。瞬时状态（temp_grants 及 TTL）不导出；敏感项一律 `vault://` 引用，永不出现明文（11.2 不变量）。

```toml
# ====== 资源 ======
[[resources]]
codename  = "svc-order"
adapter   = "http"
transport = "ssh"
transport_config = { host_ref = "vault://svc-order/target" }   # 真实地址永不明文
labels = { host = "A", env = "prod", kind = "business" }
[[resources.credential_tiers]]
tier = "ro"; capabilities = ["observe","query"]; secret_ref = "vault://svc-order/ro"
[[resources.credential_tiers]]
tier = "op"; capabilities = ["mutate"];          secret_ref = "vault://svc-order/op"
# 会话来源配置：非敏感项入 auth_flow，账号/令牌仍 vault:// 引用（6.13 schema 落点）
auth_flow = { flow = "form_login", auth_endpoint = "/api/login", method = "POST", \
              session_injection = { kind = "cookie", cookie_name = "JSESSIONID" }, \
              csrf = { extract_from = "response_header:X-CSRF-Token", inject_as = "header:X-CSRF-Token" }, \
              twofa = { required = true, stage = "onboarding_only", method = "otp" }, \
              refresh = { kind = "session_renew", renew_endpoint = "/api/session/renew", skew_seconds = 120, on_hard_expire = "fail_closed" }, \
              credential_ref = "vault://svc-order/op" }
[[resources.constraints]]
capability = "mutate"; kind = "http_route"; spec = { routes = [ \
  { method = "POST", path = "/api/orders" }, { method = "POST", path = "/api/orders/*/submit" } ] }
[[resources.constraints]]
capability = "query"; kind = "mask_fields"; spec = { fields = ["customer.email","customer.phone"] }

# svc-billing / svc-crm 与 svc-order 同构（adapter=http, transport=ssh, 同标签 kind:business），此处从略

[[resources]]
codename  = "docker-A"
adapter   = "docker"
transport = "ssh"
transport_config = { host_ref = "vault://docker-A/target" }
labels = { host = "A", env = "prod", kind = "docker" }
[[resources.credential_tiers]]
tier = "logs";  capabilities = ["observe"];                secret_ref = "vault://docker-A/logs"   # 只读日志 API，非裸 socket
[[resources.credential_tiers]]
tier = "admin"; capabilities = ["manage","destroy"];       secret_ref = "vault://docker-A/admin"
[[resources.constraints]]
capability = "manage"; kind = "container_prefix"; spec = { prefix = "app-" }
[[resources.constraints]]
capability = "observe"; kind = "container_prefix"; spec = { prefix = "app-" }

[[resources]]
codename  = "db-main"
adapter   = "postgres"
transport = "ssm"
transport_config = { host_ref = "vault://db-main/target" }
labels = { host = "B", env = "prod", kind = "database" }
[[resources.credential_tiers]]
tier = "ro"; capabilities = ["observe","query"]; secret_ref = "vault://db-main/ro"   # SELECT-only 账号
[[resources.credential_tiers]]
tier = "rw"; capabilities = ["mutate"];          secret_ref = "vault://db-main/rw"
[[resources.constraints]]
capability = "query"; kind = "table_allow"; spec = { tables = ["public.orders","public.customers","public.invoices"] }
[[resources.constraints]]
capability = "query"; kind = "column_mask"; spec = { fields = ["public.customers.email","public.customers.phone"] }

[[resources]]
codename  = "redis-main"
adapter   = "redis"
transport = "ssm"
transport_config = { host_ref = "vault://redis-main/target" }
labels = { host = "B", env = "prod", kind = "cache" }
# 两道防线：归类（命令→动词）+ 只读 ACL tier（引擎兜底）。旧版无 ACL 见 §6 的 engine_enforced 标注
[[resources.credential_tiers]]
tier = "ro";    capabilities = ["observe","query"];                secret_ref = "vault://redis-main/ro"     # 只读 ACL 账号
[[resources.credential_tiers]]
tier = "admin"; capabilities = ["mutate","manage","destroy"];      secret_ref = "vault://redis-main/admin"
[[resources.constraints]]
capability = "query"; kind = "key_prefix";    spec = { prefix = "cache:" }
[[resources.constraints]]
capability = "mutate"; kind = "key_prefix";   spec = { prefix = "cache:" }
[[resources.constraints]]
capability = "manage"; kind = "command_class"; spec = { classes = ["@read","@write","@keyspace"] }   # 限可执行命令类

[[resources]]
codename  = "mq-main"
adapter   = "rabbitmq"
transport = "ssm"
transport_config = { host_ref = "vault://mq-main/target" }
labels = { host = "B", env = "prod", kind = "mq" }
[[resources.credential_tiers]]
tier = "ro"; capabilities = ["observe","query"]; secret_ref = "vault://mq-main/ro"

# ====== 角色阶梯（资源无关，经继承）======
[[roles]]
name = "observer";   capabilities = ["observe","query"]
[[roles]]
name = "operator";   inherits = ["observer"]; capabilities = ["mutate","execute"]
[[roles]]
name = "maintainer"; inherits = ["operator"]; capabilities = ["manage"]
# admin 不可声明：roles.name CHECK(lower(trim(name))<>'admin') + Capability 无 Admin 变体（双重硬约束）

# ====== Principals 与 bindings（scope 用标签选择器或枚举）======
[[principals]]
name = "agent1"; kind = "agent"
bindings = [
  { role = "observer", scope = { selector = { all = [{ key = "env", value = "prod" }] } } },
]

[[principals]]
name = "agent2"; kind = "agent"
bindings = [
  { role = "maintainer", scope = { selector = { all = [{ key = "host", value = "A" }, { key = "kind", value = "docker" }] } } },
  { role = "observer",   scope = { resources = ["db-main"] } },
  { role = "maintainer", scope = { resources = ["redis-main"] } },
]

[[principals]]
name = "agent3"; kind = "agent"
bindings = [
  { role = "operator", scope = { selector = { all = [{ key = "env", value = "prod" }, { key = "kind", value = "business" }] } } },
  { role = "observer", scope = { resources = ["docker-A"] } },
  { role = "observer", scope = { selector = { all = [{ key = "host", value = "B" }] } } },
]

# ====== 条件（可选，示例：业务写仅工作时段）======
[[resources.conditions]]
# 挂在 svc-order 上：mutate 仅工作日工作时段
capability = "mutate"; predicate = "time_window"; spec = { allow = "Mon-Fri 09:00-18:00" }

# ====== 模式与审批 ======
[modes]
default = "normal"
[approval]
enabled    = false      # escalate ≡ deny
on_timeout = "deny"     # 不可配置为 allow（导入校验拒绝）

# ====== 拒绝指引（人亲笔预写，公理六）======
[[deny_notes]]
resource = "svc-order"; capability = "mutate"
note = "下单/改单仅 agent3 在工作时段可做；只读分析用 agent1。确需扩权找 @owner"
```

> 选择器在**快照构建时**展开为当时匹配的具体资源集（5.2bis ②）。`env:prod` 选中全部 prod 资源、`host:A AND kind:docker` 精确选中 `docker-A`、`host:B` 选中 B 上三资源——"某环境/某机器某类/某机器全部"都成了一句标签选择。fail-closed：选择器语法不可解析或展开为空集 → 该绑定不授予任何资源（空集，不报错也不放行）。

---

## 5 · 三条 pipeline trace

每条 trace 走《详细设计文档》6.1 管线 `[0]→[10]`；任一步判定拒绝即短路。

### Trace ① · agent1 query 数据库（tier 选择 + SSM）

请求：agent1 经 MCP 外壳调 `postern_query(resource="db-main", request="SELECT id, name FROM public.orders WHERE status='paid'")`。

```
[0] 外壳归一化   MCP 工具 → NormalizedRequest{ presented=agent1 凭证, origin=UnixPeer{uid,gid},
                resource="db-main", intent=SQL 文本 }；自此与外壳无关（公理七）
[1] 认证+可信域  Authenticator 校验 agent1 凭证有效、未过期/未吊销，origin 在可信域内 → PrincipalId(agent1)
[2] 语义归一化   postgres Adapter.classify：纯只读 Query（无写节点、无 INTO）→ Capability=Query,
                objects=["public.orders"]
[3] RBAC        快照查表：agent1 —observer→ scope(env:prod 含 db-main) → (db-main, query) 格存在 ✅
[4] 细则        kernel 先跑 check_constraint：table_allow 含 public.orders ✅（注：本示例 agent1 binding
                未单独挂表白名单，db-main 资源级 query 细则适用）→ ConstraintCheck{passed=true}
[5] 条件        db-main 上无 query 条件谓词 → 通过
[6] 动作分流    Decision::Allow{ grant, tier }：策略引擎按动词 query 选承载它的 tier=ro（SELECT-only）
[7a] 意图审计    query 是只读动词，无副作用 → 沿用执行后单次审计（不走两阶段 intent）
[7b] 取连接     connpool.acquire(db-main, ro)：池键=(db-main, ro)；机密面解析 (target, credential[ro])
                → ssm Transport.open 建到 B 的会话通路 → Channel；凭据引用即时释放
[8] 执行        postgres Adapter.execute over Channel：以 SELECT-only 账号执行 → RawResponse
[9] 脱敏        Sanitizer.scrub：系统级 ScrubSet 擦除任何真实地址/凭据回显；列 column_mask 擦 email/phone
[10] 结果审计    AuditSink.record：kind=request, decision=allow, tier="ro", capability=query,
                objects=["public.orders"], response_digest=sha256(...)（不含内容）
```

要点：**tier 选择在策略引擎**（步骤[6] 产出 `Allow{tier=ro}`），**连接管理只按已选 tier 建连**（步骤[7b]），二者职责分离（速查表）。即便 SQL 被误归类，`ro` 走 SELECT-only 账号，引擎层拒绝任何写——`engine_enforced=true` 的兜底。

### Trace ② · agent3 用业务系统下单（http + 业务账号 + 会话复用/刷新 + 脱敏）

请求：agent3 经 HTTP 外壳调 `postern_mutate(resource="svc-order", request={method:"POST", path:"/api/orders", body:{...}})`。

```
[0] 外壳归一化   → NormalizedRequest{ presented=agent3 凭证, resource="svc-order", intent=HTTP 请求负载 }
[1] 认证+可信域  Authenticator 校验 agent3 凭证 → PrincipalId(agent3)
[2] 语义归一化   http Adapter.classify：POST /api/orders → Capability=Mutate, objects=[route:/api/orders]
[3] RBAC        agent3 —operator→ scope(env:prod AND kind:business 含 svc-order) → (svc-order, mutate) ✅
[4] 细则        check_constraint：http_route 白名单含 {POST /api/orders} ✅ → ConstraintCheck{passed=true}
[5] 条件        svc-order:mutate 挂 time_window=Mon-Fri 09:00-18:00；当前在窗口内 → 通过
                （若不在窗口 → 任一条件 false → deny，公理二）
[6] 动作分流    Decision::Allow{ tier=op }：策略引擎按动词 mutate 选业务操作账号 tier=op
[7a] 意图审计    mutate 有副作用 → 两阶段审计：先落 intent 事件（执行前）；写不进 → 执行前 deny（确未执行）
[7b] 取连接     connpool.acquire(svc-order, op)：
                → CredentialProvider.credential_for(svc-order, op) 查 live-session 缓存（键=(svc-order,op)）：
                   ├─ 命中且未临近过期 → 直接复用缓存会话（★不重登）
                   └─ 临近过期(expiry−skew=120s) → single-flight 触发刷新 /api/session/renew，
                      旧会话在新会话就绪前仍有效（重叠窗口），刷新成功回填缓存；
                      若硬过期/需重新 2FA → fail-closed：deny + operator_note，绝不数据面静默重登
                → 活跃会话以"请求注入描述"交 http 适配器：Set-Cookie:JSESSIONID + 回填 CSRF 头
[8] 执行        http Adapter.execute：贴 Cookie/CSRF，转发 POST /api/orders 到业务系统 → RawResponse
[9] 脱敏        Sanitizer.scrub：系统级擦真实地址/凭据；mask_fields 擦响应里的 customer.email/phone
[10] 结果审计    落 outcome 事件（与 intent 同请求 id 关联）：decision=allow, tier="op", capability=mutate
                注：已执行的请求绝不返回 deny；outcome 写失败 → 返回"已执行但审计降级"错误码，非 deny
```

要点：**登录一次→复用→刷新，不每请求重登**（步骤[7b]）；**2FA 不在此路径**（只在接入期）；mutate 的**两阶段审计**保证"发起即有痕、结果可追溯"（6.1 时序不变量）；引擎兜底落在**业务系统自身权限体系**——`op` 账号在订单系统里本就只能下单/改单（6.13、10.6 会话来源）。

### Trace ③ · 越权 deny（结构化拒绝 + your_grants + request_hint）

请求：agent1（只读）尝试重启容器——`postern_manage(resource="docker-A", request={action:"restart", container:"app-order"})`。

```
[0] 外壳归一化   → NormalizedRequest{ presented=agent1 凭证, resource="docker-A", intent=manage 负载 }
[1] 认证+可信域  agent1 凭证有效 → PrincipalId(agent1)
[2] 语义归一化   docker Adapter.classify：restart → Capability=Manage, objects=[container:app-order]
[3] RBAC        快照查表：agent1 —observer→（observe+query），(docker-A, manage) 格【不存在】
                → deny（公理一，stage=rbac）；短路，不进 [4]~[10] 执行
[7a/8] —        不执行（manage 是有副作用动词，但在 deny 短路下确未执行，无 intent 事件副作用）
[9] 脱敏        DenyResponse 经同一 Sanitizer 出口（拒绝响应也脱敏，红线 7.2-3）
[10] 审计       kind=request, decision=deny, stage=rbac, reason="role=observer 不含 docker-A:manage"
```

返回 Agent 的结构化拒绝响应（4.1 `DenyResponse`，公理六：只含事实或人预写内容，`DENY_RESPONSE_SCOPE_BOUNDED` 只含 agent1 自身授权世界）：

```jsonc
{
  "decision": "deny",
  "denied": { "resource": "docker-A", "capability": "manage", "objects": ["container:app-order"] },
  "reason": "role=observer 不含 docker-A:manage",            // 引用策略事实，非话术
  "your_grants": {                                            // agent1 自身授权世界的投影（从快照导出）
    "docker-A": ["observe"],
    "db-main": ["observe","query"],
    "redis-main": ["observe","query"],
    "svc-order": ["observe","query"],
    "svc-billing": ["observe","query"],
    "svc-crm": ["observe","query"],
    "mq-main": ["observe","query"]
  },
  "request_hint": "postern elevate agent1 --cap docker-A:manage --ttl 30m",  // 策略机械生成的命令
  "operator_note": null                                       // docker-A:manage 无 deny_note，则不出现（人未预写）
}
```

要点：拒绝是**结构化事实**，Agent 据 `your_grants` 自行分流（可知自己能 observe docker-A 但不能 manage）；`request_hint` 是策略**机械生成**的 `postern elevate` 命令（不是网关编造的建议，公理六）；`your_grants` 只含 agent1 自己的授权，**不泄露其他 Agent 或资源是否存在**（Scope 外资源代号访问的拒绝同样不泄露存在性，6.7 verify 项 4）。

---

## 6 · redis 只读怎么落地（归类 + 只读 ACL tier 两道防线 + 诚实标注）

redis 只读用**两道独立防线**（5.2bis ④、§3.1/§3.3 适配器），不靠单点：

### 防线一 · 归类（命令 → 动词）

redis 适配器把 redis 命令归类为 Capability（命令归类，与 SQL 语法树归类同理念）：

- `GET`/`MGET`/`HGET`/`LRANGE`/`SCAN`/`EXISTS`/`TTL`/`INFO`/`DBSIZE` → `observe`/`query`（只读）。
- `SET`/`DEL`/`HSET`/`EXPIRE`/`LPUSH` → `mutate`。
- `FLUSHDB`/`FLUSHALL` → `destroy`；`CONFIG SET`/`CLIENT KILL`/`SLAVEOF` 等 → `manage`。
- 无法可靠归类的命令 → `Err` → deny（公理二，白名单归类宁可误拒）。

agent1/agent3 对 `redis-main` 只有 observe/query 格 → 任何写命令在 **RBAC 步骤[3] 即无格 deny**；细则 `key_prefix=cache:` 进一步把 query 收窄到 `cache:` 前缀键（步骤[4]）。

### 防线二 · 只读 ACL tier（引擎兜底）

`redis-main` 的 `ro` tier 指向一个 **redis ACL 只读账号**（redis 6+ ACL，如 `user ro_agent on >... ~cache:* +@read -@write -@dangerous`）：

- 即便归类被绕过（误把写命令归为 query），`ro` tier 走的 ACL 账号在 **redis 引擎层**也只有 `+@read`，写命令被引擎直接拒绝——这是 `engine_enforced=true` 的兜底（与 db SELECT-only 同构）。
- tier 声明 ⊆ 账号真实 ACL 权限是需被验证的前提（6.3）：接入时探测 ACL（`ACL GETUSER`）与 tier 声明比对，不符则拒绝接入或降级 tier；`postern verify` 含该项。

### 诚实标注 · 旧版 redis 无 ACL → engine_enforced=false

redis 6 之前无 ACL，无法在引擎层做账号级只读约束：

- 此时 `redis-main` 的 `Adapter::engine_enforced()` **如实返回 `false`**，能力声明与文档显式标注**"归类 + 细则是唯一防线"**（5.2bis ④、§3.3、公理三：系统强制不靠口头）。
- 防线二失效，只剩防线一（归类 + `key_prefix`/`command_class` 细则）；设计不假装旧版 redis 有引擎兜底——把真实强度如实暴露给运维，由其决定是否接受该资源在该防护强度下接入，或升级 redis 到 6+ 以获得 ACL 兜底。

资源建模表（§1.1）对 `redis-main` 的 engine_enforced 列已写明"redis 6+ ACL=true；旧版=false（如实声明）"，与本节一致。

---

## 7 · 小结：本示例覆盖的设计要点

| 设计点 | 本示例落点 |
|---|---|
| 同一物理服务用/管拆多资源 | A 业务服务：用=`svc-*`(http+业务账号)，管=`docker-A`(docker)（§1、§3） |
| 异构资源建模 + tier | http/postgres/docker/redis/rabbitmq 各声明 tier 与 engine_enforced（§1） |
| 业务系统账号认证（会话来源） | `svc-order` auth_flow：表单登录+Cookie+CSRF+刷新+2FA 接入期（§2） |
| token 维护不重登、刷新、2FA 边界、硬过期 fail-closed | §2.2、Trace ②[7b] |
| 角色阶梯资源无关 | observer/operator/maintainer 经继承，复用于全部资源类型（§3、§4） |
| 标签 Scope 分组 | `env:prod` / `host:A AND kind:docker` / `host:B` 选择器（§3、§4） |
| 完整授权矩阵 + 默认拒绝 | 三 Agent 的 (资源×动词) 展开表，❌ 标默认拒绝（§3） |
| 声明式 TOML + vault:// | §4，敏感项全 `vault://`，无明文 |
| 结构化拒绝 + your_grants + request_hint | Trace ③（§5） |
| redis 只读两道防线 + 旧版诚实标注 | §6 |
