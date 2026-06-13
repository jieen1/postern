import { useId } from 'react';
import { Search, X } from 'lucide-react';
import type { AuditEventKind, Decision } from '../../api/types';

/**
 * Audit filter bar (02-audit §五): since / principal / kind / decision (a
 * segmented single-select that DISTINGUISHES escalate_denied from deny) +
 * apply / clear + a live total count. Reuses base tokens & control styling;
 * sorting/paging are server-driven, so this only owns the query inputs.
 */

/** Decision values offered in the segmented filter (escalate_denied kept distinct). */
const DECISION_OPTIONS: readonly { value: Decision | 'all'; label: string }[] = [
  { value: 'all', label: '全部' },
  { value: 'allow', label: 'allow' },
  { value: 'deny', label: 'deny' },
  { value: 'escalate_denied', label: 'escalate_denied' },
] as const;

/** Event-kind options (doc-specified taxonomy). */
const KIND_OPTIONS: readonly AuditEventKind[] = [
  'request',
  'policy_change',
  'credential_event',
  'lifecycle',
  'connection_event',
  'alert',
] as const;

export interface AuditFilters {
  since: string;
  principal: string;
  kind: AuditEventKind | '';
  decision: Decision | 'all';
}

export const EMPTY_FILTERS: AuditFilters = {
  since: '',
  principal: '',
  kind: '',
  decision: 'all',
};

export function AuditFilterBar({
  draft,
  onDraftChange,
  onApply,
  onClear,
  total,
  /** Whether a query is in flight (disables apply to avoid double-submit). */
  busy,
  inputRef,
}: {
  draft: AuditFilters;
  onDraftChange: (next: AuditFilters) => void;
  onApply: () => void;
  onClear: () => void;
  total: number | undefined;
  busy?: boolean;
  inputRef?: React.Ref<HTMLInputElement>;
}) {
  const sinceId = useId();
  const principalId = useId();
  const kindId = useId();

  function set<K extends keyof AuditFilters>(key: K, value: AuditFilters[K]) {
    onDraftChange({ ...draft, [key]: value });
  }

  function submit(e: React.FormEvent) {
    e.preventDefault();
    onApply();
  }

  return (
    <form
      onSubmit={submit}
      aria-label="审计筛选条"
      className="flex flex-col gap-3 rounded-card border border-border bg-surface p-3"
    >
      <div className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1 text-xs text-text-muted" htmlFor={sinceId}>
          since（时间范围）
          <input
            id={sinceId}
            ref={inputRef}
            type="datetime-local"
            value={draft.since}
            onChange={(e) => set('since', e.target.value)}
            className="rounded-card border border-border bg-surface px-2 py-1 font-mono text-sm text-text"
          />
        </label>

        <label className="flex flex-col gap-1 text-xs text-text-muted" htmlFor={principalId}>
          principal（主体）
          <input
            id={principalId}
            type="text"
            value={draft.principal}
            placeholder="agent 名"
            onChange={(e) => set('principal', e.target.value)}
            className="rounded-card border border-border bg-surface px-2 py-1 text-sm text-text"
          />
        </label>

        <label className="flex flex-col gap-1 text-xs text-text-muted" htmlFor={kindId}>
          kind（事件类）
          <select
            id={kindId}
            value={draft.kind}
            onChange={(e) => set('kind', e.target.value as AuditEventKind | '')}
            className="rounded-card border border-border bg-surface px-2 py-1 text-sm text-text"
          >
            <option value="">全部</option>
            {KIND_OPTIONS.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </select>
        </label>
      </div>

      <fieldset className="flex flex-wrap items-center gap-2">
        <legend className="sr-only">decision 决策筛选</legend>
        <span className="text-xs text-text-muted">decision</span>
        {DECISION_OPTIONS.map((opt) => {
          const active = draft.decision === opt.value;
          return (
            <label
              key={opt.value}
              className={[
                'cursor-pointer rounded-badge border px-2 py-0.5 text-xs',
                active
                  ? 'border-info bg-info/10 text-info'
                  : 'border-border text-text-muted hover:text-text',
              ].join(' ')}
            >
              <input
                type="radio"
                name="decision"
                value={opt.value}
                checked={active}
                onChange={() => set('decision', opt.value)}
                className="sr-only"
              />
              {opt.label}
            </label>
          );
        })}
      </fieldset>

      <div className="flex items-center gap-3">
        <button
          type="submit"
          disabled={busy}
          className="inline-flex items-center gap-1 rounded-card border border-info/50 bg-info/10 px-3 py-1 text-sm text-info disabled:opacity-40 hover:enabled:bg-info/20"
        >
          <Search size={14} />
          应用
        </button>
        <button
          type="button"
          onClick={onClear}
          className="inline-flex items-center gap-1 rounded-card border border-border px-3 py-1 text-sm text-text-muted hover:bg-surface-2"
        >
          <X size={14} />
          清空
        </button>
        <span className="ml-auto font-mono text-xs text-text-muted">
          匹配 {total ?? '—'} 条
        </span>
      </div>
    </form>
  );
}
