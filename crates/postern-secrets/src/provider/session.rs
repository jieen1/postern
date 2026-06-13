//! 会话来源（live-session）凭据物化（设计承诺级桩，函数体 `todo!()`）。
//!
//! 职责（§3 凭据解析「会话来源」/ §3.1 并发模型 / 详细设计 6.13）：会话来源**有状态**
//! ——"登录一次 → 复用 → 续期/重登"，绝不每请求重登。进程内一张以
//! `(ResourceCode, CredentialTier)` 为键的 **live-session 缓存**，值含活跃会话/令牌
//! （`Zeroizing` 持有）、`expiry`（硬过期墙钟）、"是否有在途续会话"的并发占位。
//!
//! 三档形态（§8 F-7 / 详细设计 6.13 表）：
//! - `PasswordSession`（①账号密码，默认）：临近过期 → 用 vault 账号密码**无人值守重登**（L-7）；
//! - `ApiToken`（②长效 token）：通常无需续，直接取用；
//! - `OAuthRefresh`（③OAuth/强制 2FA）：用 refresh token 换新 access token；运行期建连路径
//!   **无 2FA 触发点**，会话不可续走 deny 而非触发 2FA（L-10、F-7）。
//!
//! 续期判定（详细设计 6.13）：命中且 `now < expiry − skew` → 直接复用缓存（**不重登**，L-8）；
//! 缺失 / 临近过期（`now ≥ expiry − skew`）→ 触发续会话，成功回填缓存。
//!
//! 单飞（§3.1 锁纪律两红线 / L-9）：同一 `(res, tier)` 任一时刻**至多一个在途续会话**；
//! 并发等待者**复用同一在途产物**（`ResourceCredential` 不可 Clone，等待者复用同一会话
//! 物化结果而非克隆凭据），杜绝登录/刷新风暴。登录的网络等待**不持表锁**。
//!
//! fail-closed（§8 L-10 / 详细设计 6.13）：续会话不可建立（账号密码失效/refresh 失效/
//! 强制 2FA 无长效会话）→ `Err(CredentialError::RefreshFailed | InteractiveAuthRequired)`、
//! 请求 deny；**绝不在数据面静默重试、绝不在运行期触发 2FA**；错误不回吐账号明文（L-11）。
//!
//! 内存纪律（§7-1）：会话值/令牌全程 `Zeroizing`，绝不落盘（短效会话只在内存）、绝不进
//! log/错误文案/审计；本域不直接写审计（`credential_event` 归控制面/连接管理层）。
//!
//! 测试可决定性：续期判定用**注入时钟**（`Clock`）而非真实墙钟；续会话经**注入的认证
//! 端点**（`SessionAuthority`）而非真实业务系统——使 L-7/L-8/L-9/L-10 确定可复现。

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::Poll;

use zeroize::Zeroizing;

use postern_core::domain::{CredentialTier, ResourceCode, ResourceCredential};
use postern_core::error::CredentialError;

/// 把一条活跃会话/令牌明文物化成不透明 `ResourceCredential`（本 crate 唯一构造点，
/// 契约 SEC_CONSTRUCTION_SITES）。明文取自 `Zeroizing<String>`，只在此步落入材料串。
fn materialize_session(session_value: &Zeroizing<String>) -> ResourceCredential {
    ResourceCredential // sole construction point in this crate
    {
        material: session_value.as_str().to_string(),
    }
}

/// 注入式时钟（续期判定的时间源）。生产实现取真实墙钟；测试用 Fake 时钟可推进到
/// `expiry − skew` 与硬过期，使续期路径确定可复现（§3.1 测试策略）。
pub trait Clock: Send + Sync {
    /// 当前墙钟（毫秒）。续期判定只比较该值与缓存条目的 `expiry`/`skew`。
    fn now_millis(&self) -> u64;
}

/// 持久凭据的三档形态（详细设计 6.13 表；§8 F-7）。决定"会话过期后如何续"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionForm {
    /// ①账号密码（默认）：用 vault 账号密码无人值守重登（L-7）。
    PasswordSession,
    /// ②API token：长效 token，通常无需续、直接取用。
    ApiToken,
    /// ③OAuth refresh / 强制 2FA：用 refresh token 换新 access token；
    /// 运行期会话不可续 → deny，**绝不触发 2FA**（L-10）。
    OAuthRefresh,
}

/// 一次续会话成功的产物：新会话/令牌明文（`Zeroizing`）+ 新硬过期墙钟（毫秒）。
/// 不实现 `Clone`/`Serialize`（值经 `Zeroizing`，不向外复制明文）。
pub struct RenewedSession {
    /// 续会话取得的活跃会话/访问令牌明文（`Zeroizing`，绝不落盘/日志）。
    pub session_value: Zeroizing<String>,
    /// 新硬过期墙钟（毫秒）——回填缓存条目的 `expiry`。
    pub expiry_millis: u64,
}

/// 注入式认证端点（续会话的执行者）。生产实现经 `Transport` 通路对业务系统认证端点发起
/// 一次登录/刷新；测试用 Fake 端点可编排"登录成功 / 失败 / 需交互"并记录登录次数
/// （用于 L-9 单飞观察）。async：续会话是对认证端点的网络调用（§3.1 会话来源涉及 await）。
pub trait SessionAuthority: Send + Sync {
    /// 对某 `(res, tier)` 按其 `form` 续会话（①重登 / ③刷新）。成功 → `RenewedSession`；
    /// 失败 → `RefreshFailed`（账号密码/refresh 失效）/ `InteractiveAuthRequired`
    /// （强制 2FA 无长效会话，仅 ③ 档）。**绝不在此触发 2FA**——③档运行期不可续即此错误。
    fn renew<'a>(
        &'a self,
        res: &'a ResourceCode,
        tier: &'a CredentialTier,
        form: SessionForm,
    ) -> Pin<Box<dyn Future<Output = Result<RenewedSession, CredentialError>> + Send + 'a>>;
}

/// live-session 缓存条目：活跃会话/令牌（`Zeroizing`）+ 硬过期墙钟。
/// 不 derive `Debug`（不落会话明文）；值不向外复制（凭据零接触）。
struct SessionEntry {
    /// 活跃会话/访问令牌明文（`Zeroizing`，绝不落盘）。
    session_value: Zeroizing<String>,
    /// 硬过期墙钟（毫秒）。`now ≥ expiry − skew` 即触发续会话。
    expiry_millis: u64,
}

/// 会话来源凭据提供者：持有 live-session 缓存 + 注入时钟 + 注入认证端点。
///
/// 不拥有 runtime、不 spawn（§3.1）；续会话在调用方任务上下文内 `await`。缓存与续期
/// 单飞是唯一的细粒度并发控制点（§3.1 运行期高并发点）。`A`/`C` 为认证端点/时钟实现。
pub struct LiveSessionProvider<A: SessionAuthority, C: Clock> {
    /// 该来源服务的形态（三档之一；接入期声明，运行期据此续会话）。
    form: SessionForm,
    /// 续期提前量（毫秒）：`now ≥ expiry − skew` 即主动续（重叠窗口，详细设计 6.13）。
    skew_millis: u64,
    /// live-session 缓存（键 = `(res, tier)`）。整表与值在内存，绝不落盘。
    cache: Mutex<BTreeMap<(ResourceCode, CredentialTier), SessionEntry>>,
    /// 单飞门（L-9）：当前**正在续会话**的键集合。同一 `(res, tier)` 任一时刻至多一个
    /// 在途续会话；并发等待者见键在集合内即**不再发起 renew**，让出后复用 leader 回填的
    /// 缓存（杜绝登录/刷新风暴）。与 `cache` 分锁，锁只在登记/撤销在途标记时短持，
    /// **绝不跨 `await` 持有**（续会话网络等待不持任何锁，§3.1）。
    in_flight: Mutex<BTreeSet<(ResourceCode, CredentialTier)>>,
    /// 注入的认证端点（续会话执行者）。
    authority: A,
    /// 注入的时钟（续期判定时间源）。
    clock: C,
}

/// 让出一次执行权的 future（单飞 follower 的协作让出点）：首 `poll` 唤醒自身并 `Pending`，
/// 次 `poll` `Ready`。本 crate 不拥有 runtime、不 spawn——follower 以此把控制权交还执行器，
/// 待 leader 在途续会话推进、回填缓存后再复查（§3.1 续会话在调用方任务上下文驱动）。
struct YieldOnce {
    yielded: bool,
}

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// follower 复查让出次数上限（fail-closed 防御）：协作执行器下 leader 必在有限步内回填
/// 缓存或撤销在途标记，follower 至多让出该次数即得结果或自身转为 leader；超出即视会话面
/// 不可用 → `VaultUnavailable`，绝不无界自旋。
const MAX_SINGLE_FLIGHT_YIELDS: u32 = 1_024;

impl<A: SessionAuthority, C: Clock> LiveSessionProvider<A, C> {
    /// 绑定形态、续期提前量、认证端点与时钟，构造空缓存的会话来源。
    pub fn new(form: SessionForm, skew_millis: u64, authority: A, clock: C) -> Self {
        Self {
            form,
            skew_millis,
            cache: Mutex::new(BTreeMap::new()),
            in_flight: Mutex::new(BTreeSet::new()),
            authority,
            clock,
        }
    }

    /// 只读借出注入的认证端点。生产侧无消费者；测试侧据此观察续会话被触发的次数
    /// （L-9 单飞）。不暴露会话明文，只暴露注入实现本身。
    pub fn authority(&self) -> &A {
        &self.authority
    }

    /// 预置一条缓存会话（测试夹具/接入期回填入口）：把一条活跃会话直接置入缓存，
    /// 模拟"已登录一次"的稳态，用于 L-8 命中复用 / L-7 临近过期续期的初始条件。
    pub fn seed_session(
        &self,
        res: ResourceCode,
        tier: CredentialTier,
        session_value: Zeroizing<String>,
        expiry_millis: u64,
    ) {
        // 直接置入缓存（已登录稳态）。锁短持，无 await 跨持。
        let mut cache = match self.cache.lock() {
            Ok(g) => g,
            // 锁中毒（panic 期间持锁）极端态：fail-closed 不写入，留缓存原样。
            Err(_) => return,
        };
        cache.insert(
            (res, tier),
            SessionEntry {
                session_value,
                expiry_millis,
            },
        );
    }

    /// 按 `(res, tier)` 物化会话凭据（会话来源的 `credential_for` 内核）：
    /// 命中且 `now < expiry − skew` → 复用缓存（不重登，L-8）；缺失/临近过期 → 单飞续会话
    /// （L-7/L-9），成功回填后物化为不透明 `ResourceCredential`；续会话失败 → fail-closed
    /// `Err`（L-10）。本方法是 async（续会话 await 认证端点）。
    pub async fn credential_for(
        &self,
        res: &ResourceCode,
        tier: &CredentialTier,
    ) -> Result<ResourceCredential, CredentialError> {
        let key = (res.clone(), tier.clone());

        // 单飞下，follower 让出后须**复查**缓存（leader 已回填则复用），故整体是一个有界
        // 复查循环：每轮先查缓存，命中可复用即返回；否则尝试成为 leader 续会话，已有 leader
        // 在途则作 follower 让出一次再复查。`MAX_SINGLE_FLIGHT_YIELDS` 封顶防无界自旋。
        let mut yields: u32 = 0;
        loop {
            let now = self.clock.now_millis();

            // 命中复用判定（不跨 `await` 持表锁，§3.1；命中即在持锁期间就地物化产物）。
            // - ②`ApiToken`（长效 token，详细设计 6.13 / F-7）：命中即**直接取用**，不按
            //   skew 触发续会话——②档无运行期续期机制，这是与①/③ skew 续期判定相异的解析路径。
            // - ①`PasswordSession` / ③`OAuthRefresh`：`now < expiry − skew` 才复用，否则临近过期
            //   触发续会话（L-8 复用 / L-7 续期）。
            {
                let cache = self
                    .cache
                    .lock()
                    .map_err(|_| CredentialError::VaultUnavailable)?;
                if let Some(entry) = cache.get(&key) {
                    let reuse = match self.form {
                        SessionForm::ApiToken => true,
                        SessionForm::PasswordSession | SessionForm::OAuthRefresh => {
                            now < entry.expiry_millis.saturating_sub(self.skew_millis)
                        }
                    };
                    if reuse {
                        return Ok(materialize_session(&entry.session_value));
                    }
                }
            }

            // 缺失 / 临近过期 → 需续会话。单飞门（L-9）：登记本键为在途。`insert` 返回
            // `true` 即本任务抢到 leader；返回 `false` 表已有在途 leader、本任务作 follower。
            let is_leader = {
                let mut in_flight = self
                    .in_flight
                    .lock()
                    .map_err(|_| CredentialError::VaultUnavailable)?;
                in_flight.insert(key.clone())
            };

            if !is_leader {
                // follower：**绝不**发起 renew（杜绝登录风暴）。让出一次执行权，待 leader
                // 推进续会话、回填缓存后复查复用其在途产物；有界让出，超限 fail-closed。
                if yields >= MAX_SINGLE_FLIGHT_YIELDS {
                    return Err(CredentialError::VaultUnavailable);
                }
                yields += 1;
                YieldOnce { yielded: false }.await;
                continue;
            }

            // leader：续会话。本 crate 不拥有 runtime、不 spawn；续会话在调用方任务上下文
            // `await`，其网络等待**不持任何锁**（§3.1）。无论成败都撤销在途标记（让 follower
            // 得以复查或自身转 leader）。续会话失败 → fail-closed `Err`（L-10），绝不数据面静默重试。
            let result = self.authority.renew(res, tier, self.form).await;
            {
                let mut in_flight = self
                    .in_flight
                    .lock()
                    .map_err(|_| CredentialError::VaultUnavailable)?;
                in_flight.remove(&key);
            }
            let renewed = result?;

            // 成功 → 回填缓存（L-7），后续解析复用回填会话不再重登。物化在持锁期间完成，
            // 续期产物明文不复制到额外位置。
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| CredentialError::VaultUnavailable)?;
            let entry = cache.entry(key).or_insert_with(|| SessionEntry {
                session_value: Zeroizing::new(String::new()),
                expiry_millis: 0,
            });
            entry.session_value = renewed.session_value;
            entry.expiry_millis = renewed.expiry_millis;
            return Ok(materialize_session(&entry.session_value));
        }
    }
}
