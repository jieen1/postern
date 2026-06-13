/**
 * 主体/凭证页（10-principals-credentials.md）的表单契约与派生逻辑。
 *
 * 这里集中：
 *  - 新建主体 / 新建凭证的 Zod 校验 schema（与后端 `PrincipalRow` / `CredentialMeta`
 *    的字段约束对齐）；
 *  - 一个本地极简 RHF resolver（脚手架未装 `@hookform/resolvers`，故就地实现
 *    `Resolver = (values) => { values, errors }`，保留设计要求的 RHF+Zod 流程）；
 *  - 凭证派生状态机（生效 / 即将过期 / 已过期 / 已吊销，§3.2，非独立字段）；
 *  - 写操作提交前的摘要预览文案（§4，真话且只说事实）。
 *
 * 绝不在表单里出现 `secret_hash`/明文字段（基座原则六）：表单只接收元数据。
 */

import { z } from 'zod';
import type { FieldValues, Resolver } from 'react-hook-form';
import type {
  CredentialKind,
  CredentialRow,
  PrincipalKind,
} from '../../api/types';
import { formatTime, ttlRemainingMs } from '../../lib/format';

// ── 主体表单 ──────────────────────────────────────────────────────────────────

export const PRINCIPAL_KINDS: readonly PrincipalKind[] = [
  'agent',
  'program',
  'human',
] as const;

export const principalSchema = z.object({
  // 归一化校验：非空、去空白后仍有内容、长度受限、仅常见标识字符。
  name: z
    .string()
    .trim()
    .min(1, '请填写主体名')
    .max(128, '主体名过长')
    .regex(/^[A-Za-z0-9][A-Za-z0-9._-]*$/, '仅允许字母数字与 . _ -，且不以符号开头'),
  kind: z.enum(['agent', 'program', 'human']),
});

export type PrincipalFormValues = z.infer<typeof principalSchema>;

// ── 凭证表单 ──────────────────────────────────────────────────────────────────

export const CREDENTIAL_KINDS: readonly CredentialKind[] = [
  'local_process',
  'api_key',
  'token',
] as const;

/**
 * 有效期以"天"录入（可空＝长期有效）。token 录入既有令牌值（明文经控制面转发、
 * 本地不留存、提交后即清），其余 kind 不录入明文（api_key 由 daemon 生成、
 * local_process 为进程上下文凭证）。schema 不含 `secret_hash`。
 */
export const credentialSchema = z
  .object({
    kind: z.enum(['local_process', 'api_key', 'token']),
    trust_domain: z
      .string()
      .trim()
      .min(1, '请填写可信域')
      .max(128, '可信域过长'),
    // 空字符串＝长期有效；否则 1..3650 天。
    ttl_days: z
      .string()
      .trim()
      .refine(
        (v) => v === '' || (/^\d+$/.test(v) && Number(v) >= 1 && Number(v) <= 3650),
        '有效期需为 1..3650 的天数，留空＝长期有效',
      ),
    // 仅 token 录入；提交后即清，永不回显，永不落本地。
    secret: z.string().max(4096, '令牌值过长').optional().default(''),
  })
  .refine((v) => v.kind !== 'token' || v.secret.trim().length > 0, {
    path: ['secret'],
    message: 'token 凭证需录入既有令牌值',
  });

export type CredentialFormValues = z.infer<typeof credentialSchema>;

// ── 本地 zod resolver（无 @hookform/resolvers 依赖）────────────────────────────

/**
 * 把一个 zod schema 适配成 RHF 的 Resolver。RHF resolver 的契约就是
 * `(values) => { values, errors }`，故此处足够，无需引入额外依赖。
 */
export function zodResolver<S extends z.ZodTypeAny>(
  schema: S,
): Resolver<z.infer<S> & FieldValues> {
  return (values) => {
    const parsed = schema.safeParse(values);
    if (parsed.success) {
      return { values: parsed.data as z.infer<S> & FieldValues, errors: {} };
    }
    const errors: Record<string, { type: string; message: string }> = {};
    for (const issue of parsed.error.issues) {
      const key = issue.path.join('.') || 'root';
      // 仅保留每个字段的首条错误（RHF 默认行为）。
      if (!errors[key]) {
        errors[key] = { type: issue.code, message: issue.message };
      }
    }
    return {
      values: {},
      errors: errors as never,
    };
  };
}

// ── 凭证派生状态（§3.2）────────────────────────────────────────────────────────

export type CredentialStatus = 'active' | 'near_expiry' | 'expired' | 'revoked';

/** 即将过期阈值：临近 24h 转琥珀（与 TtlBadge 的近过期语义同向，本页放宽到 1 天）。*/
export const NEAR_EXPIRY_MS = 24 * 60 * 60_000;

/**
 * 由 `revoked_at`/`expires_at` 派生凭证状态（不是独立字段）。
 * 吊销是终态、优先级最高；其次按过期判定。fail-closed：过期即按受限呈现。
 */
export function deriveCredentialStatus(
  cred: Pick<CredentialRow, 'revoked_at' | 'expires_at'>,
  now: number = Date.now(),
): CredentialStatus {
  if (cred.revoked_at !== null) return 'revoked';
  const remaining = ttlRemainingMs(cred.expires_at, now);
  if (remaining === null) return 'active'; // 长期有效
  if (remaining <= 0) return 'expired';
  if (remaining <= NEAR_EXPIRY_MS) return 'near_expiry';
  return 'active';
}

export const STATUS_LABEL: Record<CredentialStatus, string> = {
  active: '生效',
  near_expiry: '即将过期',
  expired: '已过期',
  revoked: '已吊销',
};

// ── 主体凭证聚合（§3.1 凭证数：生效计数）─────────────────────────────────────

export interface CredentialTally {
  active: number;
  revoked: number;
  expired: number;
  /** 包含即将过期（仍生效）。 */
  near_expiry: number;
}

/** 按派生状态把一组凭证归类计数（即将过期单列，但仍计入"生效"概念之外的提示）。*/
export function tallyCredentials(
  creds: Array<Pick<CredentialRow, 'revoked_at' | 'expires_at'>>,
  now: number = Date.now(),
): CredentialTally {
  const tally: CredentialTally = { active: 0, revoked: 0, expired: 0, near_expiry: 0 };
  for (const c of creds) {
    const s = deriveCredentialStatus(c, now);
    if (s === 'revoked') tally.revoked += 1;
    else if (s === 'expired') tally.expired += 1;
    else if (s === 'near_expiry') {
      tally.near_expiry += 1;
      tally.active += 1; // 即将过期仍生效
    } else tally.active += 1;
  }
  return tally;
}

// ── 摘要预览文案（§4，事实陈述，不安抚、不建议）──────────────────────────────

export function principalCreateSummary(v: PrincipalFormValues): string {
  return `将登记主体 ${v.name}（kind=${v.kind}）。该主体初始无任何凭证、无任何授权——存在但默认拒绝一切（公理一）。`;
}

export function credentialCreateSummary(
  principalName: string,
  v: CredentialFormValues,
): string {
  const ttl = v.ttl_days.trim() === '' ? '长期有效' : `${v.ttl_days.trim()}d`;
  return `为主体 ${principalName} 新建 ${v.kind} 凭证，可信域=${v.trust_domain}，有效期=${ttl}。`;
}

export function revokeSummary(
  principalName: string,
  cred: Pick<CredentialRow, 'kind' | 'id'>,
): string {
  return `吊销主体 ${principalName} 的 ${cred.kind} 凭证（id ${cred.id}）。`;
}

export function deletePrincipalSummary(
  name: string,
  hasActiveCreds: boolean,
): string {
  const base = `将逻辑删除主体 ${name} 及其归属判定，不等于吊销其凭证——若需立即切断认证，请先吊销其凭证。`;
  return hasActiveCreds
    ? `${base} 该主体仍有生效凭证，请先吊销再删除（避免"已删主体仍可认证"的悖态）。`
    : base;
}

export function deleteCredentialSummary(
  principalName: string,
  cred: Pick<CredentialRow, 'kind' | 'id'>,
): string {
  return `从名册移除主体 ${principalName} 的 ${cred.kind} 凭证（id ${cred.id}）记录。删除是名册清理；若意图是立即停止认证，应使用"吊销"（热生效）。`;
}

/** 把过期时刻渲染为可读文字（长期有效用文字标注，不留空）。*/
export function expiryLabel(expiresAt: string | null): string {
  return expiresAt === null ? '长期有效' : formatTime(expiresAt);
}
