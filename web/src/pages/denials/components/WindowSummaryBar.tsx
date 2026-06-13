import type { DenialWindow } from '../../../api/types';
import { WINDOW_LABEL } from '../lib';

/**
 * WindowSummaryBar (窗口汇总条) — echoes the window the user picked and the
 * group count (`total` from the paged envelope = number of aggregation groups).
 *
 * The endpoint does NOT return a server-resolved UTC start/end nor a
 * window-wide deny-EVENT total, so those are intentionally NOT shown — the page
 * never fabricates them (fail-closed). When the backend DTO grows those fields
 * they can be surfaced here.
 */
export function WindowSummaryBar({
  window,
  groupCount,
}: {
  window: DenialWindow;
  groupCount: number;
}) {
  return (
    <div
      aria-label="窗口汇总"
      className="rounded-card border border-border bg-surface-2 px-3 py-2 text-sm text-text-muted"
    >
      窗口{' '}
      <span className="font-medium text-text">{WINDOW_LABEL[window]}</span>
      <span className="mx-1 font-mono text-xs">({window})</span>
      <span className="mx-2 text-border">·</span>
      聚合组{' '}
      <span data-testid="group-count" className="font-mono font-medium text-text">
        {groupCount}
      </span>
    </div>
  );
}
