import { useState } from 'react';
import { CheckCircle2, ChevronDown, ChevronRight, XCircle } from 'lucide-react';
import { StageChip } from '../../components';
import type { VerifyItem } from '../../api/types';
import { cn } from '../../lib/cn';
import { describeProbe } from './probeCatalog';

/**
 * One probe result row (04-verify.md §2 展开行).
 *
 * PASS green / FAIL red with the same fixed semantic + icon as the shared
 * `VerifyItemRow`; on FAIL the `gap_note` is shown VERBATIM (原样转述, never
 * reworded) in a red panel pointing at the breached defense line. The row
 * additionally exposes an expandable detail backed by the client-side static
 * catalog (intent / expected defense stage(s) / PASS criterion). FAIL rows
 * default open; PASS rows default collapsed.
 *
 * The shared `VerifyItemRow` carries no expand affordance / static catalog, so
 * this page renders a richer local row reusing the shared `StageChip` and the
 * identical PASS/FAIL + verbatim-gap_note discipline (see notes: candidate to
 * promote expansion onto the shared VerifyItemRow).
 */
export function ProbeRow({ item }: { item: VerifyItem }) {
  const desc = describeProbe(item.name);
  // FAIL auto-expands so the gap_note is immediately visible.
  const [open, setOpen] = useState(!item.pass);
  const panelId = `probe-detail-${item.name}`;

  return (
    <div
      className={cn(
        'rounded-card border',
        item.pass ? 'border-allow/30' : 'border-deny/40 bg-deny/5',
      )}
    >
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-controls={panelId}
        className="flex w-full items-start gap-3 px-3 py-2 text-left"
      >
        <span className="mt-0.5">
          {item.pass ? (
            <CheckCircle2 size={16} className="text-allow" aria-hidden />
          ) : (
            <XCircle size={16} className="text-deny" aria-hidden />
          )}
        </span>
        <span className="flex flex-1 flex-col gap-1">
          <span className="flex flex-wrap items-center gap-2">
            {desc && (
              <span className="font-mono text-xs text-text-muted">
                {String(desc.ordinal).padStart(1, '0')}
              </span>
            )}
            <span className={cn('text-sm font-medium', item.pass ? 'text-allow' : 'text-deny')}>
              {item.pass ? 'PASS' : 'FAIL'}
            </span>
            <span className="font-mono text-xs text-text">{item.name}</span>
            {desc && <span className="text-xs text-text-muted">{desc.label}</span>}
          </span>
          {/* gap_note 原样转述（仅 FAIL）— always rendered when present, even collapsed. */}
          {item.gap_note && (
            <span className="mt-1 block rounded-card border border-deny/40 bg-deny/10 px-2 py-1 font-mono text-xs text-deny">
              {item.gap_note}
            </span>
          )}
        </span>
        <span className="mt-0.5 text-text-muted" aria-hidden>
          {open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
        </span>
      </button>

      {open && (
        <div
          id={panelId}
          className="flex flex-col gap-2 border-t border-border px-3 py-2 pl-9 text-xs"
        >
          {desc ? (
            <>
              <div className="flex flex-col gap-1">
                <span className="text-text-muted">探针在做什么</span>
                <span className="text-text">{desc.intent}</span>
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-text-muted">预期防线落点</span>
                <span className="flex flex-wrap items-center gap-1">
                  {desc.stages.map((s) => (
                    <StageChip key={s} stage={s} />
                  ))}
                  {desc.compositeNote && (
                    <span className="text-text">{desc.compositeNote}</span>
                  )}
                </span>
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-text-muted">PASS 判据</span>
                <span className="text-text">{desc.passCriterion}</span>
              </div>
            </>
          ) : (
            // Unknown probe name — fail-closed: do NOT invent a description.
            <span className="text-text-muted">无静态描述（未知探针名）。</span>
          )}
        </div>
      )}
    </div>
  );
}
