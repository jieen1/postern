/**
 * Client-side STATIC catalog for the nine red-team probes (04-verify.md §3.1).
 *
 * The wire `VerifyReport` is deliberately minimal: per item only `name` /
 * `pass` / `gap_note`. This catalog translates the probe `name` into human
 * prose (what the probe does, where the defense is expected to bite, the PASS
 * criterion) and supplies the StageChip target(s).
 *
 * HARD RULE (04-verify.md §3.2 / §7 原则二): this catalog is PURELY
 * descriptive. It NEVER participates in the verdict and NEVER overrides the
 * backend. Authority is always the backend `all_pass` / `pass` / `gap_note`.
 * `stages` here are Stage闭集 values (or null for composite/anonymization
 * probes whose criterion is "no secret echo", not a single pipeline stage).
 */

import type { Stage } from '../../api/types';

export interface ProbeDescriptor {
  /** Ordinal (1..9) — render order matches the backend `items` order. */
  ordinal: number;
  /** Human label. */
  label: string;
  /** What the probe sends / does. */
  intent: string;
  /**
   * Expected defense-line stage(s). Closed `Stage` values render a StageChip;
   * composite probes (⑤/⑥/⑨) use `null` + a `compositeNote` instead of
   * fabricating a single stage.
   */
  stages: Stage[];
  /** For composite defense points (⑤/⑥/⑨), the honest "not a single stage" note. */
  compositeNote?: string;
  /** PASS criterion in plain prose. */
  passCriterion: string;
}

/** Authoritative probe order = the nine fixed names, in `probe_set` order. */
export const PROBE_ORDER = [
  'scope_out_mutate',
  'disguised_write',
  'session_tamper',
  'multi_statement',
  'default_deny_unknown_resource',
  'credential_zero_touch',
  'origin_not_trusted',
  'untrusted_origin_auth_stage',
  'redaction_probe',
] as const;

export type ProbeName = (typeof PROBE_ORDER)[number];

export const PROBE_CATALOG: Record<ProbeName, ProbeDescriptor> = {
  scope_out_mutate: {
    ordinal: 1,
    label: '越权写（Scope 外 mutate）',
    intent: '以低权 Principal 对其 Scope 外资源发起 mutate 写请求。',
    stages: ['rbac'],
    passCriterion: '应在 rbac 阶因授权矩阵缺格被拒。',
  },
  disguised_write: {
    ordinal: 2,
    label: '伪装写（只读外壳包写删）',
    intent: '把写/删语义伪装成只读外壳，试图穿透归类。',
    stages: ['rbac'],
    passCriterion: '应穿透归类为 Destroy/写并在 rbac 阶被拒。',
  },
  session_tamper: {
    ordinal: 3,
    label: '会话语义篡改',
    intent: '篡改会话语义以越过归类。',
    stages: ['classify'],
    passCriterion: '应在 classify 阶被拒。',
  },
  multi_statement: {
    ordinal: 4,
    label: '多语句注入',
    intent: '在单请求内塞入多条语句试图夹带写操作。',
    stages: ['classify'],
    passCriterion: '应在 classify 阶被拒。',
  },
  default_deny_unknown_resource: {
    ordinal: 5,
    label: '默认拒绝（不存在资源）',
    intent: '对一个不存在的资源代号发起请求，探测默认拒绝与存在性泄露。',
    stages: ['rbac'],
    compositeNote: 'rbac（缺格）且不泄露存在性',
    passCriterion: '应在 rbac 阶因缺格被拒，且 deny 响应不泄露该资源存在性（your_grants 不含被探测代号、request_hint 为空）。',
  },
  credential_zero_touch: {
    ordinal: 6,
    label: '凭据零接触',
    intent: '探测 deny 响应是否回显任何凭据材料。',
    stages: [],
    compositeNote: '凭据零接触（deny 响应无机密回显）',
    passCriterion: 'deny 响应中不含任何凭据明文 / secret_hash / 真实地址。',
  },
  origin_not_trusted: {
    ordinal: 7,
    label: 'ConnOrigin 自报不被采信',
    intent: '请求自报一个可信来源，试图据此放行。',
    stages: ['auth'],
    passCriterion: '自报来源不被采信，应在 auth 阶被拒。',
  },
  untrusted_origin_auth_stage: {
    ordinal: 8,
    label: '错误来源 auth 阶拒',
    intent: '从一个不受信来源发起请求。',
    stages: ['auth'],
    passCriterion: '应在 auth 阶被拒。',
  },
  redaction_probe: {
    ordinal: 9,
    label: '脱敏探测（放行无回显）',
    intent: '发起一个会被放行的请求，探测出口脱敏是否生效。',
    stages: [],
    compositeNote: '上游匿名化 + 脱敏出口（放行但响应无敏感回显）',
    passCriterion: '请求放行，但响应经脱敏出口后不含真实地址 / 敏感明文。',
  },
};

/** Lookup by name; unknown names fall through to `undefined` (fail-closed). */
export function describeProbe(name: string): ProbeDescriptor | undefined {
  return (PROBE_CATALOG as Record<string, ProbeDescriptor>)[name];
}
