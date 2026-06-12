//! 求值器（Evaluator）：纯逻辑求值入口（模块设计 5.3；详细设计 8.3、4.1）。
//!
//! 本文件实现 5.3/4.1 的对外形状——`Evaluator`（持有 `Authenticator` 与
//! `ConditionPredicate` 两个按 `kind` 选型的同步插件注册表）、`ConstraintCheck`
//! （kernel 先行调用 `Adapter::check_constraint` 物化的步骤[4]结果），以及纯函数
//! 入口 `evaluate(req, ci, constraint, policy, now) -> (Decision, EvalTrace)`，
//! 签名逐字对齐 §5.3 / 详细设计 4.1。
//!
//! `evaluate` 编排短路串行管线 `[1]`(认证)→`[3]`(RBAC 展开查表)→`[4]`(读
//! `constraint.passed`)→`[5]`(条件谓词逐一求值)→`[6]`(动作分流 + 动词→tier
//! 映射)：任一步判定拒绝即就地短路，轨迹截止于当前步、以 `Deny` 返回。纯同步、
//! 确定性、零 IO——只调同步插件方法（`authenticate` / `eval`），绝不触达
//! async 的 `open` / `execute` / `credential_for` / `discover`，亦不读系统时钟
//! （只用入参 `now`）、不用随机。一切错误路径 fail-closed 为 `Decision::Deny`
//! （公理二）；错误处理全程显式 `match`、绝不吞错（契约
//! `EVAL_NO_ERROR_SWALLOWING`）。

use std::collections::BTreeMap;

use crate::decision::{Decision, EvalTrace};
use crate::domain::{
    Capability, EvalContext, GrantAction, GrantCell, MatchedGrant, Mode, PolicySnapshot,
    PrincipalId, ResourceCode, TierDecl, Timestamp,
};
use crate::error::Stage;
use crate::eval::deny::assemble;
use crate::eval::trace::TraceBuilder;
use crate::id::SnowflakeId;
use crate::plugin::{Authenticator, ConditionPredicate};
use crate::request::{ClassifiedIntent, NormalizedRequest};

/// 步骤[4]细则校验的结果，由 kernel 先行调用 `Adapter::check_constraint`
/// 得出后传入（CONS-8）；`passed == false` 时 `evaluate` 据此 deny。
pub struct ConstraintCheck {
    /// 细则是否通过；`false` → 步骤[4] deny。
    pub passed: bool,
}

/// 纯逻辑求值器：持有按 `kind` 选型的同步插件注册表，自身不持有可变状态，
/// `evaluate` 因 `now` 显式入参而保持确定性（相同输入 → 相同决策）。注册表
/// 装配后只读，求值期不改（`&self` 不可变）。
pub struct Evaluator {
    /// 步骤[1]认证器注册表（`local_process` / `api_key` / `token` ...），
    /// 按 `Authenticator::kind` 选型。
    authenticators: BTreeMap<&'static str, Box<dyn Authenticator>>,
    /// 步骤[5]条件谓词注册表（`rate_limit` / `time_window` / `mode` /
    /// `ttl` ...），按 `ConditionPredicate::kind` 选型。
    predicates: BTreeMap<&'static str, Box<dyn ConditionPredicate>>,
}

impl Evaluator {
    /// 由注册好的同步插件注册表组装求值器。装配后注册表只读。
    pub fn new(
        authenticators: BTreeMap<&'static str, Box<dyn Authenticator>>,
        predicates: BTreeMap<&'static str, Box<dyn ConditionPredicate>>,
    ) -> Self {
        Self {
            authenticators,
            predicates,
        }
    }

    /// 输入：归一化请求 + 适配器归类结果 + 细则校验结果 + 策略快照 + 墙钟；
    /// 输出：三值决策 + 完整求值轨迹。
    ///
    /// 短路串行管线 `[1]→[3]→[4]→[5]→[6]`：任一步判定拒绝即就地短路，轨迹
    /// 截止于当前步，以 `Deny` 返回（公理一/二）。一切 `Err`/无法判定一律解析
    /// 为 `Deny`（fail-closed），错误处理全程显式 `match`、绝不吞错。
    pub fn evaluate(
        &self,
        req: &NormalizedRequest,
        ci: &ClassifiedIntent,
        constraint: &ConstraintCheck,
        policy: &PolicySnapshot,
        now: Timestamp,
    ) -> (Decision, EvalTrace) {
        let mut trace = TraceBuilder::new();

        // [1] 认证：按出示物 kind 选认证器；未注册（无法判定来源）即拒。
        let principal = match self.authenticate(req, policy, now) {
            Ok(p) => p,
            Err(detail) => {
                trace.push(Stage::Auth, detail);
                // 认证未通过：无已认证 principal，用零 principal（grants 取不到 →
                // 空 your_grants，fail-closed，不泄露存在性）。
                return deny(trace, policy, unauthenticated(), req, ci);
            }
        };
        trace.push(Stage::Auth, "principal authenticated");

        // [3] RBAC：以 principal 在 grants 查 (resource, capability) 格；
        //     无命中 → Deny（公理一：缺格即拒）。detail 援引资源代号 + 动词
        //     （策略事实，代号非真实地址）。
        let cell = match self.grant_cell(policy, &principal, &req.resource, ci.capability) {
            Some(c) => c,
            None => {
                trace.push(
                    Stage::Rbac,
                    format!(
                        "no grant cell for resource '{}' capability '{}'",
                        req.resource.as_str(),
                        ci.capability.as_str()
                    ),
                );
                return deny(trace, policy, principal, req, ci);
            }
        };
        trace.push(
            Stage::Rbac,
            format!(
                "grant cell matched for resource '{}' capability '{}'",
                req.resource.as_str(),
                ci.capability.as_str()
            ),
        );

        // [4] 细则：kernel 物化结果，passed == false → Deny。
        if !constraint.passed {
            trace.push(Stage::Constraint, "constraint check did not pass");
            return deny(trace, policy, principal, req, ci);
        }
        trace.push(Stage::Constraint, "constraint check passed");

        // [5] 条件谓词：逐一求值；未注册 / 解析失败 / Ok(false) / Err 均 → Deny。
        let ctx = EvalContext {
            principal,
            resource: req.resource.clone(),
            capability: ci.capability,
            objects: ci.objects.clone(),
            now,
            // mode 取自快照事实；快照不携带模式覆盖时即 Normal（确定性）。
            mode: Mode::Normal,
        };
        for condition in &cell.conditions {
            // detail 援引谓词 kind（策略事实，代号类内容），绝不援引 spec 原文。
            match self.eval_condition(&ctx, condition) {
                Ok(true) => {}
                Ok(false) => {
                    trace.push(
                        Stage::Condition,
                        format!("condition predicate '{}' not satisfied", condition.kind),
                    );
                    return deny(trace, policy, principal, req, ci);
                }
                Err(reason) => {
                    trace.push(
                        Stage::Condition,
                        format!("condition predicate '{}' {}", condition.kind, reason),
                    );
                    return deny(trace, policy, principal, req, ci);
                }
            }
        }
        trace.push(Stage::Condition, "conditions satisfied");

        // [6] 动作分流。
        match cell.action {
            // escalate：审批关闭恒折叠为 Deny；core 不挂起、不引入等待态。
            GrantAction::Escalate => {
                trace.push(Stage::Tier, "escalate cell folds to deny (approval closed)");
                deny(trace, policy, principal, req, ci)
            }
            // allow → tier 选择：找承载该动词的 TierDecl；无任一承载 → Deny(tier)。
            GrantAction::Allow => match self.select_tier(policy, &req.resource, ci.capability) {
                Some(tier_decl) => {
                    let grant = MatchedGrant {
                        resource: cell.resource.clone(),
                        capability: cell.capability,
                        role: cell.role.clone(),
                    };
                    let tier = tier_decl.tier.clone();
                    trace.push(Stage::Tier, "tier carrying the verb selected");
                    (Decision::Allow { grant, tier }, trace.finish())
                }
                None => {
                    trace.push(Stage::Tier, "no tier carries the verb (no default tier)");
                    deny(trace, policy, principal, req, ci)
                }
            },
        }
    }

    /// [1] 选认证器并认证。未注册 kind = 无法判定来源 = 拒；`authenticate` 的
    /// `Err` 同样 fail-closed 为拒。返回轨迹 detail（拒）或 principal（过）。
    fn authenticate(
        &self,
        req: &NormalizedRequest,
        policy: &PolicySnapshot,
        now: Timestamp,
    ) -> Result<PrincipalId, &'static str> {
        match self.authenticators.get(req.presented.kind()) {
            None => Err("no authenticator registered for presented kind"),
            Some(authenticator) => {
                match authenticator.authenticate(
                    &req.presented,
                    &req.origin,
                    &policy.credentials,
                    now,
                ) {
                    Ok(principal) => Ok(principal),
                    Err(_) => Err("authentication failed"),
                }
            }
        }
    }

    /// [3] 以 principal 在 grants 查命中格。缺 principal 子表或缺格 → `None`。
    fn grant_cell<'p>(
        &self,
        policy: &'p PolicySnapshot,
        principal: &PrincipalId,
        resource: &ResourceCode,
        capability: Capability,
    ) -> Option<&'p GrantCell> {
        match policy.grants.get(principal) {
            Some(per_principal) => per_principal.get(&(resource.clone(), capability)),
            None => None,
        }
    }

    /// [5] 选谓词、解析 spec、求值。未注册 / spec 解析失败均 fail-closed 为拒
    /// （返回 `Err(detail)`，绝不静默放行）。
    fn eval_condition(
        &self,
        ctx: &EvalContext,
        condition: &crate::domain::ConditionSpec,
    ) -> Result<bool, &'static str> {
        let predicate = match self.predicates.get(condition.kind.as_str()) {
            Some(p) => p,
            None => return Err("no predicate registered for condition kind"),
        };
        let spec: serde_json::Value = match serde_json::from_str(&condition.spec) {
            Ok(value) => value,
            Err(_) => return Err("condition spec is not valid json"),
        };
        match predicate.eval(ctx, &spec) {
            Ok(verdict) => Ok(verdict),
            Err(_) => Err("condition predicate undecidable"),
        }
    }

    /// [6] tier 选择：在 `policy.tiers[resource]` 找承载该动词的 `TierDecl`。
    /// resource 无声明或无任一承载 → `None`（不退默认 tier）。
    fn select_tier<'p>(
        &self,
        policy: &'p PolicySnapshot,
        resource: &ResourceCode,
        capability: Capability,
    ) -> Option<&'p TierDecl> {
        match policy.tiers.get(resource) {
            Some(decls) => decls.iter().find(|decl| decl.carries.contains(&capability)),
            None => None,
        }
    }
}

/// 未认证占位 principal：以零雪花构造，恒不出现在任何快照 `grants` 中，故
/// 认证阶段拒时 `your_grants` 退化为空集合（fail-closed，不泄露存在性）。
fn unauthenticated() -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(0))
}

/// 拒绝路径统一出口：消费轨迹累积器，据截止步与请求事实组装 `DenyResponse`，
/// 封为 `Decision::Deny`。`reason` 机械取自截止 `Stage`，绝不臆造话术。
fn deny(
    trace: TraceBuilder,
    policy: &PolicySnapshot,
    principal: PrincipalId,
    req: &NormalizedRequest,
    ci: &ClassifiedIntent,
) -> (Decision, EvalTrace) {
    let trace = trace.finish();
    let reason = match trace.final_stage() {
        Some(stage) => format!("denied at {}", stage.as_str()),
        None => "denied".to_string(),
    };
    let response = assemble(
        policy,
        &principal,
        &req.resource,
        ci.capability,
        &ci.objects,
        reason,
    );
    (Decision::Deny(response), trace)
}
