import { useState } from 'react';
import { ChevronDown, Download } from 'lucide-react';
import type { AuditQuery } from '../../api/types';

/**
 * Export menu (02-audit §4.3 / §五 ExportMenu): the page's only "action", and
 * it is NOT a write — it reads the current audit window under the active filter
 * and serializes it as JSONL (one JSON object per line), machine-shaped, ids as
 * STRINGS, content already redacted (same source as the human render). It never
 * mutates server state, so there is no confirm dialog and no optimistic lock.
 */
export function ExportMenu({
  query,
  /** Resolves the rows to serialize (the current filtered window). */
  onExport,
  disabled,
}: {
  query: AuditQuery;
  onExport: () => Promise<unknown[]> | unknown[];
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);

  async function exportJsonl() {
    setOpen(false);
    const rows = await onExport();
    // JSON.stringify keeps snowflake ids as strings — never coerced to Number.
    const jsonl = rows.map((r) => JSON.stringify(r)).join('\n');
    const blob = new Blob([jsonl], { type: 'application/x-ndjson' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = exportName(query);
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  }

  return (
    <div className="relative">
      <button
        type="button"
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex items-center gap-1 rounded-card border border-border bg-surface px-3 py-1.5 text-sm text-text disabled:opacity-40 hover:enabled:bg-surface-2"
      >
        <Download size={14} />
        导出 JSONL
        <ChevronDown size={14} />
      </button>
      {open && (
        <div
          role="menu"
          className="absolute right-0 z-10 mt-1 w-56 rounded-card border border-border bg-surface p-1 text-sm shadow-lg"
        >
          <button
            type="button"
            role="menuitem"
            onClick={exportJsonl}
            className="w-full rounded-card px-2 py-1.5 text-left hover:bg-surface-2"
          >
            导出当前筛选结果（JSONL）
          </button>
        </div>
      )}
    </div>
  );
}

/** Deterministic export filename embedding the active filter, for traceability. */
function exportName(query: AuditQuery): string {
  const parts = ['audit'];
  if (query.principal) parts.push(query.principal);
  if (query.kind) parts.push(query.kind);
  if (query.decision) parts.push(query.decision);
  return `${parts.join('-')}.jsonl`;
}
