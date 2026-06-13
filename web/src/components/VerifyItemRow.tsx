import { CheckCircle2, XCircle } from 'lucide-react';
import type { VerifyItem } from '../api/types';
import { cn } from '../lib/cn';

/**
 * Red-team verify item row (设计系统 §4): PASS green / FAIL red, with the
 * FAIL `gap_note` shown verbatim to point at the breached defense line. The
 * probe `name` is the authoritative key; a human label is a client-side
 * catalog lookup (never authoritative).
 */
const PROBE_LABELS: Record<string, string> = {
  scope_out_mutate: '越权写（Scope 外 mutate）',
  disguised_write: '伪装写（只读外壳包写删）',
  session_tamper: '会话语义篡改',
  multi_statement: '多语句注入',
  default_deny_unknown_resource: '默认拒绝（不存在资源）',
  credential_zero_touch: '凭据零接触',
  origin_not_trusted: 'ConnOrigin 自报不被采信',
  untrusted_origin_auth_stage: '错误来源 auth 阶拒',
  redaction_probe: '脱敏探测（放行无回显）',
};

export function VerifyItemRow({ item }: { item: VerifyItem }) {
  return (
    <div
      className={cn(
        'flex items-start gap-3 rounded-card border px-3 py-2',
        item.pass ? 'border-allow/30' : 'border-deny/40 bg-deny/5',
      )}
    >
      <span className="mt-0.5">
        {item.pass ? (
          <CheckCircle2 size={16} className="text-allow" />
        ) : (
          <XCircle size={16} className="text-deny" />
        )}
      </span>
      <div className="flex flex-col gap-1">
        <div className="flex items-center gap-2">
          <span className={cn('text-sm', item.pass ? 'text-text' : 'text-deny')}>
            {item.pass ? 'PASS' : 'FAIL'}
          </span>
          <span className="font-mono text-xs text-text-muted">{item.name}</span>
          <span className="text-xs text-text-muted">
            {PROBE_LABELS[item.name] ?? ''}
          </span>
        </div>
        {item.gap_note && (
          <p className="text-xs text-deny">{item.gap_note}</p>
        )}
      </div>
    </div>
  );
}
