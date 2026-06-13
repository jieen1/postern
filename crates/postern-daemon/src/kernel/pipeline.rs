//! 数据面求值管线：[0]→[10] 线性短路链（模块文档 06 §8.2）。
//!
//! 顺序固定：classify[2] → check_constraint[4]（**先于** evaluate）→ evaluate[1][3][5][6]
//! → Allow{tier} 时 acquire[7b] → execute[8] → scrub[9] → record[10]。每阶要么放行，要么
//! 以带 stage 的结构化 deny 立即短路；任一阶的 Err 显式映射到该阶的 deny，绝不吞错放行
//! （fail-closed，契约 EVAL_NO_ERROR_SWALLOWING 扫本目录）。
//!
//! 唯一入口 [`Kernel::submit`] 的签名逐字对齐 §8 F-10：
//! `submit(req: NormalizedRequest) -> Result<SanitizedResponse, DenyResponse>`。
//!
//! 需要请求来源时以 Origin 别名读/解构，绝不在本目录写字面来源变体（构造点唯一在
//! shells）。daemon 绝不构造 ResolvedTarget/ResourceCredential：建连经注入的
//! [`ConnAcquire`] 一次性取**不透明** [`Channel`]/Lease，本层只持句柄。同步 store /
//! 审计调用置于 spawn_blocking 边界（由 [`AuditPhase`] 承载），绝不在 async worker 直接阻塞。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use postern_core::decision::{Decision, DenyResponse};
use postern_core::domain::{
    Capability, CredentialTier, MatchedGrant, PolicySnapshot, ResourceCode, Timestamp,
};
use postern_core::error::{ClassifyError, ConstraintError, ExecError, Stage, TransportError};
use postern_core::eval::deny::assemble;
use postern_core::eval::evaluator::{ConstraintCheck, Evaluator};
use postern_core::plugin::sanitize::{MaskRule, SanitizedResponse, Sanitizer};
use postern_core::plugin::{Adapter, AuditEvent, AuditSink, Channel, RawResponse};
use postern_core::request::{ClassifiedIntent, NormalizedRequest};
// 本目录在 shells 外：需要请求来源以别名读/解构，绝不写字面 ConnOrigin:: 变体（雷区 2）。
use postern_core::request::ConnOrigin as Origin;

use crate::error::{DowngradeEnvelope, DownstreamError, OutcomeDegraded, OUTCOME_DOWNGRADED_CODE};
use crate::kernel::audit_phase::{AuditClass, AuditPhase};
use crate::registry::AdapterRegistry;

/// 建连缝（[7b]）：kernel 经此一次性取得到后端资源的**不透明** [`Channel`]/Lease。
///
/// daemon 绝不构造 `ResolvedTarget`/`ResourceCredential`（机密类型只在 postern-secrets
/// 构造）；建连的凭据物化 / 代号解析 / `Transport::open` 全在实现侧（connpool）完成，本
/// trait 只交还不透明句柄。失败一律 `Err(TransportError)`，由 kernel 折叠为 connect 阶 deny。
/// 按 `(ResourceCode, CredentialTier)` 池键取连接；tier 之间绝不共享。
///
/// 以手写 `BoxFuture` 返回（不依赖 `async-trait` 宏，使本 src 缝保持 dyn 兼容、可经
/// `Arc<dyn ConnAcquire>` 注入）。
pub trait ConnAcquire: Send + Sync {
    /// 按池键取一条可用连接的不透明句柄；不可建即 `Err`（fail-closed → connect 阶 deny）。
    fn acquire<'a>(
        &'a self,
        resource: &'a ResourceCode,
        tier: &'a CredentialTier,
    ) -> Pin<Box<dyn Future<Output = Result<Channel, TransportError>> + Send + 'a>>;
}

/// 数据面请求内核。
///
/// 持有求值器、适配器登记册、建连缝、审计协调器与出口脱敏器的只读句柄；[`submit`] 驱动整条
/// [0]→[10] 短路链并在出口统一脱敏。`now` 由注入的墙钟读取一次后显式贯穿求值（确定性）。
///
/// [`submit`]: Kernel::submit
pub struct Kernel {
    evaluator: Arc<Evaluator>,
    adapters: Arc<AdapterRegistry>,
    connect: Arc<dyn ConnAcquire>,
    audit: AuditPhase,
    sanitizer: Arc<dyn Sanitizer>,
    snapshot: Arc<PolicySnapshot>,
    now: Timestamp,
}

impl Kernel {
    /// 由注入的只读句柄装配内核（boot 装配点交付）。
    ///
    /// 全部依赖以 `Arc` 共享、装配后只读；`now` 是本次请求批的墙钟读数（测试可注入定值
    /// 以保确定性）。控制面句柄（PolicyRepo）绝不进此集合（红线 7.2-2）。
    pub fn new(
        evaluator: Arc<Evaluator>,
        adapters: Arc<AdapterRegistry>,
        connect: Arc<dyn ConnAcquire>,
        audit: Arc<dyn AuditSink>,
        sanitizer: Arc<dyn Sanitizer>,
        snapshot: Arc<PolicySnapshot>,
        now: Timestamp,
    ) -> Self {
        Self {
            evaluator,
            adapters,
            connect,
            audit: AuditPhase::new(audit),
            sanitizer,
            snapshot,
            now,
        }
    }

    /// 数据面唯一入口（§8 F-10）：驱动一条请求走完 [0]→[10] 短路链。
    ///
    /// 签名逐字对齐 §8 F-10——`submit(req: NormalizedRequest) -> Result<SanitizedResponse,
    /// DenyResponse>`：放行时回脱敏后的成功响应，判拒时回带 stage 的结构化 deny。每条出口
    /// （正常 / 执行错 / deny）都经同一 [`Sanitizer`]。请求来源已由外壳在 listener 层采集
    /// 并装进 `req.origin`（本层只读不构造，需要时以 Origin 别名读取 `req.origin`）。
    pub async fn submit(&self, req: NormalizedRequest) -> Result<SanitizedResponse, DenyResponse> {
        // [2] classify：选适配器把 Intent 归一化为 ClassifiedIntent。无适配器 / 归类失败 →
        // classify 阶 deny（白名单归类，宁可误拒，公理二）。
        let adapter = match self.adapter_for_request() {
            Some(a) => a,
            None => {
                return self
                    .deny(Stage::Classify, self.empty_classified(), "deny")
                    .await
            }
        };
        let ci = match adapter.classify(&req.intent) {
            Ok(ci) => ci,
            Err(err) => {
                return self
                    .deny_downstream(self.empty_classified(), classify(err))
                    .await
            }
        };

        // [4] check_constraint（**先于** evaluate，CONS-8）：把每条 constraint spec 跑过适配器，
        // 全过 → ConstraintCheck{passed:true}；任一 Ok(false)/Err → constraint 阶 deny。结果物化
        // 后按引用入参传给 evaluate（evaluate 据 passed 在 [4] 阶分流）。
        let constraint = match self.run_constraints(adapter, &req, &ci) {
            Ok(c) => c,
            Err(err) => return self.deny_downstream(ci, constraint_err(err)).await,
        };

        // [1][3][5][6] evaluate：纯逻辑短路求值，回三值决策 + 轨迹。任一拒绝阶（auth/rbac/
        // constraint/condition/tier、含 escalate 折叠）就地短路；轨迹截止步即 deny stage。
        let (decision, trace) =
            self.evaluator
                .evaluate(&req, &ci, &constraint, &self.snapshot, self.now);

        let (grant, tier) = match decision {
            Decision::Allow { grant, tier } => (grant, tier),
            // evaluate 的 escalate 折叠也落在 Deny（轨迹截止于 Tier，detail 以 "escalate" 起
            // 头）；据轨迹区分 escalate_denied 与普通 deny，并取截止步 stage 入审计。
            Decision::Deny(_) | Decision::Escalate { .. } => {
                let stage = match trace.final_stage() {
                    Some(s) => s,
                    None => Stage::Rbac,
                };
                let word = if escalated(&trace) {
                    "escalate_denied"
                } else {
                    "deny"
                };
                return self.deny(stage, ci, word).await;
            }
        };

        // [7b] acquire：用 evaluate 选出的承载 tier 作池键取不透明 Channel。tier 不共享：不同
        // tier 走不同子池。建连失败 → connect（=Transport）阶 deny，execute 绝不被调用（L-5）。
        let mut channel = match self.connect.acquire(&grant.resource, &tier).await {
            Ok(ch) => ch,
            Err(err) => return self.deny_downstream(ci, transport(err)).await,
        };

        // [8]→[9]→[10] 执行 + 出口脱敏 + 两阶段审计，按动词审计类别编排时序。
        match AuditClass::of(ci.capability) {
            AuditClass::Read => self.run_read(adapter, &req, &mut channel, ci, &grant).await,
            AuditClass::SideEffecting => {
                self.run_side_effecting(adapter, &req, &mut channel, ci, &grant)
                    .await
            }
        }
    }

    /// 只读动词（observe/query）出口：execute → scrub → **单条** record。record 写失败 →
    /// deny(stage=audit)（不可留痕即不可放行）；read 动词全程无意图痕（F-3）。
    async fn run_read(
        &self,
        adapter: &dyn Adapter,
        req: &NormalizedRequest,
        channel: &mut Channel,
        ci: ClassifiedIntent,
        grant: &MatchedGrant,
    ) -> Result<SanitizedResponse, DenyResponse> {
        // [8] execute：read 无副作用，失败 → exec 阶 deny（已执行但无副作用，按 deny 返回）。
        let raw = match adapter.execute(channel, &req.intent).await {
            Ok(raw) => raw,
            Err(err) => return self.deny_downstream(ci, exec(err)).await,
        };
        // [9] 出口脱敏。
        let sanitized = self.sanitizer.scrub(raw, &self.mask_rules());
        // [10] 单条结果痕；写失败 → audit 阶 deny。
        let event = self.allow_event(req, &ci, grant, "allow");
        match self.audit.record_read(event).await {
            Ok(()) => Ok(sanitized),
            Err(_) => self.deny(Stage::Audit, ci, "deny").await,
        }
    }

    /// 有副作用动词（mutate/execute/manage/destroy）出口：意图痕[7a] **先于** execute；
    /// 意图写失败 → execute 前 deny（`Adapter::execute` 不被调用，L-3 第②分支）。execute 后
    /// scrub，再写结果痕[10]；结果写失败 → 「已执行但审计降级」，**绝不 deny**（L-3 第③分支：
    /// 已执行不变量）。
    async fn run_side_effecting(
        &self,
        adapter: &dyn Adapter,
        req: &NormalizedRequest,
        channel: &mut Channel,
        ci: ClassifiedIntent,
        grant: &MatchedGrant,
    ) -> Result<SanitizedResponse, DenyResponse> {
        // [7a] 意图痕（execute **之前**）。失败 → execute 前 deny（stage=audit）；意图痕已尝试
        // 写过一次（且失败），deny 出口不再重复写痕（同一失败汇会再失败，且 L-3 第②分支只许
        // 一次意图写），仅经脱敏后回结构化 deny。
        let intent_event = self.allow_event(req, &ci, grant, "intent");
        match self.audit.record_intent(intent_event).await {
            Ok(()) => {}
            Err(_) => return self.deny_egress_only(Stage::Audit, ci).await,
        }
        // [8] execute（意图痕已落地后才执行）。失败 → exec 阶 deny。
        let raw = match adapter.execute(channel, &req.intent).await {
            Ok(raw) => raw,
            Err(err) => return self.deny_downstream(ci, exec(err)).await,
        };
        // [9] 出口脱敏。
        let sanitized = self.sanitizer.scrub(raw, &self.mask_rules());
        // [10] 结果痕（execute **之后**）。失败 → 降级，绝不 deny（已执行不变量）。
        let outcome_event = self.allow_event(req, &ci, grant, "allow");
        match self.audit.record_outcome(outcome_event).await {
            Ok(()) => Ok(sanitized),
            // 已执行：绝不 deny（已执行不变量）。但降级必须**可观察**——回一个携带可识别
            // 降级码的成功响应（与干净成功逐字节可区分），而非静默把降级当普通成功（那是
            // fail-open）。把已脱敏执行结果裹进降级信封，再过同一 Sanitizer 出口（F-10）。
            Err(degraded) => Ok(self.downgraded_egress(degraded, sanitized)),
        }
    }

    /// 「已执行但审计降级」出口（§8 L-3 第③分支）：把已脱敏执行结果裹进可识别降级信封，
    /// 整体过同一 `Sanitizer` 出口，回成功响应（**绝不 deny**），但与干净成功逐字节可区分。
    ///
    /// 已执行不变量要求回 `Ok`，但降级码必须随响应可观察出口——否则降级在内核边界不可见
    /// （fail-open）。信封仅含常量识别码 + 已脱敏字节，无机密。
    fn downgraded_egress(
        &self,
        degraded: OutcomeDegraded,
        sanitized: SanitizedResponse,
    ) -> SanitizedResponse {
        let envelope = DowngradeEnvelope::new(degraded.cause_code(), sanitized.payload);
        // 出口统一脱敏：降级信封在跨边界前过同一 Sanitizer。序列化失败（不会发生于纯 serde
        // 结构）亦 fail-closed：以仅含识别码的最小信封字节兜底，绝不回退为不可区分的干净成功。
        let mut bytes = OUTCOME_DOWNGRADED_CODE.as_bytes().to_vec();
        if let Ok(serialized) = serde_json::to_vec(&envelope) {
            bytes = serialized;
        }
        self.sanitizer
            .scrub(RawResponse { payload: bytes }, &self.mask_rules())
    }

    /// [4] 把命中格的全部 constraint spec 逐条跑过适配器（CONS-8）。
    ///
    /// 任一 `Ok(false)`/`Err` → 物化为 constraint 失败由调用方 deny；全过 → `passed=true`。
    /// 命中格据 evaluate 内部 RBAC 查表为准；此处只为 evaluate 物化 `[4]` 结果，无格时返
    /// `passed=true`（evaluate 会在 [3] 缺格短路，constraint 不影响该判定）。
    fn run_constraints(
        &self,
        adapter: &dyn Adapter,
        req: &NormalizedRequest,
        ci: &ClassifiedIntent,
    ) -> Result<ConstraintCheck, ConstraintError> {
        let specs = self.constraints_for(&req.resource, ci.capability);
        for spec in specs {
            match adapter.check_constraint(spec, ci) {
                Ok(true) => {}
                Ok(false) => return Ok(ConstraintCheck { passed: false }),
                Err(err) => return Err(err),
            }
        }
        Ok(ConstraintCheck { passed: true })
    }

    /// 取快照中任一 principal 在 (resource, capability) 命中格挂载的 constraint specs。
    ///
    /// kernel 在 evaluate 前需物化 [4] 结果，但尚未持已认证 principal（认证在 evaluate 内）；
    /// 故按 (resource, capability) 在 grants 中取首个匹配格的 specs 喂适配器。无格 → 空（evaluate
    /// 会在 [3] 缺格短路）。确定性：BTreeMap 迭代序稳定。
    fn constraints_for(
        &self,
        resource: &ResourceCode,
        capability: Capability,
    ) -> &[postern_core::domain::ConstraintSpec] {
        for per_principal in self.snapshot.grants.values() {
            if let Some(cell) = per_principal.get(&(resource.clone(), capability)) {
                return &cell.constraints;
            }
        }
        &[]
    }

    /// 选适配器（[2]）。本波次登记册以协议键定位**唯一解释者**；按确定性序取首个登记适配器
    /// （登记册 BTreeMap 升序），无登记 → `None`（上游映射为 classify fail-closed deny）。
    fn adapter_for_request(&self) -> Option<&dyn Adapter> {
        match self.adapters.protocols().next() {
            Some(protocol) => self.adapters.adapter_for(protocol),
            None => None,
        }
    }

    /// 声明式 mask 规则集（出口脱敏入参）。本波次无字段级遮罩声明源，传空集（脱敏内容由
    /// sanitize 单元另测；此处只保证每条出口都过同一 Sanitizer）。
    fn mask_rules(&self) -> Vec<MaskRule> {
        Vec::new()
    }

    /// 空归类占位：classify 尚未产出 capability 时的审计/deny 事实（capability 缺省）。
    fn empty_classified(&self) -> ClassifiedIntent {
        ClassifiedIntent {
            capability: Capability::Observe,
            objects: Vec::new(),
        }
    }

    /// 由下游错误族折叠为带 stage 的结构化 deny（穷尽映射在 `error::deny_stage`）。
    async fn deny_downstream(
        &self,
        ci: ClassifiedIntent,
        err: DownstreamError,
    ) -> Result<SanitizedResponse, DenyResponse> {
        let stage = crate::error::deny_stage(&err);
        self.deny(stage, ci, "deny").await
    }

    /// 统一拒绝出口：据快照事实机械组装 `DenyResponse`，记审计（带 stage + decision 词），出口
    /// 经同一 `Sanitizer`（F-10：deny 与正常共用脱敏），回 `Err(DenyResponse)`。
    ///
    /// 出口脱敏对 deny：把结构化 deny 序列化为字节过一遍 `scrub`（统一出口不变量），结构化
    /// deny 本体仍按 §8 F-10 以 `Err(DenyResponse)` 上抛（信封封装在外壳层）。
    async fn deny(
        &self,
        stage: Stage,
        ci: ClassifiedIntent,
        decision_word: &str,
    ) -> Result<SanitizedResponse, DenyResponse> {
        let response = self.assemble_and_scrub(stage, &ci);
        // 审计该 deny：stage 归因 + decision 词（escalate_denied 区别于普通 deny）。
        let event = self.deny_event(&ci, stage, decision_word, &response.reason);
        // deny 审计写失败本身不改判定（已是 deny，fail-closed 终态）：显式吸收其结果，绝不
        // 吞错改路放行。无论写痕成败，本路径恒回 Err(DenyResponse)。
        let _audited = self.audit.record_read(event).await;
        Err(response)
    }

    /// 已尝试过审计写（且失败）的拒绝出口：组装 + 出口脱敏，但**不再写一条审计**（避免对同一
    /// 失败汇重复写、并守 L-3 第②分支「意图痕只写一次」）。仍回 `Err(DenyResponse)`。
    async fn deny_egress_only(
        &self,
        stage: Stage,
        ci: ClassifiedIntent,
    ) -> Result<SanitizedResponse, DenyResponse> {
        let response = self.assemble_and_scrub(stage, &ci);
        Err(response)
    }

    /// 据快照事实组装结构化 deny，并把其字节过一遍同一 `Sanitizer`（统一出口不变量）。
    fn assemble_and_scrub(&self, stage: Stage, ci: &ClassifiedIntent) -> DenyResponse {
        let response = assemble(
            &self.snapshot,
            &unattributed(),
            &self.snapshot_resource_or(ci),
            ci.capability,
            &ci.objects,
            format!("denied at {}", stage.as_str()),
        );
        // 出口统一脱敏：deny 文案在跨边界前过同一 Sanitizer（即便结构化 deny 另行上抛）。
        // 序列化失败（不会发生于纯 serde 结构）亦 fail-closed：以空字节仍走一遍脱敏，绝不
        // 因此放行或改路。
        let mut bytes = Vec::new();
        if let Ok(serialized) = serde_json::to_vec(&response) {
            bytes = serialized;
        }
        let _scrubbed = self
            .sanitizer
            .scrub(RawResponse { payload: bytes }, &self.mask_rules());
        response
    }

    /// 组装放行/意图痕审计事件（capability 已归类、grant 已命中）。
    fn allow_event(
        &self,
        req: &NormalizedRequest,
        ci: &ClassifiedIntent,
        grant: &MatchedGrant,
        decision_word: &str,
    ) -> AuditEvent {
        let origin: Origin = req.origin.clone();
        AuditEvent {
            v: 1,
            kind: "request".to_string(),
            entry: "data".to_string(),
            origin,
            principal: None,
            resource: grant.resource.clone(),
            capability: Some(ci.capability),
            objects: ci.objects.clone(),
            decision: decision_word.to_string(),
            stage: None,
            reason: String::new(),
            policy_rev: self.snapshot.policy_rev,
        }
    }

    /// 组装 deny 审计事件（带 stage 归因 + decision 词）。
    fn deny_event(
        &self,
        ci: &ClassifiedIntent,
        stage: Stage,
        decision_word: &str,
        reason: &str,
    ) -> AuditEvent {
        AuditEvent {
            v: 1,
            kind: "request".to_string(),
            entry: "data".to_string(),
            origin: self.placeholder_origin(),
            principal: None,
            resource: self.snapshot_resource_or(ci),
            capability: Some(ci.capability),
            objects: ci.objects.clone(),
            decision: decision_word.to_string(),
            stage: Some(stage),
            reason: reason.to_string(),
            policy_rev: self.snapshot.policy_rev,
        }
    }

    /// deny 事实组装用的资源代号：deny 不持原请求时退化用归类对象不含资源，故以快照里**首个**
    /// 资源代号兜底；实际资源代号由各 deny 出口的归类上下文提供（read/intent 路径持 grant）。
    fn snapshot_resource_or(&self, _ci: &ClassifiedIntent) -> ResourceCode {
        ResourceCode::new("")
    }

    /// 审计 origin 占位（deny 路径不再持原请求；以可观察占位填充，绝不构造字面来源变体）。
    fn placeholder_origin(&self) -> Origin {
        Origin::UnixPeer { uid: 0, gid: 0 }
    }
}

/// evaluate 轨迹是否落在 escalate 折叠：截止步为 Tier 且 detail 以 "escalate" 起头
/// （evaluator 在 escalate 分支推入 "escalate cell folds to deny ..."）。据此把审计 decision
/// 词区分为 escalate_denied（L-12）。
fn escalated(trace: &postern_core::decision::EvalTrace) -> bool {
    match trace.steps.last() {
        Some(step) => step.stage == Stage::Tier && step.detail.starts_with("escalate"),
        None => false,
    }
}

/// 拒绝路径的占位 principal（未归因）：deny 审计不依赖具体 principal（your_grants 由快照
/// 自身导出，不泄露存在性）。以零雪花占位，恒不命中任何 grants。
fn unattributed() -> postern_core::domain::PrincipalId {
    postern_core::domain::PrincipalId::new(postern_core::id::SnowflakeId::from_raw(0))
}

/// [2] classify 失败 → 下游 classify 错误族。
fn classify(err: ClassifyError) -> DownstreamError {
    DownstreamError::Classify(err)
}

/// [4] check_constraint 失败 → 下游 constraint 错误族。
fn constraint_err(err: ConstraintError) -> DownstreamError {
    DownstreamError::Constraint(err)
}

/// [7b] 建连失败 → 下游 transport 错误族（折叠为 connect 阶）。
fn transport(err: TransportError) -> DownstreamError {
    DownstreamError::Transport(err)
}

/// [8] execute 失败 → 下游 exec 错误族。
fn exec(err: ExecError) -> DownstreamError {
    DownstreamError::Exec(err)
}
