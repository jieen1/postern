//! `StaticVaultProvider`：静态来源凭据物化（设计承诺级桩，函数体 `todo!()`）。
//!
//! 职责（§3 凭据解析 / §5.3 / 详细设计 4.3、6.13 静态来源）：实现 core 定义的
//! `#[async_trait] CredentialProvider`。**静态来源**——数据库密码、redis ACL 账号、
//! API token：解锁后从 payload `secrets` 段**纯查表物化**出不透明 `ResourceCredential`，
//! 无运行期状态、零 IO、常数级查表（与 mapping::resolve 同构）。
//!
//! 物化规则（§3「静态来源是纯查表物化」、详细设计 4.3 的 `secrets` 字段形态）：
//! 按 `(res, tier)` 拼 `vault://<code>/<tier>` 引用键（payload `secrets` 段的键即此
//! 形态），取出该键下的字段映射，就地物化为 `ResourceCredential`——本 crate 写其结构体
//! 字面量即唯一构造点（§5.1 / §8 F-5、契约 `SEC_CONSTRUCTION_SITES`）。字段值取自
//! `Zeroizing<String>`，物化时不复制明文到额外位置。
//!
//! 机密类型纪律（§7-1、契约 `SEC_SECRET_TYPE_DISCIPLINE`）：不 derive / 手写
//! `ResourceCredential` 的 `Clone` / `Serialize`，`Debug` 由 core 恒输出 `REDACTED`，
//! 本 crate 不另加任何 impl。
//!
//! fail-closed（§5.3 / §6.3 / §8 F-3 / L-5 / L-11）：无匹配 `(res, tier)` →
//! `Err(CredentialError::NotFound)`、不返回缺省凭据；句柄不可用 → `VaultUnavailable`。
//! 签名层即无"缺省凭据"返回路径。错误侧只返回 core 常量英文错误码、绝不内插
//! code / tier / 账号明文（§8 L-11、红线 7.2-1）。
//!
//! async-trait 依赖冲突（设计承诺 vs 依赖白名单的真实冲突，见 notes 上报）：
//! `CredentialProvider` 在 core 标 `#[async_trait]`，但 `async-trait` proc-macro
//! **不在 secrets 依赖白名单、core 亦未 re-export**。本桩**不擅自引入** `async-trait`，
//! 改以**手工 desugar** 的 `Pin<Box<dyn Future>>` 返回签名实现该 trait（与
//! `#[async_trait]` 宏展开后的形态等价，无需该 proc-macro 即可满足 trait 约束）。
//! 是否将 `async-trait` 纳入白名单由 wrap 裁决，本单元不预设。

use std::future::Future;
use std::pin::Pin;

use postern_core::domain::{CredentialTier, ResourceCode, ResourceCredential};
use postern_core::error::CredentialError;
use postern_core::plugin::CredentialProvider;

use crate::vault::UnlockedVault;

/// 静态保险箱凭据来源：借用已解锁保险箱句柄，按 `(res, tier)` 从 `secrets` 段纯查表
/// 物化凭据。无运行期状态——所有状态都在被借用的 `UnlockedVault` 里。
pub struct StaticVaultProvider<'v> {
    /// 已解锁保险箱句柄（持有 payload `secrets` 段）。句柄存在即"保险箱可用"前提。
    vault: &'v UnlockedVault,
}

impl<'v> StaticVaultProvider<'v> {
    /// 绑定到一个已解锁保险箱句柄。构造本身无 IO、无状态。
    pub fn new(vault: &'v UnlockedVault) -> Self {
        Self { vault }
    }

    /// 按 `(res, tier)` 拼 `secrets` 段引用键 `vault://<code>/<tier>`，纯查表物化为
    /// 不透明 `ResourceCredential`（本 crate 唯一构造点）。
    ///
    /// 这是同步内核（静态来源无 IO）；trait 侧 `credential_for` 把它包成已就绪
    /// future 返回。fail-closed：无匹配键 → `NotFound`、无产物。
    fn materialize(
        &self,
        res: &ResourceCode,
        tier: &CredentialTier,
    ) -> Result<ResourceCredential, CredentialError> {
        // 引用键 `vault://<code>/<tier>`——payload `secrets` 段的键即此形态。
        let mut secret_ref =
            String::with_capacity("vault://".len() + res.as_str().len() + 1 + tier.as_str().len());
        secret_ref.push_str("vault://");
        secret_ref.push_str(res.as_str());
        secret_ref.push('/');
        secret_ref.push_str(tier.as_str());

        // 纯查表，零 IO、无运行期状态。无匹配键 → fail-closed `NotFound`，无产物。
        let fields = self
            .vault
            .payload()
            .secrets
            .get(&secret_ref)
            .ok_or(CredentialError::NotFound)?;

        // 物化：字段按 `BTreeMap` 序拼 `field=value`、以 `;` 连接（静态来源唯一确定
        // 形态）。明文取自 `Zeroizing<String>`，仅在最终物化进 `ResourceCredential`
        // （本 crate 唯一构造点）这一步落入材料串，不另设非 `Zeroizing` 中间副本。
        let mut material = String::new();
        let mut first = true;
        for (field, value) in fields {
            if !first {
                material.push(';');
            }
            first = false;
            material.push_str(field);
            material.push('=');
            material.push_str(value);
        }

        // 唯一构造点（契约 SEC_CONSTRUCTION_SITES）：本 crate 写结构体字面量。
        Ok(ResourceCredential // sole construction point in this crate
        {
            material,
        })
    }
}

/// `CredentialProvider` 实现——**手工 desugar** 的 `async_trait` 形态（见模块头注释的
/// 依赖冲突说明）。返回签名与 `#[async_trait]` 宏展开后等价：
/// `Pin<Box<dyn Future<Output = Result<ResourceCredential, CredentialError>> + Send + 'a>>`。
impl<'v> CredentialProvider for StaticVaultProvider<'v> {
    fn credential_for<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        res: &'life1 ResourceCode,
        tier: &'life2 CredentialTier,
    ) -> Pin<
        Box<dyn Future<Output = Result<ResourceCredential, CredentialError>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        // 静态来源无 IO：把同步物化内核包成已就绪 future。
        Box::pin(async move { self.materialize(res, tier) })
    }
}
