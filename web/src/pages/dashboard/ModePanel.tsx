import { Gauge } from 'lucide-react';
import { Link } from 'react-router-dom';
import { useModeState } from '../../api/hooks';
import {
  EmptyState,
  ErrorState,
  LoadingSkeleton,
  ModeBadge,
  ResourceCodeBadge,
  TtlBadge,
} from '../../components';
import type { ModeStateRow } from '../../api/types';
import { Card, CardHeader } from './Card';

/**
 * ModePanel (01-dashboard §2 / §3): the current posture — global mode + each
 * per-resource override with a TTL countdown. Read from the POST /v1/mode
 * same-source `mode_state` projection (NOT a nonexistent GET /v1/mode), the
 * same source the top-bar GlobalEmergencyBar badge uses.
 *
 * fail-closed: on a fetch error it shows "模式状态未知" and NEVER defaults to
 * NORMAL — an uncertain mode must not be rendered as "unrestricted".
 */
function globalRow(rows: ModeStateRow[] | undefined): ModeStateRow | undefined {
  return rows?.find((r) => r.scope === null);
}

function overrideRows(rows: ModeStateRow[] | undefined): ModeStateRow[] {
  return rows?.filter((r) => r.scope !== null) ?? [];
}

export function ModePanel() {
  const { data, isLoading, isError, error, refetch } = useModeState();
  const global = globalRow(data);
  const overrides = overrideRows(data);

  return (
    <Card>
      <CardHeader
        icon={<Gauge size={16} className="text-info" />}
        title="当前模式姿态 Mode"
        action={
          <Link to="/mode" className="text-xs text-info hover:underline">
            管理模式 → Mode
          </Link>
        }
      />
      {isLoading ? (
        <LoadingSkeleton rows={3} />
      ) : isError || !data || !global ? (
        // fail-closed: unknown mode is NOT normal; render an explicit red bar.
        <ErrorState
          title="模式状态未知"
          message={error instanceof Error ? error.message : '无法读取 mode_state 投影'}
          onRetry={() => void refetch()}
        />
      ) : (
        <div className="flex flex-col gap-2 text-sm">
          <div className="flex items-center justify-between gap-3">
            <span className="text-text-muted">全局</span>
            <ModeBadge mode={global.effective_mode} />
          </div>

          <div className="border-t border-border pt-2 text-xs text-text-muted">
            资源覆盖 ({overrides.length})
          </div>

          {overrides.length === 0 ? (
            <EmptyState title="无资源级模式覆盖" />
          ) : (
            <ul className="flex flex-col gap-2">
              {overrides.map((row) => (
                <li
                  key={row.scope ?? 'global'}
                  className="flex items-center justify-between gap-2"
                >
                  <Link
                    to={`/mode?resource=${encodeURIComponent(row.scope ?? '')}`}
                    className="hover:underline"
                  >
                    <ResourceCodeBadge code={row.scope ?? ''} />
                  </Link>
                  <span className="flex items-center gap-2">
                    <ModeBadge mode={row.mode} />
                    {/* Persisted absolute expiry (not an in-memory timer). */}
                    {row.expires_at && <TtlBadge expiresAt={row.expires_at} />}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </Card>
  );
}
