# postern-transports 模块详细设计

> 本篇是 `postern-transports` crate 的模块级详细设计，在《详细设计文档》第八部分 8.7「传输（单通路域）」的领域裁决之上展开。结构严格遵循 [00-模块详细设计-索引与规约](00-模块详细设计-索引与规约.md) 规定的七小节。**纯设计，不含任何实现代码、阶段划分或进度状态。** 与本篇冲突时，以《技术设计文档》七公理与《详细设计文档》第八部分领域裁决为准。

---

## 1. 定位（一句话）

`postern-transports` 是把「一种远端接入方式」抽象为「本地可用通路」的**单通路域**——它只负责一条通路从建立到关闭的协议级机制，对上层（连接管理）始终呈现一致的「本地 socket」抽象，不感知池化、不持有凭据、不做任何通路间生命周期决策。

---

## 2. 承载领域与职责范围

对应《详细设计文档》第八部分 **8.7 传输**（《技术设计文档》第八部分 8.1 传输层、第十部分 10.1 Transport 插件）。本 crate 是 8.7 的唯一载体。

职责范围（封闭列举，全部限定在「单条通路」粒度内）：

1. **通路建立**：实现 `Transport::open`——消费连接管理层一次性注入的 `ResolvedTarget`（真实地址）与 `ResourceCredential`（资源凭据），据此建到远端、在本地暴露一个可用通路（`Channel`）。
2. **通路内保活**：维持**已建**通路的活性——心跳、对有时限通路的协议级续约（如 SSM 会话续期、SSH keepalive、租约续租）。保活只作用于本条已建通路，是「单条通路的协议级机制」。
3. **通路状态如实上报**：经 `Channel` 的健康语义如实报告当前通路状态（活/僵死/已关闭），供连接管理层据以决策。
4. **按指令关闭与释放**：按连接管理层经 `Channel` 关闭接口下达的指令释放本条通路与其底层隧道；关闭的**决策**在连接管理层，**执行**在本域（决策者-执行者分离，见《详细设计文档》8.0 总则第 4 条）。
5. **`persistent` 声明**：经 `Transport::persistent()` 声明本传输是长连接型（持续通路，建立后保活、可复用）还是非长连接型（按需通路，用毕即释放），供连接管理层据以决定是否池化。
6. **具体 Transport 实现（feature 门控）**：以模块 + cargo feature 形式在本 crate 内提供 `ssh` / `ssm` / `direct` 等通路形态实现（`postern-transports/src/{ssh,ssm,direct}/`），不再细分 crate（《详细设计文档》3.2）。各实现仅在 `kind()` 与 `persistent()` 的取值、以及 `open` 内部的协议机制上不同，对上层呈现的 `Channel` 抽象一致。

「单条通路」是本域唯一粒度：本域不知道、也不需要知道同一资源是否有其他通路并存，不知道本通路是新建还是被复用，不知道远端是否可达多个 tier。

---

## 3. 支持的功能

本 crate 对外提供的能力按 `core` 定义的 `Transport` trait 与 `Channel` 抽象组织：

- **建立到远端的通路并暴露本地 socket**（`open`）：把代号背后的真实接入（SSH 隧道、SSM 端口转发、直连等）收敛为一个对上层一致的本地可用通路。资源凭据与真实地址仅在本次 `open` 调用生命周期内使用。
- **长连接型保活**：对 `persistent() == true` 的通路，在通路存活期间维持心跳与协议级续约，使连接管理层得以安全复用同一条通路而无需频繁重建。
- **非长连接型即建即弃语义**：对 `persistent() == false` 的通路，不承担跨请求保活，通路随上层「用毕」即被关闭释放。长/非长差异由 `persistent()` 声明承载，不外溢到 `Channel` 抽象。
- **健康事实上报**：通过 `Channel` 暴露当前通路健康事实（不含任何真实地址/凭据信息），供连接管理层做健康剔除与重建决策。
- **协议级关闭**：按 `Channel` 的关闭语义执行优雅释放或被强制 abort/cancel（紧急切断时由连接管理层发起，本域执行底层隧道的取消与关闭）。
- **传输形态可扩展**：新增一种远端接入方式 = 在本 crate 内新增一个 feature 门控的 `Transport` 实现，不触动连接管理、适配器与求值逻辑（《技术设计文档》10.1、8.3 上层无感知原则）。

---

## 4. 明确边界（不做什么）

本域显式排除以下职责，每项指明归属域：

- **绝不自行重建通路**：通路死亡时只**如实上报**，是否重建、何时重建、退避节奏一概不在本域。→ 归 **连接管理（`postern-daemon::connpool`，8.5）**：重建决策与指数退避是连接管理的范围内职责（《详细设计文档》8.5 范围内、《技术设计文档》8.2「重连只发生在连接管理层」）。
- **不池化、不复用决策**：本域不维护通路池、不决定一条通路被几个请求共享、不决定新建还是复用。→ 归 **连接管理（8.5）**。
- **不做并发上限、背压、空闲回收、优雅/紧急销毁的发起**。→ 归 **连接管理（8.5）**（本域只执行连接管理发起的关闭）。
- **不保管、不自取凭据与真实地址**：不持久持有、不缓存 `ResolvedTarget`/`ResourceCredential`，不从机密面主动取用机密。→ 机密的唯一权威持有者与解析者是 **机密面（`postern-secrets`，8.8）**；机密的**注入**由 **连接管理（8.5）** 在建立流程中一次性完成（凭据取用方向：连接管理向机密面取、传入本域，本域不反向自取）。
- **不解释协议语义、不归类动词、不执行业务操作**：本域只提供「一个可用通路」，不知道通路上跑的是 SQL、容器日志还是 HTTP。→ 归 **适配器（`postern-adapters`，8.6）**（适配器经 `Channel` 执行）。
- **不触策略与审计**：不读策略快照、不写审计事件、不做任何授权判定。→ 策略归 **策略引擎（`postern-core::eval`，8.3）**；审计载体归 **存储层（`postern-store`，8.11）**、审计语义归 **观测面（8.9）**、审计调用归 **数据面内核（8.2）/连接管理（8.5）**。本域到 `store` 的依赖被契约 `ARCH_FORBIDDEN_EDGES` 禁止。
- **不做脱敏**：不持有 ScrubSet、不擦除任何字节。→ ScrubSet 归 **机密面（8.8）**，脱敏执行调用归 **数据面内核（8.2）**。
- **不采集连接来源、不构造 `ConnOrigin`**：本域面向资源侧（出站通路），不接受 Agent 入站连接。→ `ConnOrigin` 只在 **外壳层 listener（8.12）** 构造（契约 `SEC_CONSTRUCTION_SITES`）。

---

## 5. 对外接口

本 crate **实现** `core` 中**定义**的 `Transport` trait，并在 `core` 定义的 `Channel` 抽象上承载健康与关闭语义。下列签名是设计承诺（与《详细设计文档》4.1/8.7 一致），实现可调内部细节但不得违背签名与本篇不变量。

### 5.1 `Transport` trait（定义在 `core::plugin`，实现在本 crate）

```rust
/// 传输(步骤[7b]取连接的底层)。实现:ssh / ssm / direct(feature 门控)
#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> &'static str;
    fn persistent(&self) -> bool;        // 长连接型→可入池;非长连接型→用毕即销
    async fn open(&self, target: ResolvedTarget, cred: ResourceCredential)
        -> Result<Channel, TransportError>;
    // ResolvedTarget / ResourceCredential 由 daemon 连接管理层从机密面取出注入,
    // 二者不实现 Clone/Serialize,Debug 输出恒为 REDACTED,生命周期不出本调用
}
```

- `kind()`、`persistent()`：本 crate 实现（取值由各传输形态固定，`persistent()` 是连接管理判定池化与否的依据）。
- `open(...)`：本 crate 实现。`target`/`cred` 以**值**传入（move 语义），随 `open` 调用结束即释放——这是「凭据生命周期不出 `open` 调用」在签名层的表达。`ResolvedTarget`/`ResourceCredential` 由机密面**唯一构造**（契约 `SEC_CONSTRUCTION_SITES`），本 crate 只**消费**，无法 import 其构造路径。
- 返回 `Result<Channel, TransportError>`：`open` 失败必返回 `Err`，**绝不**返回一个伪健康的 `Channel`。`TransportError` 在跨 crate 边界抛出前必须已脱敏为不含真实地址的错误码（见第 7 节红线 1）。

### 5.2 `Channel`（在 `core` 声明的通路抽象，本 crate 承载其健康与关闭语义）

`Channel` 是上层（适配器经连接管理获得）持有的「本地可用通路」句柄。本域在其上承载的语义承诺：

- **一致的本地 socket 抽象**：无论底层是 SSH / SSM / direct、长连接或非长连接，`Channel` 对上层呈现一致的可用通路语义；长/非长差异不外溢到 `Channel` 的使用方式。
- **健康语义**：`Channel` 可被查询当前通路健康事实（活/僵死/已断开），事实内容不含真实地址与凭据。
- **关闭语义**：`Channel` 提供按连接管理指令执行的关闭/释放路径（含被强制 abort/cancel 的紧急切断），关闭的发起权在连接管理，本域只执行。

> 注：`Channel` 类型本身定义于 `core`（被 `Adapter::execute(ch: &mut Channel, ...)` 与本 trait 共享）；本 crate 提供其在各传输形态下的具体通路语义，不重新定义类型。

### 5.3 错误类型

- `TransportError`：本 crate 的 thiserror 错误枚举（每 crate 一个，《详细设计文档》7.1）。所有变体在跨 crate 边界呈现前均已脱敏（不含真实地址/凭据明文）。连接管理层据此错误判定通路不可建立并 fail-closed（向上 deny），或据健康上报判定通路死亡并自行决策重建。

### 5.4 本 crate **不**对外暴露的内容

- 不暴露任何 `ResolvedTarget`/`ResourceCredential` 的构造或读取接口（无该路径，编译期不可达）。
- 不暴露池、健康检查器、退避器、回收器等通路间治理设施（不属本域）。
- 不暴露任何 SQL/策略/审计/脱敏接口。

---

## 6. 与相邻模块的交互

依据[权威依赖图](00-模块详细设计-索引与规约.md#权威依赖图唯一事实来源)，`postern-transports` 的**允许依赖边**仅有一条：`postern-transports → postern-core`（消费领域类型与 `Transport`/`Channel` 定义）。本 crate 被 `postern-daemon`（唯一组装点）依赖与调用。下述每条交互均与依赖图和交互矩阵一致，**不描述任何被禁止的依赖边**。

### 6.1 ← `postern-daemon::connpool`（连接管理层调用本 crate）—— 建立通路

- **方向**：`daemon::connpool` → `transports`（连接管理层是调用方，本域是被调方）。本域**绝不**反向调用 daemon，也不主动向任何方拉取数据。
- **内容（传什么类型）**：连接管理层调用 `Transport::open(target: ResolvedTarget, cred: ResourceCredential) -> Result<Channel, TransportError>`。`target`/`cred` 由连接管理层先行从**机密面**（`secrets::CredentialProvider::credential_for(res, tier)` 与 `resolve(code) -> ResolvedTarget`）取出的**不透明句柄**，以 move 语义一次性传入；本域返回 `Channel`（成功）或 `TransportError`（失败，已脱敏）。
- **时机（求值管线/启动序列的哪一步）**：求值管线**步骤 [7b] 取连接**。连接管理层按 `Decision::Allow{tier}` 选定的 `(ResourceCode, CredentialTier)` 在池中未命中、或为非长连接型需新建时，进入建立流程：①向机密面取 `(ResolvedTarget, ResourceCredential[tier])`；②调用本域 `open` 注入二者；③`open` 返回后，凭据引用即时释放（《技术设计文档》6.3 建立流程「凭据引用即时释放」）。本域只参与步骤②的执行。
- **失败语义（fail-closed）**：`open` 失败必返回 `Err(TransportError)`（脱敏后），**绝不**返回伪健康通路。连接管理层据此判定「通路不可建立」并 **fail-closed → deny**（公理二「连接不可建立」一律拒绝、《详细设计文档》6.1 步骤 [7b]「不可建→deny」、8.5「无法建立→deny」）。本域不重试、不退避——重试/退避是连接管理的决策。

### 6.2 → `postern-daemon::connpool`（本 crate 经 `Channel` 向连接管理层上报）—— 健康上报与按指令关闭

- **方向**：信息流 `transports` → `daemon::connpool`，但**不是本域主动调用 daemon**——本域经 `core` 定义的 `Channel` 健康语义**被动呈现**事实，由连接管理层读取（健康检查、剔除决策、回收发起均由连接管理层主动驱动）。本域无 daemon 依赖，不能调用 daemon。
- **内容**：经 `Channel` 暴露的健康事实（活/僵死/已断开，不含真实地址/凭据）；关闭方向上，连接管理层经 `Channel` 关闭语义下达指令，本域执行底层隧道的释放或强制 abort/cancel。
- **时机**：
  - **健康上报**：通路存活期间持续——连接管理层在定期探活、空闲回收判断、健康剔除决策时读取本域呈现的健康事实。
  - **按指令关闭**：在连接管理层发起空闲回收 / 健康剔除 / 资源下线 / daemon 停止（优雅销毁，排空在途后释放）或 freeze/吊销触发的**紧急切断**（强制 abort/cancel，不走优雅排空，见《详细设计文档》6.2/8.5）时，本域据指令执行。
- **失败语义（fail-closed）**：
  - **通路死亡如实上报**：通路断开/僵死时，健康事实如实呈现「死亡」，**绝无静默重建**（本篇核心不变量）。连接管理层据此 fail-closed——重建期间到来的请求等待新通路或被拒（《技术设计文档》8.2）。
  - **关闭/续约失败**：保活续约失败 → 通路转入「死亡」健康态如实上报（不掩盖、不自愈重连）；关闭执行中的底层错误经脱敏后以 `TransportError` 呈现，绝不让原始错误串（如 `connection refused to 10.0.3.17`）外泄（第 7 节红线 1）。

> 说明：连接管理层对应的审计（`connection_event`：通路建立/健康剔除/回收）由 **连接管理层**写入（经 `AuditSink`），**不在本域**——本域只提供供其决策的健康事实。

### 6.3 → `postern-secrets`（**禁止的依赖边——本 crate 绝不依赖机密面**）

- **方向**：**无**。`transports` **不依赖** `postern-secrets`（依赖图中无此边）。`ResolvedTarget`/`ResourceCredential` 的类型**定义**对本域可见（经 `core` 的不透明声明），但其**构造路径**只在 `postern-secrets`（契约 `SEC_CONSTRUCTION_SITES`），本域无法 import、无法构造、无法自取。
- **交互实质**：机密**不由本域向机密面索取**，而是由**连接管理层**从机密面取出后注入本域 `open`（见 6.1）。这是「凭据取用方向」的权威表达（《技术设计文档》8.1/10.6、《详细设计文档》4.1/6.3）：本域是机密的**末端消费者**，消费窗口严格限于单次 `open` 调用，消费后不留存。
- **失败语义**：若机密面解析失败，错误发生在**连接管理层取机密**这一步（6.1 步骤①），本域根本不会被调用 `open`；连接管理层 fail-closed → deny。本域不感知机密面的存在或其失败。

### 6.4 → `postern-store`（**禁止的依赖边——契约硬约束**）

- **方向**：**无**，且被契约 `ARCH_FORBIDDEN_EDGES` 显式禁止（`transports ↛ store`）。本域不触策略状态与审计载体，不含任何 SQL 字符串与 rusqlite 依赖（裸 SQL 仅允许在 `postern-store`，契约 `DB_NO_RAW_SQL_OUTSIDE_STORE`）。
- **含义**：传输层不感知策略、不参与审计写入；通路相关审计由连接管理层在 daemon 内承担。

### 6.5 → `postern-adapters`（无依赖边，经 `Channel` 间接协作）

- **方向**：**无直接依赖**（两者均为 `core` 的下游 lib crate，互不依赖；`adapters ↛ transports` 亦被契约禁止）。二者经 `core` 定义的 `Channel` 类型**间接协作**：本域产出 `Channel`，适配器经连接管理获得 `Channel` 后调用 `Adapter::execute(ch: &mut Channel, ...)` 执行。
- **时机**：求值管线**步骤 [8] 执行**——适配器在连接管理交付的 `Channel` 上执行被放行的意图。本域在此阶段不被调用（通路已于步骤 [7b] 建立），仅在通路存活期间持续保活、按需呈现健康事实。
- **失败语义**：适配器执行中若发现通路不可用，其错误沿适配器路径上报；通路本身的死亡仍由本域经 `Channel` 健康语义如实上报给连接管理层。本域不解释适配器协议层的错误。

### 6.6 关于求值管线其余步骤

本域**只**参与求值管线的步骤 [7b]（建立，被连接管理调用）与步骤 [8] 期间的通路保活/健康呈现（被动）。步骤 [0]~[7a]、[9]、[10] 与本域无任何交互——认证、归类、RBAC、细则、条件、动作分流、意图审计、脱敏、结果审计全部在本域之外（见第 4 节边界）。这与公理七「外壳差异不导致安全语义差异」一致：本域处于安全语义内核的下游执行端，对决策无影响。

---

## 7. 必守不变量

下列不变量按属主裁决与契约/签名强制方式逐条标注。

1. **凭据与真实地址生命周期不出 `open` 调用**（公理四·凭据零接触）：`ResolvedTarget`/`ResourceCredential` 以 move 语义传入 `open`，随调用结束即释放；本域不持久持有、不缓存、不向上传递、不落日志。
   - 强制：`Transport::open` 签名（值传入）+ 机密类型不可 Clone/Serialize（无法复制留存）+ 契约 `SEC_SECRET_TYPE_DISCIPLINE`（`ResolvedTarget`/`ResourceCredential` 不得 derive/手写 `Clone`/`Serialize`）。

2. **机密类型 `Debug=REDACTED`、无 `Display`、不入日志**（红线 7.2-1、机密类型纪律 7.1）：本域无法把机密通过 tracing 字段直接记录（类型层即不可表达）；任何 `TransportError` 在跨 crate 边界抛出前必须脱敏为不含真实地址的错误码，绝不让 `connection refused to <真实地址>` 一类原始错误串外泄。
   - 强制：机密类型 `Debug` 恒输出 `REDACTED`、不实现 `Display`（机密面定义）+ 红线 7.2-1（跨边界错误先脱敏）。

3. **绝不自行重建——断开如实上报、无静默重建**（本篇与 8.7 核心承诺）：通路死亡时本域只经 `Channel` 健康语义如实呈现「死亡」，是否/何时/如何重建一概由连接管理层决策。本域内部不含任何「重连」逻辑。
   - 强制：`Transport` trait 无重建接口（签名约束）+ 领域裁决（8.7 范围外「重建决策与退避节奏→连接管理；通路死亡只上报，绝不自行重建」、速查表「重建决策/退避/回收→连接管理，传输按指令执行关闭」）。

4. **不池化、不做通路间生命周期决策**：本域只管单条通路。池化、并发上限、回收、退避、优雅/紧急销毁的**发起**均不在本域。
   - 强制：领域裁决（8.5 连接管理范围内、8.7 范围外）+ 本 crate 不暴露任何池/治理设施。

5. **不依赖 `store`/`secrets`，不自取机密**：本域唯一允许的工作区依赖是 `core`；不依赖 `secrets`（机密由连接管理注入）、不依赖 `store`（不触策略/审计）。
   - 强制：契约 `ARCH_FORBIDDEN_EDGES`（`transports ↛ store`）+ 依赖图（`secrets` 不在 `transports` 的允许依赖边内）+ 契约 `SEC_CONSTRUCTION_SITES`（机密只在 `secrets` 构造，本域不可构造/自取）+ `DB_NO_RAW_SQL_OUTSIDE_STORE`（本域无 SQL/rusqlite）。

6. **`open` 失败必 `Err`，绝不返回伪健康通路**（公理二·fail-closed）：通路不可建立即返回 `Err(TransportError)`，由连接管理层解析为 deny。
   - 强制：`Transport::open -> Result<Channel, TransportError>` 签名 + 公理二（连接不可建立→拒绝）。

7. **`Channel` 抽象一致、长/非长差异不外溢**：无论底层形态、是否长连接，`Channel` 对上层呈现一致的「本地可用通路」语义；长/非长差异仅由 `persistent()` 声明承载，供连接管理判定池化，不改变 `Channel` 的使用方式。
   - 强制：`Channel` 抽象（`core` 定义）+ `persistent()` 单一声明点 + 领域裁决（8.7 必守不变量「长/非长连接差异不外溢」、《技术设计文档》8.3 上层无感知原则）。

8. **健康事实如实、不含机密**：经 `Channel` 上报的健康事实只描述通路状态，不含真实地址、凭据或拓扑标识。
   - 强制：机密类型纪律（事实中不可出现机密）+ 领域裁决（8.7「通路状态的如实上报」、公理六·只说事实）。

9. **`unsafe` 全 crate forbid**：本 crate 与全工作区一致 `unsafe_code = "forbid"`；`SO_PEERCRED` 不在本域（来源采集归外壳层），本域无已知 unsafe 需求。
   - 强制：`[workspace.lints]` `unsafe_code = "forbid"`（《详细设计文档》7.1）。
