import { AlertTriangle, ArrowDownToLine } from 'lucide-react';
import { CapabilityBadge, ResourceCodeBadge, StageChip } from '../../../components';
import type { DenialSummaryRow } from '../../../api/types';
import { ALERT_THRESHOLD, DASH, groupKey } from '../lib';

/**
 * AlertBand (告警带) — amber horizontal list of over-threshold groups, built
 * from base tokens (--warn) + StageChip/CapabilityBadge/ResourceCodeBadge. It
 * renders ONLY when at least one group is over threshold; with no alerts the
 * whole band is hidden (no "0 alerts" green bar — avoids implying "已审完").
 *
 * Each item locates its matching ranking row via `onLocate(groupKey)`.
 */
export function AlertBand({
  rows,
  onLocate,
}: {
  rows: DenialSummaryRow[];
  onLocate: (key: string) => void;
}) {
  if (rows.length === 0) return null;

  return (
    <section
      aria-label="告警 Alerts"
      className="rounded-card border border-warn/40 bg-warn/5"
    >
      <header className="flex items-center gap-2 border-b border-warn/30 px-3 py-2 text-sm font-medium text-warn">
        <AlertTriangle size={16} aria-hidden />
        告警 Alerts ({rows.length})
        <span className="font-normal text-text-muted">
          超阈值聚合组 · 点击定位榜内对应行（阈值 ≥{ALERT_THRESHOLD}）
        </span>
      </header>
      <ul className="divide-y divide-warn/20">
        {rows.map((row) => {
          const key = groupKey(row);
          return (
            <li
              key={key}
              className="flex flex-wrap items-center gap-2 px-3 py-2 text-sm"
            >
              <AlertTriangle size={14} className="text-warn" aria-hidden />
              <span className="text-text">{row.principal ?? DASH}</span>
              <span className="text-text-muted">×</span>
              <ResourceCodeBadge code={row.resource} />
              <span className="text-text-muted">×</span>
              <StageChip stage={row.stage} />
              <span className="text-text-muted">×</span>
              <CapabilityBadge capability={row.capability} />
              <span className="ml-1 font-mono font-semibold text-warn">
                {row.count} 次
              </span>
              <span className="text-xs text-text-muted">
                ≥阈值 {ALERT_THRESHOLD}
              </span>
              <button
                type="button"
                onClick={() => onLocate(key)}
                className="ml-auto inline-flex items-center gap-1 rounded-card border border-warn/40 px-2 py-0.5 text-xs text-warn hover:bg-warn/10"
              >
                <ArrowDownToLine size={12} aria-hidden />
                定位
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
