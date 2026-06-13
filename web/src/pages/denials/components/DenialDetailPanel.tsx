import { useState } from 'react';
import { Check, Copy, ExternalLink } from 'lucide-react';
import { Link } from 'react-router-dom';
import { SnowflakeId, StageChip } from '../../../components';
import type { DenialSummaryRow } from '../../../api/types';
import { DASH, elevateTemplate } from '../lib';

/**
 * DenialDetailPanel (展开细节) — read-only facts for one aggregation group:
 *  - the deny `stage` is shown as the落点 (the closed-set StageChip vocabulary);
 *    the per-sample verbatim `reason` is NOT in the summary wire type, so the
 *    panel points the human to Audit for the流水 rather than fabricating it,
 *  - `intent_digest` shown in full (a DESENSITIZED sample digest, not the
 *    request payload) with a copy button; absent → "—" placeholder (不臆造),
 *  - `policy_rev` shown as a single value (the endpoint carries one rev per
 *    group; a cross-sample range would need a backend field that doesn't exist
 *    yet — fail-closed, no fabricated range),
 *  - jump entries route the human to the right RULE editor (this page never
 *    writes), and a mechanical `postern elevate …` template (copy, not a button).
 *
 * No FormDrawer — this page has no write operations.
 */
export function DenialDetailPanel({ row }: { row: DenialSummaryRow }) {
  const principal = row.principal ?? row.principal_id ?? '';
  const auditHref =
    `/audit?principal=${encodeURIComponent(principal)}&decision=deny`;

  return (
    <div
      role="region"
      aria-label="聚合组细节"
      className="border-t border-border bg-surface-2 px-4 py-3 text-xs"
    >
      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5">
        <Term>拒绝落点</Term>
        <Def>
          <span className="inline-flex items-center gap-2">
            <StageChip stage={row.stage} />
            <span className="text-text-muted">
              在 {row.stage} 阶段被拒（逐条 reason 流水见 Audit）
            </span>
          </span>
        </Def>

        <Term>代表样本</Term>
        <Def>
          {row.intent_digest ? (
            <span className="inline-flex items-center gap-2">
              <span className="font-mono text-text">{row.intent_digest}</span>
              <CopyButton value={row.intent_digest} label="复制摘要" />
              <span className="text-text-muted">（脱敏摘要，非请求原文）</span>
            </span>
          ) : (
            <span className="text-text-muted">{DASH}</span>
          )}
        </Def>

        <Term>policy_rev</Term>
        <Def>
          <span className="font-mono text-text">{row.policy_rev || DASH}</span>
          <span className="ml-2 text-text-muted">
            （样本所属策略版本；其间策略可能已变更）
          </span>
        </Def>

        {row.principal_id && (
          <>
            <Term>principal_id</Term>
            <Def>
              <SnowflakeId id={row.principal_id} />
            </Def>
          </>
        )}
      </dl>

      <div className="mt-3 border-t border-border pt-3">
        <div className="mb-2 text-text-muted">
          跳转裁决（人决定，本页不写）：
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <JumpLink
            to={`/grants?principal=${encodeURIComponent(principal)}&resource=${encodeURIComponent(row.resource)}`}
          >
            查看 {row.principal ?? DASH} 在 Grants 矩阵的这一格
          </JumpLink>
          <JumpLink to={`/constraints?resource=${encodeURIComponent(row.resource)}`}>
            查看此资源的 Constraints
          </JumpLink>
          <JumpLink to={auditHref}>
            查 {row.principal ?? DASH} 全部 deny 流水（Audit）
          </JumpLink>
        </div>

        <div className="mt-3 flex flex-wrap items-center gap-2">
          <span className="text-text-muted">机械模板：</span>
          <code className="rounded-card border border-border bg-surface px-2 py-1 font-mono text-text">
            {elevateTemplate(row)}
          </code>
          <CopyButton value={elevateTemplate(row)} label="复制 elevate 模板" />
          <span className="text-text-muted">
            （需人填 TTL · 非放行按钮，放行须去 Grants 显式写入）
          </span>
        </div>
      </div>
    </div>
  );
}

function Term({ children }: { children: React.ReactNode }) {
  return <dt className="font-mono text-text-muted">{children}</dt>;
}
function Def({ children }: { children: React.ReactNode }) {
  return <dd className="min-w-0">{children}</dd>;
}

function JumpLink({ to, children }: { to: string; children: React.ReactNode }) {
  return (
    <Link
      to={to}
      className="inline-flex items-center gap-1 rounded-card border border-border px-2 py-1 text-info hover:bg-surface"
    >
      {children}
      <ExternalLink size={12} aria-hidden />
    </Link>
  );
}

function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard may be unavailable; the value is still visible inline.
    }
  }
  return (
    <button
      type="button"
      onClick={copy}
      aria-label={label}
      title={label}
      className="inline-flex items-center gap-1 text-text-muted hover:text-text"
    >
      {copied ? (
        <Check size={12} className="text-allow" aria-hidden />
      ) : (
        <Copy size={12} aria-hidden />
      )}
    </button>
  );
}
