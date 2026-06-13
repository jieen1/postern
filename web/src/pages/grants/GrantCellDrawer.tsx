import { Link } from 'react-router-dom';
import { CapabilityBadge, ResourceCodeBadge, SnowflakeId, TtlBadge } from '../../components';
import { formatTime } from '../../lib/format';
import type { MatrixCell } from './matrix';

/**
 * Cell provenance drawer (本页特有，只读展开 daemon 回报的事实).
 *  - persistent cell: shows it derives from a role binding + a read-only link to
 *    the Bindings page to revise it (we do NOT edit persistent grants here).
 *  - temp cell: shows the temp_grant id / granted_at / expires_at + an inline
 *    "立即吊销 revoke" entry.
 * No tier / role NAME is invented: `your_grants` carries only capability names,
 * so we render only the facts the wire actually provides.
 */
export function GrantCellDrawer({
  cell,
  now,
  onClose,
  onRevoke,
}: {
  cell: MatrixCell;
  now: number;
  onClose: () => void;
  onRevoke: (cell: MatrixCell) => void;
}) {
  return (
    <div className="fixed inset-0 z-40 flex justify-end">
      <div className="absolute inset-0 bg-black/40" onClick={onClose} aria-hidden="true" />
      <aside
        role="dialog"
        aria-label="格详情"
        aria-modal="true"
        className="relative z-10 flex h-full w-full max-w-sm flex-col gap-4 border-l border-border bg-surface px-4 py-4"
      >
        <header className="flex items-center gap-2">
          <ResourceCodeBadge code={cell.resource} />
          <span className="text-text-muted">×</span>
          <CapabilityBadge capability={cell.capability} />
        </header>

        {cell.state === 'temp' && cell.temp ? (
          <dl className="flex flex-col gap-3 text-sm">
            <div className="flex items-center gap-2">
              <dt className="w-20 text-text-muted">决策</dt>
              <dd className="text-warn">⏱ 临时授权 (allow)</dd>
            </div>
            <div className="flex items-center gap-2">
              <dt className="w-20 text-text-muted">来源</dt>
              <dd>
                <span className="mr-1 text-text-muted">temp_grant</span>
                <SnowflakeId id={cell.temp.id} />
              </dd>
            </div>
            <div className="flex items-center gap-2">
              <dt className="w-20 text-text-muted">授予</dt>
              <dd className="font-mono text-xs">{formatTime(cell.temp.granted_at)}</dd>
            </div>
            <div className="flex items-center gap-2">
              <dt className="w-20 text-text-muted">到期</dt>
              <dd className="flex items-center gap-2 font-mono text-xs">
                {formatTime(cell.temp.expires_at)}
                <TtlBadge expiresAt={cell.temp.expires_at} now={now} />
              </dd>
            </div>
            <div className="pt-2">
              <button
                type="button"
                onClick={() => onRevoke(cell)}
                className="rounded-card border border-deny/50 px-3 py-1.5 text-sm text-deny hover:bg-deny/10"
              >
                立即吊销 revoke
              </button>
            </div>
          </dl>
        ) : (
          <dl className="flex flex-col gap-3 text-sm">
            <div className="flex items-center gap-2">
              <dt className="w-20 text-text-muted">决策</dt>
              <dd className="text-allow">✅ 持久授权 (allow)</dd>
            </div>
            <p className="text-text-muted">
              该格由角色绑定赋予（持久授权）。本页只读展开，修订持久授权请前往绑定页。
            </p>
            <div>
              <Link to="/bindings" className="text-info hover:underline">
                → 去 Bindings 页修订
              </Link>
            </div>
          </dl>
        )}
      </aside>
    </div>
  );
}
