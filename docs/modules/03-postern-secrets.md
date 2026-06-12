# postern-secrets · 模块详细设计

> 本篇是 `postern-secrets` 的模块级详细设计。它在《详细设计文档》第八部分 8.8「机密面」的领域裁决之上展开本 crate 的定位、职责、功能、边界、与相邻模块的交互细节与必守不变量。结构遵循 [00-模块详细设计-索引与规约](00-模块详细设计-索引与规约.md) 规定的七小节。纯设计，不含任何实现代码、阶段划分或进度状态。与本篇冲突时，以《技术设计文档》的七公理与《详细设计文档》第八部分的领域裁决为准。

---

## 1 · 定位（一句话）

`postern-secrets` 是机密面（8.8）的载体——系统中**两类机密（资源凭据、代号↔真实地址映射）的唯一权威持有者**，以及一切机密派生视图（解析出的 `ResolvedTarget`/`ResourceCredential`、系统级擦除集 ScrubSet）的**唯一签发者**。明文机密只存在于本 crate 解锁后的进程内存与一次性注入 `Transport::open` 的调用生命周期内，对数据面无任何取用路径。

---

## 2 · 承载领域与职责范围

**对应第八部分领域**：8.8 机密面（全部）。

本 crate 是 workspace 内的库 crate（无二进制），由唯一组装点 `postern-daemon` 依赖与装配。它封闭承载以下职责（封闭列举，超出此列表的职责不属本域）：

1. **保险箱载体**——`vault.postern` 加密文件的格式定义、加解密、解锁后明文在 `Zeroizing` 容器中的内存持有、原子写入与轮换协议。
2. **解锁来源族（MasterKeySource）**——把不同部署形态的解锁材料抽象为统一接口，并对各来源的**真实强度差异**作诚实界定（KDF 仅作用于 passphrase 来源）。
3. **代号↔真实地址映射**——保险箱 `targets` 段的持有，以及 `resolve(code) -> ResolvedTarget` 解析操作。
4. **资源凭据来源（CredentialProvider 实现）**——静态保险箱实现，按 `(ResourceCode, CredentialTier)` 解析出 `ResourceCredential`；动态签发/证书为同接口预留。
5. **机密类型的唯一构造权**——`ResolvedTarget`（真实地址）与 `ResourceCredential`（资源凭据）这两个类型的构造路径**只在本 crate 存在**，是公理四（凭据零接触）的编译期表达。
6. **系统级擦除集 ScrubSet 的构造、更新与持有**——由保险箱 `targets`/`secrets` 派生的单向匹配视图，随保险箱写入而更新；并以**不透明 match-and-erase 句柄**形式签发给数据面内核。
7. **面向控制面的机密录入/更新写接口**——经 `vault://` 引用承接控制面写入，回读仅以掩码或引用形态呈现。

---

## 3 · 支持的功能

按对外接口组织本 crate 提供的能力：

- **保险箱解锁**：在启动序列中由选定的 `MasterKeySource::obtain()` 取得 32 字节主密钥，解开明文头中的包裹槽得到随机 data-key，再以 data-key 经 XChaCha20-Poly1305 解密 payload；明文头全段作为 AEAD 的 AAD 参与校验。`format_version` 不被当前实现识别时拒绝解锁（fail-closed）。
- **凭据解析（按 (res,tier)）**：`CredentialProvider::credential_for(res, tier)` 返回 `ResourceCredential`——一个不可复制、不可序列化、`Debug=REDACTED` 的不透明持有，生命周期不出调用方的单次建立通路调用。
- **地址解析**：`resolve(code) -> ResolvedTarget`——把资源代号映射为真实地址的不透明持有，约束同 `ResourceCredential`。
- **ScrubSet 句柄签发**：以单向 match-and-erase 句柄形式向内核签发系统级擦除集——句柄只能"匹配并擦除"，不可枚举、不可序列化，内容永不出现在任何输出路径。随保险箱写入更新。
- **机密录入/更新**：承接控制面对资源凭据与目标地址的写入（经 `vault://` 引用寻址），执行整体重加密的原子写入（写临时文件→`fsync`→原子 `rename` 覆盖，保留上一代 `.bak`），每次写入随机重生成 nonce，绝不复用。
- **rekey / rotate-kdf**：重包裹 data-key 或更换 KDF 参数（仅作用于 passphrase 来源的包裹槽），不触动 payload 加密语义。
- **导出协调支持**：向控制面导出时只产出元数据与 `vault://` 引用，永不含明文。

---

## 4 · 明确边界（不做什么）

每项排除均指明归属域（与第八部分 8.8「范围外」一致）：

- **不持有、不判定 Principal 凭证（网关凭证）的任何语义**——名称相近、概念无交集。Principal 凭证的有效期/吊销/可信域规则归**身份与凭证域**（8.4，认证器族）；其元数据持久化归**存储层**（8.11，`credentials` 表）。本域只经手资源凭据。
- **不声明、不选择 CredentialTier**——tier 的**声明**归**策略状态**（资源声明的一部分，存储承载，8.11 的 `resource_credential_tiers` 表）；动词→tier 的**选择**归**策略引擎**（8.3，`evaluate` 产出 `Decision::Allow{tier}`，无匹配 tier → deny）。本域只按已选定的 `(res, tier)` 解析出凭据，绝不参与 tier 的选取。
- **不执行脱敏**——脱敏的**调用职责**归**数据面请求内核**（8.2 出口）。本域只**构造、更新、持有 ScrubSet 并签发句柄**；句柄的应用（`scrub`/`scrub_stream`）由内核经 `Sanitizer` 执行（`daemon::sanitize`）。
- **不编排解锁时机**——保险箱解锁在启动序列的哪一步发生由 `daemon::boot` 编排（8.10/启动序列）。本域只提供 `MasterKeySource::obtain` 与解锁操作，不决定其调用时机。
- **不提供机密录入的人机入口**——录入的**操作入口**（CLI/控制面 API、`vault://` 引用语义的用户侧表达）归**控制面**（8.10）。本域只提供被控制面调用的写接口。
- **不建立通路、不解释协议、不持有连接**——通路建立归**传输**（8.7），用通路做什么归**适配器**（8.6）。本域只在 `Transport::open` 调用边界一次性交付 `(ResolvedTarget, ResourceCredential)`。
- **不持久化策略状态、不写审计载体**——策略/审计的载体归**存储层**（8.11）；保险箱的载体由本域自持，不经存储层。机密录入与轮换产生的审计事件（`lifecycle`/`credential_event`）由控制面在其事务路径上落地，本域不直接写审计。

---

## 5 · 对外接口

下列为本 crate 暴露给其他 crate 的类型与 trait（设计级签名，是设计承诺；实现可调内部细节但不得违背签名与不变量）。标注「定义」者其权威定义在 `postern-core`，本 crate 实现；标注「本域」者由本 crate 定义并持有。

### 5.1 机密类型（本域唯一构造）

```rust
/// 真实地址(代号解析产物)。仅在 postern-secrets 构造(契约 SEC_CONSTRUCTION_SITES);
/// 不实现 Clone/Serialize(契约 SEC_SECRET_TYPE_DISCIPLINE),Debug 恒输出 REDACTED,无 Display。
pub struct ResolvedTarget { /* 字段私有,不可在域外读取或重建 */ }

/// 资源凭据(按 (res,tier) 解析产物)。约束同 ResolvedTarget。
pub struct ResourceCredential { /* 字段私有 */ }
```

`ResolvedTarget` / `ResourceCredential` 在 `postern-core` 中仅有不透明声明（无构造路径）；本 crate 是其唯一构造点——这是公理四的编译期表达，由契约 `SEC_CONSTRUCTION_SITES`（构造点限定本 crate）与 `SEC_SECRET_TYPE_DISCIPLINE`（禁 derive/手写 `Clone`/`Serialize`）双重强制。

### 5.2 解锁来源族（trait 定义）

```rust
/// 解锁材料来源(D6):载体唯一,解锁方式按部署形态选择。
pub trait MasterKeySource {
    fn obtain(&self) -> Result<Zeroizing<[u8; 32]>, UnlockError>;
}
```

实现族与各自**真实强度的诚实说明**（取值由 `config.toml` 选定其一；强度不可一概宣称「argon2id 保护」）：

| 实现 | 主密钥获取方式 | 真实强度（诚实表述） | 适用形态 |
|---|---|---|---|
| `passphrase` | argon2id KDF 派生（**KDF 仅作用于本来源**） | 取决于口令熵 + argon2id 参数 | 有人值守（启动交互输入） |
| `key_file` | 直接持有 32 字节主密钥 | **等于文件系统权限**（与无口令 SSH 私钥同级），无 KDF 加固；任何能读到 key_file 的主体即可解锁全部凭据 | 无人值守 |
| `os_keychain` | 直接持有 32 字节主密钥 | 等于 OS 钥匙串保护强度（桌面外壳进程承担钥匙串交互并经受保护通道交付，daemon 不直接弹窗） | 桌面外壳桥接 |
| `systemd_cred` | 直接持有 32 字节主密钥 | 等于 systemd 凭据保护（可走 TPM 封存） | 无人值守、服务化 |

诚实约束：`passphrase` 仅适用于有人值守场景，与「常驻 daemon 重启需无人干预解锁」互斥；无人值守常驻应选受保护的自动解锁来源（`systemd_cred`/TPM、或受 OS 钥匙串保护的 `os_keychain`），不以明文缓存口令作变通。`key_file` 不作简单默认推荐，其弱模式威胁后果须显式声明。

### 5.3 凭据来源（trait 定义，本域实现）

```rust
/// 资源凭据来源。实现:StaticVaultProvider(静态保险箱);动态签发/证书为同接口预留。
#[async_trait]
pub trait CredentialProvider: Send + Sync {
    async fn credential_for(&self, res: &ResourceCode, tier: &CredentialTier)
        -> Result<ResourceCredential, CredentialError>;
}
```

### 5.4 地址解析与 ScrubSet 句柄（本域）

- `resolve(code: &ResourceCode) -> Result<ResolvedTarget, ResolveError>`——代号→真实地址的不透明解析。
- ScrubSet 不透明句柄——仅暴露 match-and-erase 能力（不可枚举、不可序列化），供内核经 `Sanitizer` 应用。其覆盖面：`targets` 中全部真实地址字符串/IP、`secrets` 中全部凭据值（及常见编码形态）、私网 IP 段模式、连接串模式。句柄随保险箱写入更新（新版本句柄原子替换交付给内核）。

> 诚实度界定：系统级 ScrubSet 本质是**黑名单**，不承诺识别全部编码变体；它是匿名化（Agent 只见代号、无从指定/获知真实地址）与凭据零接触（凭据从不经手 Agent、从不进入响应路径）之上的一层尽力而为兜底，而非绝对识别保证。

### 5.5 面向控制面的录入/更新接口（本域）

- 承接控制面对 `(资源凭据, 目标地址)` 的写入（经 `vault://` 引用寻址），执行整体重加密原子写入；回读默认仅以掩码或 `vault://` 引用形态呈现，绝不回吐明文。
- `vault://` 引用语义：`secrets` 键即 `vault://<code>/<tier-or-slot>` 引用路径；`targets` 即代号↔真实地址映射表。控制面与 policy.db 中只存 `vault://` 引用，永不存明文地址与凭据。

### 5.6 消费的 core 类型（定义在 core）

`ResourceCode`、`CredentialTier`（消费，不定义）；`Zeroizing` 容器纪律与机密类型纪律（与 core 一致）。

---

## 6 · 与相邻模块的交互

> 依赖方向：本 crate 仅依赖 `postern-core`（消费领域类型与插件 trait 定义）。本 crate **被** `postern-daemon` 依赖（唯一组装点）。本 crate **绝不被** `postern-adapters` / `postern-transports` / `postern-store` / `postern-cli` 依赖——这些边由契约 `ARCH_FORBIDDEN_EDGES` 强制（其中 `adapters↛secrets`、`cli↛secrets` 是禁止边的一部分）。下文逐一展开每一条真实存在的交互。

### 6.1 ← `postern-core`（本域消费 core）

- **方向**：`postern-secrets` → `postern-core`（编译期依赖，消费类型与 trait 定义）。
- **内容**：消费领域类型 `ResourceCode`、`CredentialTier`；实现 core 定义的插件 trait `CredentialProvider` 与机密类型纪律（`MasterKeySource` 为本域定义，见 4.3 代码分层）；`ResolvedTarget`/`ResourceCredential` 的不透明声明来自 core，构造体在本域。
- **时机**：编译期；解锁与解析在运行时使用这些类型。
- **失败语义**：core 为零 IO 纯定义层，无运行时失败面；类型纪律违反在编译期/契约期即被拦截，不进入运行时。

### 6.2 ← `postern-daemon::boot`（启动序列调本域解锁）

- **方向**：`daemon::boot` → `postern-secrets`（调用 `MasterKeySource::obtain()` 并触发保险箱解锁）。
- **内容**：boot 据 `config.toml` 选定的解锁来源构造对应 `MasterKeySource` 实现并调 `obtain()` 取 `Zeroizing<[u8;32]>` 主密钥；本域据此解开包裹槽、解密 payload，把解锁后的保险箱句柄（含 `CredentialProvider`、`resolve`、ScrubSet 句柄来源）交回 boot 装配。
- **时机**：启动序列「开库→重建快照→**解锁保险箱**→注册插件→开放数据面」的解锁步骤，**在开放数据面之前**（交互矩阵：`daemon::boot → secrets vault MasterKeySource obtain()`）。
- **失败语义**：fail-closed。`obtain()` 失败、`format_version` 不被识别、AAD 校验失败（头部篡改/降级）、payload 解密失败一律拒绝解锁；解锁未成功则**绝不开放数据面**——数据面在保险箱可用前不接受任何请求。错误向上抛前先脱敏为不含明文/真实地址的错误码。

### 6.3 ← `postern-daemon::connpool`（建立通路时取凭据与地址）

这是本域最关键的运行期交互——「凭据零接触」的承重点。

- **方向**：`daemon::connpool` → `postern-secrets`（调用 `credential_for(res, tier)` 与 `resolve(code)`）。
- **内容**：连接管理层在为某 `(ResourceCode, CredentialTier)` 建立通路时，向本域请求**不透明句柄** `(ResolvedTarget, ResourceCredential)`；本域返回的两个值不可 Clone/不可 Serialize、`Debug=REDACTED`。连接管理层**仅搬运、不可读取、不持有超出单次建立调用**，随即一次性传入 `Transport::open(target, cred)`。
- **时机**：求值管线步骤 **[7b] 取连接**——`Decision::Allow{tier}` 后，连接管理层按已选 tier 建连时（交互矩阵：`daemon::connpool → secrets CredentialProvider + 映射解析 → credential_for / resolve`，「建立通路时一次性取不透明句柄」）。tier 的选择已由策略引擎在更早步骤完成，本域只按既定 `(res, tier)` 解析，不参与选择。
- **失败语义**：fail-closed。无匹配 `(res, tier)`、解析失败、保险箱不可用一律返回 `CredentialError`/`ResolveError`，连接管理层据此 deny（步骤[7b]「不可建→deny」）。本域抛出的错误在跨 crate 边界前已脱敏为不含真实地址/凭据的错误码（红线 7.2-1：绝不让 `connection refused to 10.0.3.17` 一类原始串外泄）。句柄生命周期不出 `Transport::open` 调用；调用结束即时释放，不入池、不缓存。

### 6.4 ← `postern-daemon::sanitize`（持有并应用 ScrubSet 句柄）

- **方向**：`postern-secrets` → `daemon`（签发方向）：本域在保险箱解锁后构造 ScrubSet 并**签发不透明 match-and-erase 句柄**给 daemon；`daemon::sanitize` 持该句柄并经 `Sanitizer` 应用（句柄是 daemon 持有、本域签发与更新）。
- **内容**：单向匹配视图句柄——只能匹配并擦除，不可枚举、不可序列化。`daemon::sanitize` 把它与声明级 `MaskRule`（来自 `grant_constraints.kind='mask_fields'`）组合，作用于一切离开内核的字节。
- **时机**：句柄**签发**于启动序列解锁之后（随保险箱句柄一同交回 boot 装配）；句柄**更新**于每次保险箱写入之后（新版本句柄原子替换交付）；句柄**应用**于求值管线步骤 **[9] 脱敏**出口（交互矩阵：`daemon::kernel → secrets Sanitizer/ScrubSet 句柄 → scrub / scrub_stream`，步骤[9]出口）。
- **失败语义**：ScrubSet 是系统级黑名单兜底（恒生效）。句柄内容永不出现在输出路径；句柄不可被 daemon 读出内容（即便 daemon 代码亦然）。脱敏的调用职责在内核（本域不执行脱敏调用）；脱敏本身无「放行」语义分支——擦除是单向的，失败不会变成「未擦除直出」（白名单输出是高敏感场景的更强手段，归适配器/细则声明，不在本域）。

### 6.5 ← `postern-daemon::control`（机密录入与更新）

- **方向**：`daemon::control` → `postern-secrets`（调用本域录入/更新写接口）。
- **内容**：控制面承接人类操作者经 `control.sock` 的机密录入（资源凭据、目标地址），以 `vault://` 引用寻址，把待写明文交给本域；本域执行整体重加密的原子写入并更新 ScrubSet 句柄。回读时本域只返回掩码或 `vault://` 引用形态。policy.db 与导出文件中只落 `vault://` 引用，绝无明文（与 6.6 声明式导入导出的 `host_ref = "vault://..."` 不变量一致）。
- **时机**：控制面写操作期间（资源接入/凭证签发轮换/rekey 等带机密的管理命令）；不在数据面求值管线内（数据面对机密无任何写或读路径）。
- **失败语义**：fail-closed。原子写入未完成则保留上一代 `.bak`，vault 不被半写损坏；写入失败向控制面返回错误，控制面不提交相应策略变更（控制面写入是「一次事务 + 快照重建 + 审计事件」三联动，机密写失败即整体不生效）。机密录入/轮换成功后由控制面在其路径上落 `lifecycle`/`credential_event` 审计——本域不直接写审计载体，但写入的事实经控制面留痕。

### 6.6 被禁止的依赖边（绝不交互，明示以防误连）

- `postern-adapters` **不得**依赖本 crate（`adapters↛secrets`，契约 `ARCH_FORBIDDEN_EDGES`）——适配器只见 `Channel`，永不经手凭据与真实地址。
- `postern-transports` **不得**依赖本 crate——传输只在 `Transport::open` 调用边界**接收**由 daemon 注入的 `(ResolvedTarget, ResourceCredential)`，不依赖本 crate 的构造路径（依赖图中 transports 与 secrets 是 core 的并列子节点，无边相连）。
- `postern-store` **不得**依赖本 crate（且 store 不存任何真实地址/凭据明文，仅存 `vault://` 引用与单向 `secret_hash`）。
- `postern-cli` **不得**依赖本 crate（`cli↛secrets`）——CLI 是瘦客户端，机密录入经控制面 API，不含任何机密逻辑。

---

## 7 · 必守不变量

下列不变量本域必须守住，标注其强制手段（契约 / 类型签名 / 文件协议）：

1. **机密类型纪律**（契约 `SEC_SECRET_TYPE_DISCIPLINE`）：`ResolvedTarget` / `ResourceCredential` 不得 derive 或手写 `impl Clone` / `Serialize`，`Debug` 恒输出 `REDACTED`，无 `Display`，明文置于 `Zeroizing` 容器。tracing/日志在类型层即无法直接记录它们。

2. **构造点唯一**（契约 `SEC_CONSTRUCTION_SITES`）：`ResolvedTarget` / `ResourceCredential` 只能在 `postern-secrets` 构造；任何其他 crate（含 daemon、adapters、transports）出现其构造表达即契约红。

3. **凭据零接触**（公理四）：资源凭据与真实地址明文**只存在于本域内存与 `Transport::open` 调用生命周期内**，永不向上传递、永不落任何返回 Agent 的字节、永不落审计/运行日志；跨 crate 边界前的错误一律脱敏为不含真实地址/凭据的错误码。

4. **数据面无取用路径**：数据面对本域无任何读/写路径——这由依赖图（数据面 router 注入集合不含 vault 句柄）与 `ARCH_FORBIDDEN_EDGES`（`adapters↛secrets`、`cli↛secrets`）共同保证。

5. **ScrubSet 单向性**：本域签发的 ScrubSet 句柄只可 match-and-erase，不可枚举、不可序列化；句柄内容永不出现在输出路径，即便持有者（daemon）亦不可读出其内容。

6. **vault 格式与写入纪律**（文件协议）：明文头全段作为 AEAD AAD（防头部篡改/`format_version` 降级）；`format_version` 不被识别即拒绝解锁（fail-closed）；每次写入随机重生成 nonce、**绝不复用**（XChaCha20-Poly1305 的 24B nonce 空间使随机生成安全）；原子写入（临时文件→`fsync`→原子 `rename`，保留 `.bak`），半写不损坏 vault。

7. **解锁强度诚实**：各 `MasterKeySource` 实现按 5.2 表如实声明真实强度，不夸大；`passphrase` 与无人值守互斥的约束、`key_file` 弱模式威胁后果须显式表达。

8. **导出永不含明文**：面向控制面/声明式导出只产出元数据与 `vault://` 引用；存储层只存 `vault://` 引用与单向 `secret_hash`，本域是明文的唯一可达处。

9. **属主一致性**：tier 的声明/选择不在本域（归策略状态/策略引擎）；脱敏调用不在本域（归内核出口）；解锁时机编排不在本域（归 boot）；机密录入入口不在本域（归控制面）。本域只持有、解析、构造、签发——与第八部分 8.8 的属主裁决严格一致。

---

## 8 · 验收标准

本节是 `postern-secrets` 的**验收基准**：每条给出「可观察判据」与「验证方式」，让实现者据此自检、审查者据此判定本模块是否完成。判据一律「输入→预期可观察结果」，验证方式取统一词汇。下文按 A~F 六维度组织，与本模块相关者逐条列出，不相关者明示「不适用」。**凡只能靠人工审查、无法机器验证的项，均在该条标注「仅审查」**。

### 8.A 功能完整性（逐条对应 §3 功能）

| # | 功能（§3） | 验收判据（输入→可观察结果） | 验证方式 |
|---|---|---|---|
| A1 | 保险箱解锁 | 输入：合法 `vault.postern` + 正确主密钥经 `MasterKeySource::obtain()` 取得 → 可观察：解开包裹槽得 data-key、以 XChaCha20-Poly1305 解密 payload 成功、明文头全段作为 AAD 参与校验通过，返回可用保险箱句柄。输入：篡改明文头任一字节 / data-key 错误 → 可观察：AEAD 校验失败、`UnlockError`、不返回句柄。 | `集成测试(内存Fake vault 字节)`；`场景规格 docs/examples/02 §4.2-E6` |
| A2 | `format_version` fail-closed | 输入：vault 明文头 `format_version` 取当前实现不识别的值 → 可观察：拒绝解锁、返回 `UnlockError`，绝不尝试按未知格式解密。 | `单元测试`；不变量见 D6 |
| A3 | 凭据解析（按 (res,tier)） | 输入：已解锁保险箱 + 存在的 `(ResourceCode, CredentialTier)` 调 `credential_for(res, tier)` → 可观察：返回一个 `ResourceCredential`（不可 Clone/Serialize、`Debug=REDACTED`）。输入：不存在的 `(res, tier)` → 可观察：返回 `CredentialError`，不返回任何凭据。 | `集成测试(内存Fake)`；`场景规格 docs/examples/04 §4.1 Trace② [7b]` |
| A4 | 地址解析 | 输入：存在的 `code` 调 `resolve(code)` → 可观察：返回 `ResolvedTarget`（约束同 A3）。输入：未知 `code` → 可观察：`ResolveError`，无产物。 | `集成测试(内存Fake)`（已知 code→产物 / 未知 code→`ResolveError`）；解析参与建连的时机见 `场景规格 docs/examples/04 §4.1 Trace② [7b]`；解析失败下游 deny 见 F2 |
| A5 | ScrubSet 句柄签发 | 输入：解锁后由 `targets`/`secrets` 派生 ScrubSet，向内核签发句柄 → 可观察：句柄仅暴露 match-and-erase；对句柄调用枚举/序列化在类型层即不存在该方法（编译期不可达）。输入：句柄作用于含真实地址/凭据的字节流 → 可观察：命中段被擦除。 | `构造签名审查`；`单元测试(匹配-擦除行为)`；`场景规格 docs/examples/04 §4.1 Trace③ [9]` |
| A6 | 机密录入/更新（原子写入） | 输入：经 `vault://` 引用写入新凭据/地址 → 可观察：整体重加密、写临时文件→`fsync`→原子 `rename` 覆盖、保留上一代 `.bak`、本次 nonce 与历史不同。输入：写入中途失败 → 可观察：原 vault 不被半写损坏、`.bak` 可回退。 | `集成测试(真实文件系统)`；不变量见 D6；`场景规格 docs/examples/02 §4.1 步骤2/3` |
| A7 | rekey / rotate-kdf | 输入：rekey 重包裹 data-key 或换 argon2id 参数（仅 passphrase 来源的包裹槽）→ 可观察：payload 加密语义不变（同一 data-key 仍可解原 payload），仅包裹槽更新。 | `集成测试(内存Fake)`；不变量见 D6/D7 |
| A8 | 导出协调支持 | 输入：向控制面导出 → 可观察：产物只含元数据与 `vault://` 引用，逐字段扫描无任何明文地址/凭据。 | `场景规格 docs/examples/02 §4.1 步骤2`（`export` 产物逐字段可验收）；不变量见 D8 |

### 8.B 对外接口契约（逐条对应 §5 暴露的类型/trait）

| # | 接口（§5） | 验收判据 | 验证方式 |
|---|---|---|---|
| B1 | `ResolvedTarget` / `ResourceCredential` 类型纪律 | 两类型无 `Clone`/`Serialize`（derive 或手写均无）、`Debug` 恒输出 `REDACTED`、无 `Display`、明文置 `Zeroizing`；字段私有、域外无构造/读取路径。 | `Stele契约 SEC_SECRET_TYPE_DISCIPLINE`；`构造签名审查` |
| B2 | 机密类型唯一构造点 | `ResolvedTarget`/`ResourceCredential` 的构造表达只出现在 `postern-secrets`；任何其他 crate 出现构造即契约红。 | `Stele契约 SEC_CONSTRUCTION_SITES` |
| B3 | `MasterKeySource::obtain()` 签名与语义 | 签名 `obtain(&self) -> Result<Zeroizing<[u8;32]>, UnlockError>` 稳定；成功返回 `Zeroizing` 包裹的 32 字节主密钥，失败返回 `UnlockError`，错误路径不泄漏材料明文。 | `构造签名审查`；`单元测试` |
| B4 | `CredentialProvider::credential_for` 签名与错误路径 | 签名 `async fn credential_for(&self, res, tier) -> Result<ResourceCredential, CredentialError>` 稳定；无匹配/解析失败/库不可用走 `Err` 分支，不返回缺省凭据。本域恒以 `Result::Err` 上抛（签名层即无"缺省凭据"返回路径），消费侧（`daemon::connpool`/`kernel`）对该 `Err` 不得吞为放行——后者在求值路径由契约强制。 | `构造签名审查`（返回类型为 `Result`、无缺省分支）；`集成测试(内存Fake)`（错误输入→`Err` 而非缺省）；消费侧不吞错见 E3，由 `Stele契约 EVAL_NO_ERROR_SWALLOWING`（作用域：`core::eval`/`daemon::kernel`）强制 |
| B5 | `resolve` / ScrubSet 句柄接口 | `resolve(code) -> Result<ResolvedTarget, ResolveError>` 错误路径正确；ScrubSet 句柄仅 match-and-erase、无枚举/序列化能力（接口层不暴露）。 | `构造签名审查`；`单元测试` |
| B6 | 录入/更新写接口回读形态 | 经写接口写入后回读 → 只返回掩码或 `vault://` 引用，绝不回吐明文。 | `集成测试(内存Fake)`；`场景规格 docs/examples/02 §4.1 步骤3` |
| B7 | 消费 core 类型不重复定义 | `ResourceCode`/`CredentialTier` 仅从 core 消费，本域不另行定义；`ResolvedTarget`/`ResourceCredential` 的不透明声明来自 core、构造体在本域。 | `构造签名审查`；`Stele契约 SEC_CONSTRUCTION_SITES` |

### 8.C 边界（禁止项，逐条对应 §4「不做什么」）

| # | 禁止项（§4） | 验收判据（确实没做 / 无该路径） | 验证方式 |
|---|---|---|---|
| C1 | 不持有/判定 Principal 凭证 | 本 crate 无任何 Principal 凭证（网关凭证）的有效期/吊销/可信域语义代码路径；只经手资源凭据。 | `构造签名审查（仅审查）`；依赖边见 C8 |
| C2 | 不声明/选择 CredentialTier | 本域 API 仅接收已选定的 `(res, tier)`，无任何"动词→tier 选择"逻辑；`credential_for` 入参 `tier` 由调用方给定。 | `构造签名审查（仅审查）` |
| C3 | 不执行脱敏 | 本域只构造/更新/持有 ScrubSet 并签发句柄；无 `scrub`/`scrub_stream` 调用点（脱敏调用在 `daemon::sanitize`）。 | `构造签名审查（仅审查）`；交互见 E4 |
| C4 | 不编排解锁时机 | 本域只提供 `obtain`/解锁操作，不含"在启动序列哪一步解锁"的编排代码。 | `构造签名审查（仅审查）` |
| C5 | 不提供机密录入人机入口 | 本 crate 无 CLI/HTTP 录入入口，只暴露被控制面调用的写接口。 | `构造签名审查（仅审查）`；`Stele契约 ARCH_FORBIDDEN_EDGES`（`cli↛secrets`） |
| C6 | 不建立通路/解释协议/持有连接 | 本域不依赖 `transports`、不含 `Transport::open` 实现；只在该调用边界一次性交付 `(ResolvedTarget, ResourceCredential)`。 | `Stele契约 ARCH_FORBIDDEN_EDGES`（依赖图中 secrets 与 transports 无边）；`cargo tree`（无 transports 依赖） |
| C7 | 不持久化策略状态/不写审计载体 | 本 crate 无 rusqlite/policy.db 写路径、无 `AuditSink` 写调用；vault 载体本域自持、不经 store。 | `Stele契约 ARCH_FORBIDDEN_EDGES`（`secrets↛store` 由依赖图保证，secrets 不依赖 store）；`cargo tree`（无 store 依赖）；`构造签名审查` |
| C8 | 不被数据面 crate 依赖 | `adapters↛secrets`、`cli↛secrets` 等禁止边不出现；本 crate 只被 `daemon` 依赖。 | `Stele契约 ARCH_FORBIDDEN_EDGES`；`cargo tree`（反向依赖仅 daemon） |

### 8.D 必守不变量（逐条对应 §7，沿用 §7 已标强制手段）

| # | 不变量（§7） | 验收判据 | 验证方式（沿用 §7） |
|---|---|---|---|
| D1 | 机密类型纪律 | `ResolvedTarget`/`ResourceCredential` 无 `Clone`/`Serialize`、`Debug=REDACTED`、无 `Display`、明文在 `Zeroizing`；日志/tracing 在类型层即无法直接记录。 | `Stele契约 SEC_SECRET_TYPE_DISCIPLINE`（含其反例自检 teeth：在 `derive(Clone)` 反例上须检出违规） |
| D2 | 构造点唯一 | 两机密类型仅在本 crate 构造；他处构造表达 → 契约红。 | `Stele契约 SEC_CONSTRUCTION_SITES`（含反例自检 teeth） |
| D3 | 凭据零接触（公理四） | 资源凭据/真实地址明文只活在本域内存与 `Transport::open` 调用生命周期内；不向上传递、不落返回 Agent 的字节、不落审计/运行日志；跨 crate 边界前错误脱敏为不含真实地址/凭据的错误码。 | 类型层不可外泄由 `Stele契约 SEC_SECRET_TYPE_DISCIPLINE`（无 `Clone`/`Serialize`，`Debug=REDACTED`）保证（见 D1）；`集成测试(内存Fake)`（断言成功/错误/拒绝路径返回串均不含原始凭据/地址）；`场景规格 docs/examples/04 §4.2-G`（Agent 拿不到 app 账号）；`postern verify 响应脱敏探测`（6.7 第 8 项，诱导回显被擦净）；诚实度边界仅审查 |
| D4 | 数据面无取用路径 | 数据面 router 注入集合不含 vault 句柄；`adapters↛secrets`/`cli↛secrets` 成立。 | `Stele契约 ARCH_FORBIDDEN_EDGES`；`场景规格 docs/examples/04 §4.2-G`（Agent 拿不到 app 账号） |
| D5 | ScrubSet 单向性 | 句柄只可 match-and-erase、不可枚举/序列化；持有者（daemon）亦无法读出内容；内容永不进输出路径。 | `构造签名审查`（接口层无枚举/序列化方法，类型不可 `Serialize`）；`单元测试`（含真实地址/凭据样本的字节流命中段被擦除）；`场景规格 docs/examples/04 §4.1 Trace③ [9]`；句柄擦除生效另由 `postern verify 响应脱敏探测`（6.7 第 8 项）端到端验证 |
| D6 | vault 格式与写入纪律 | 明文头全段作 AAD（头部篡改/`format_version` 降级 → 解锁失败）；`format_version` 不识别即拒锁；每次写入随机重生成 nonce、绝不复用；原子写入（临时文件→`fsync`→`rename`，保留 `.bak`），半写不损坏。 | `集成测试(真实文件系统)`（验证 nonce 不复用：连续写入采样 nonce 互异；验证半写后 `.bak` 可回退）；`单元测试`（AAD 篡改/版本降级拒绝）；`场景规格 docs/examples/02 §4.2-E6` |
| D7 | 解锁强度诚实 | 各 `MasterKeySource` 实现按 §5.2 表如实声明强度；`passphrase` 与无人值守互斥、`key_file` 弱模式威胁后果须显式表达；不夸大「argon2id 保护」。 | `仅审查`（文档/注释级诚实声明，无机器判据） |
| D8 | 导出永不含明文 | 导出只产元数据 + `vault://` 引用；本域是明文唯一可达处。 | `场景规格 docs/examples/02 §4.1 步骤2`（`export` 逐字段无明文）；`集成测试(内存Fake)` |
| D9 | 属主一致性 | tier 声明/选择、脱敏调用、解锁时机编排、录入入口均不在本域代码内。 | `构造签名审查（仅审查）`；与 C2/C3/C4/C5 互证 |

### 8.E 与相邻模块交互（逐条对应 §6 每个相邻交互；方向/类型/时机/失败语义可验）

| # | 交互（§6） | 验收判据（方向/类型/时机 + 失败 fail-closed） | 验证方式 |
|---|---|---|---|
| E1 | ← core（§6.1 消费 core） | 方向：secrets→core 编译期依赖；消费 `ResourceCode`/`CredentialTier`、实现 `CredentialProvider`；core 无运行时失败面。 | `cargo tree`（依赖边 secrets→core 存在）；`Stele契约 ARCH_FORBIDDEN_EDGES`（`core↛任何 IO crate` 反向保证 core 纯净） |
| E2 | ← daemon::boot（§6.2 解锁） | 方向：boot→secrets 调 `obtain()` 并触发解锁，在**开放数据面之前**；失败（obtain 失败/版本不识别/AAD 失败/解密失败）→ 不返回句柄、**绝不开放数据面**，错误先脱敏。 | `集成测试(内存Fake)`；`场景规格 docs/examples/02 §4.2-E6`、`docs/examples/04 §4.2-H`（重启后解锁→开放数据面顺序） |
| E3 | ← daemon::connpool（§6.3 取凭据与地址） | 方向：connpool→secrets 调 `credential_for`/`resolve`，时机为求值步骤 **[7b] 取连接**；返回不可 Clone/Serialize 的不透明句柄，调用方仅搬运、一次性传入 `Transport::open`；失败（无匹配/解析失败/库不可用）→ 本域返回 `CredentialError`/`ResolveError`，消费侧 connpool 据此 deny 且**不吞错放行**，错误已脱敏（不泄 `connection refused to 10.0.3.17` 类原始串）。 | `集成测试(内存Fake)`（secrets 返回 `Err`、错误串无明文）；消费侧不吞错由 `Stele契约 EVAL_NO_ERROR_SWALLOWING`（作用域 `daemon::kernel`，含求值路径上的连接获取消费点）强制；`场景规格 docs/examples/04 §4.1 Trace② [7b]、§4.2-D`、`docs/examples/02 §4.2-E6` |
| E4 | ← daemon::sanitize（§6.4 签发并应用 ScrubSet） | 方向：secrets→daemon 签发不透明句柄（签发于解锁后、更新于每次写入后、应用于步骤 **[9]**）；daemon 持有但不可读出内容；脱敏调用在内核、本域不调用；擦除单向、无「未擦除直出」分支。 | `构造签名审查`；`场景规格 docs/examples/04 §4.1 Trace③ [9]、§4.2-C` |
| E5 | ← daemon::control（§6.5 录入与更新） | 方向：control→secrets 调写接口，经 `vault://` 引用寻址；时机在控制面写操作期间、**不在数据面管线内**；失败 fail-closed（原子写未完成保留 `.bak`、vault 不半写损坏，写失败→control 不提交策略变更）；审计由 control 落、本域不直接写审计。 | `集成测试(真实文件系统)`（半写回退见 F5 + 回读仅掩码/引用）；写入与匿名化建立见 `场景规格 docs/examples/02 §4.1 步骤2、步骤3` |
| E6 | 被禁止依赖边（§6.6） | `adapters`/`transports`/`store`/`cli` 均不依赖本 crate；本 crate 只被 daemon 依赖。 | `Stele契约 ARCH_FORBIDDEN_EDGES`（含反例自检 teeth：adapters 声明 secrets 依赖须检出）；`cargo tree` |

### 8.F 失败与边界行为（关键 fail-closed 路径逐条可验）

| # | 失败路径 | 验收判据（错误→拒绝 / 不可建→拒绝 / 不可解锁→不放行） | 验证方式 |
|---|---|---|---|
| F1 | 解锁失败 → 不放行 | `obtain()` 失败 / `format_version` 不识别 / AAD 校验失败 / payload 解密失败 → 解锁失败、不返回句柄、数据面在保险箱可用前不接受任何请求。 | `集成测试(内存Fake)`；`场景规格 docs/examples/02 §4.2-E6` |
| F2 | 凭据/地址不可解析 → deny | 无匹配 `(res,tier)` / `resolve` 未知 code / 库不可用 → 本域返回 `Err`、不返回缺省值；消费侧 connpool 据此 deny（步骤[7b]「不可建→deny」）且不吞错放行。 | `集成测试(内存Fake)`（本域错误输入→`Err`）；消费侧不吞错由 `Stele契约 EVAL_NO_ERROR_SWALLOWING`（作用域 `daemon::kernel`）强制；`场景规格 docs/examples/04 §4.2-D` |
| F3 | 红队探测下本域不泄机密（凭据/地址） | 模拟红队以已吊销/过期凭证发起请求 → 认证层 deny（认证层职责，非本域）且 deny 进审计；**本域可验判据**：该探测全程本域的凭据/地址解析路径与脱敏后的拒绝响应中无任何原始凭据/真实地址泄漏。 | `postern verify 已吊销/过期凭证`（6.7 第 5 项，作为驱动该探测的集成自检）；本域非泄漏判据另由 `集成测试(内存Fake)` 断言；`场景规格 docs/examples/02 §4.1 步骤9`（九项逐条，第⑤项）、`docs/examples/04 §4.2-B(B2 硬过期)` |
| F4 | 会话硬过期不可续 → fail-closed（本域承重面：凭据零接触） | 续会话失败（账号密码失效/refresh token 失效/强制 2FA 无长效会话）时，本域对 connpool 的凭据解析仍只交付不透明句柄、绝不在错误/拒绝路径回吐账号明文；最终请求 deny 为代号化拒绝、不泄账号信息。（"绝不数据面静默重登/触发 2FA" 的编排归 connpool/6.13，非本域。） | `集成测试(内存Fake)`（断言 deny/错误路径不含账号明文）；`场景规格 docs/examples/04 §4.2-B(B2 硬过期)` |
| F5 | 原子写入失败 → vault 不损坏 | 写入中途中断（临时文件写到一半 / `rename` 前进程退出）→ 原 vault 完好可解锁、`.bak` 可回退、control 不提交相应策略变更。 | `集成测试(真实文件系统)`（注入中断点后断言原 vault 仍可解锁、`.bak` 回退成功）；不变量见 D6（原子写入协议）；写入路径与回读形态见 `场景规格 docs/examples/02 §4.1 步骤3` |
| F6 | 跨边界错误脱敏 | 任一向上抛出的错误（解锁/解析/写入）在跨 crate 边界前已脱敏为不含真实地址/凭据的错误码。 | `集成测试(内存Fake)`（断言错误串不含明文）；`场景规格 docs/examples/04 §4.2-D/G`；诚实度边界部分仅审查 |

### 8.G 完成定义（Definition of Done）

**`postern-secrets` 同时满足上述 A~F 全部验收项——即 §3 全部功能可验产出、§5 全部接口签名与错误路径符合承诺、§4 全部禁止项经契约/依赖图/构造签名证实未做、§7 全部不变量由其标注的 Stele 契约/签名/文件协议强制、§6 全部相邻交互按约定方向·类型·时机·fail-closed 语义可验、全部关键失败路径解析为拒绝且不泄机密——时，视为本模块完成。** 凡标注「仅审查」的项（D7 解锁强度诚实、C1~C5/D9 的属主与无路径性中无法被现有契约/依赖图覆盖的部分、D3/F6 中诚实度边界的主观表述）须经人工审查通过，作为完成判定的补充条件。
