/**
 * Page-local UI parts for docs/08 §五 ("本页特有的小构成，用基座组件拼，非新令牌").
 * These are intentionally page-local until proven reusable — see the suggest_shared
 * note. None introduce a new design token; they compose existing ones.
 */

import type { ReactNode } from 'react';
import { Info } from 'lucide-react';
import { cn } from '../../lib/cn';
import { ResourceCodeBadge } from '../../components';
import type { Adapter } from '../../api/types';
import {
  kindsForAdapter,
  type ConstraintKind,
  type Segment,
} from './data';

// ── SegmentedControl ──────────────────────────────────────────────────────────

export interface SegmentDef {
  key: Segment;
  label: string;
}

export const SEGMENTS: readonly SegmentDef[] = [
  { key: 'constraints', label: '细则 Constraints' },
  { key: 'conditions', label: '条件 Conditions' },
  { key: 'deny-notes', label: '拒绝指引 Deny-notes' },
] as const;

export function SegmentedControl({
  value,
  onChange,
}: {
  value: Segment;
  onChange: (next: Segment) => void;
}) {
  return (
    <div
      role="tablist"
      aria-label="记录类型"
      className="inline-flex rounded-card border border-border bg-surface-2 p-1"
    >
      {SEGMENTS.map((seg) => {
        const active = seg.key === value;
        return (
          <button
            key={seg.key}
            type="button"
            role="tab"
            aria-selected={active}
            onClick={() => onChange(seg.key)}
            className={cn(
              'rounded-card px-3 py-1.5 text-sm',
              active
                ? 'bg-surface text-text shadow-card'
                : 'text-text-muted hover:text-text',
            )}
          >
            {seg.label}
          </button>
        );
      })}
    </div>
  );
}

// ── KindMatrixSelect (细则 kind, narrowed by the selected resource's adapter) ──

export function KindMatrixSelect({
  adapter,
  value,
  onChange,
  id,
}: {
  adapter: Adapter | undefined;
  value: string;
  onChange: (kind: string) => void;
  id?: string;
}) {
  const kinds = kindsForAdapter(adapter);
  return (
    <select
      id={id}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="w-full rounded-card border border-border bg-bg px-2 py-1 text-sm"
    >
      <option value="">选择 kind…</option>
      {kinds.map((k: ConstraintKind) => (
        <option key={k} value={k}>
          {k}
        </option>
      ))}
    </select>
  );
}

// ── IntersectionHint (同格同 kind 已有 N 条 → 交集生效，更窄；仅陈述，不预测) ──

export function IntersectionHint({ count }: { count: number }) {
  if (count <= 0) return null;
  return (
    <p
      data-testid="intersection-hint"
      className="flex items-start gap-1 rounded-card border border-info/40 bg-info/5 px-2 py-1 text-xs text-info"
    >
      <Info size={12} className="mt-0.5 shrink-0" />
      <span>
        该格已有 {count} 条同 kind 细则，新增后按<strong>交集</strong>生效（更窄，
        fail-closed）。
      </span>
    </p>
  );
}

// ── ScopeWidenHint (条件作用域留空 → 范围更广，强提示) ───────────────────────

export function ScopeWidenHint({ scope }: { scope: string }) {
  return (
    <p
      data-testid="scope-widen-hint"
      className="flex items-start gap-1 rounded-card border border-warn/40 bg-warn/5 px-2 py-1 text-xs text-warn"
    >
      <Info size={12} className="mt-0.5 shrink-0" />
      <span>作用域={scope}，范围更广，请确认。</span>
    </p>
  );
}

// ── VerbatimNote (deny-note 原文：等宽、不加工、不渲染 markdown；公理六) ──────

export function VerbatimNote({
  note,
  className,
}: {
  note: string;
  className?: string;
}) {
  return (
    <pre
      className={cn(
        'whitespace-pre-wrap break-words font-mono text-xs text-text',
        className,
      )}
    >
      {note}
    </pre>
  );
}

// ── JsonPreview (constraint/condition spec, raw JSON, read-only, mono) ─────────

export function JsonPreview({ spec }: { spec: string }) {
  let pretty = spec;
  try {
    pretty = JSON.stringify(JSON.parse(spec), null, 2);
  } catch {
    // Not valid JSON — show verbatim (the daemon is the semantic judge).
  }
  return (
    <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-card border border-border bg-surface-2 p-2 font-mono text-xs text-text">
      {pretty}
    </pre>
  );
}

// ── SummaryRow (write-flow 摘要预览的一行事实) ───────────────────────────────

export function SummaryLine({ children }: { children: ReactNode }) {
  return <p className="text-sm text-text">{children}</p>;
}

/** A scope cell that renders a resource code or a grey `*` for全资源/全动词. */
export function ScopeCell({
  resource,
  adapter,
}: {
  resource: string | null;
  adapter?: Adapter;
}) {
  if (resource === null) {
    return (
      <span className="font-mono text-xs text-text-muted" title="全资源/全动词">
        *
      </span>
    );
  }
  return <ResourceCodeBadge code={resource} adapter={adapter} />;
}
