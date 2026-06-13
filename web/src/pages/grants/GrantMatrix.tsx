import { Check, X } from 'lucide-react';
import { CAPABILITIES } from '../../api/types';
import { CapabilityBadge, ResourceCodeBadge, TtlBadge } from '../../components';
import { cn } from '../../lib/cn';
import type { MatrixCell, MatrixRow } from './matrix';

/**
 * Dense Resource × Capability grid (本页特有，纯展示，零授权计算).
 * Rows are ResourceCodeBadge, column headers are the six CapabilityBadge verbs,
 * cells render a three-state DECISION symbol (✅ persistent / ⏱ temp / ❌ deny)
 * — color is never the only signal (icon + text, AA). Clicking a cell opens its
 * provenance drawer. A missing row == default deny; we never synthesize rows.
 */

const STATE_META = {
  persistent: {
    label: '持久',
    icon: <Check size={12} />,
    cls: 'border-allow/50 text-allow',
  },
  temp: {
    label: '临时',
    icon: null,
    cls: 'border-warn/50 text-warn',
  },
  deny: {
    label: '默认拒绝',
    icon: <X size={12} />,
    cls: 'border-border text-text-muted',
  },
} as const;

function CellButton({
  cell,
  now,
  onSelect,
}: {
  cell: MatrixCell;
  now: number;
  onSelect: (cell: MatrixCell) => void;
}) {
  const meta = STATE_META[cell.state];
  const interactive = cell.state !== 'deny';
  const aria = `${cell.resource} × ${cell.capability}：${meta.label}`;
  return (
    <td className="border-l border-border px-2 py-1 text-center align-middle">
      <button
        type="button"
        disabled={!interactive}
        onClick={() => interactive && onSelect(cell)}
        aria-label={aria}
        title={aria}
        className={cn(
          'inline-flex min-w-[3.5rem] flex-col items-center gap-0.5 rounded-badge border px-2 py-1 text-xs',
          meta.cls,
          interactive ? 'cursor-pointer hover:brightness-110' : 'cursor-default',
        )}
      >
        <span className="inline-flex items-center gap-1 font-medium">
          {cell.state === 'temp' ? <span aria-hidden>⏱</span> : meta.icon}
          {meta.label}
        </span>
        {cell.state === 'temp' && cell.temp && (
          <TtlBadge expiresAt={cell.temp.expires_at} now={now} />
        )}
      </button>
    </td>
  );
}

export function GrantMatrix({
  rows,
  now,
  onSelectCell,
}: {
  rows: MatrixRow[];
  now: number;
  onSelectCell: (cell: MatrixCell) => void;
}) {
  return (
    <div className="overflow-x-auto rounded-card border border-border">
      <table className="w-full border-collapse text-sm" aria-label="生效授权矩阵">
        <caption className="sr-only">
          授权矩阵：行为 Scope 内资源，列为六个能力动词，格为决策（持久 / 临时 / 默认拒绝）
        </caption>
        <thead>
          <tr className="border-b border-border bg-surface-2 text-left text-text-muted">
            <th scope="col" className="px-3 py-2 font-medium">
              Resource
            </th>
            {CAPABILITIES.map((cap) => (
              <th
                key={cap}
                scope="col"
                className="border-l border-border px-2 py-2 text-center font-medium"
              >
                <span className="inline-flex justify-center">
                  <CapabilityBadge capability={cap} />
                </span>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.resource} className="border-b border-border last:border-0">
              <th scope="row" className="px-3 py-2 text-left font-normal">
                <ResourceCodeBadge code={row.resource} />
              </th>
              {row.cells.map((cell) => (
                <CellButton
                  key={cell.capability}
                  cell={cell}
                  now={now}
                  onSelect={onSelectCell}
                />
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
