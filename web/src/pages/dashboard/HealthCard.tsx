import { Activity, CircleCheck, CircleX, HardDrive } from 'lucide-react';
import { Link } from 'react-router-dom';
import { useHealth } from '../../api/hooks';
import { ErrorState, LoadingSkeleton, SnowflakeId } from '../../components';
import { formatDuration } from '../../lib/format';
import { cn } from '../../lib/cn';
import { Card, CardHeader } from './Card';

/**
 * HealthCard (01-dashboard §2 / §3): system-level hard signals, the first
 * fail-closed beacon. On a query error the lights show red/unknown ("daemon
 * 不可达") rather than defaulting to green — an uncertain posture is rendered
 * at its worst. Capacity watermark turns amber/red near the ceiling.
 */

const WATERMARK_AMBER = 0.75;
const WATERMARK_RED = 0.9;

/** A single health line: an icon-bearing semantic dot + label + value. */
function HealthLine({
  ok,
  unknown,
  label,
  children,
}: {
  ok: boolean;
  unknown?: boolean;
  label: string;
  children: React.ReactNode;
}) {
  const tone = unknown ? 'deny' : ok ? 'allow' : 'deny';
  return (
    <div className="flex items-center justify-between gap-3 py-1">
      <span className="flex items-center gap-2 text-text-muted">
        {/* Status conveyed by icon + text, never color alone (§7 a11y). */}
        {tone === 'allow' ? (
          <CircleCheck size={14} className="text-allow" aria-hidden />
        ) : (
          <CircleX size={14} className="text-deny" aria-hidden />
        )}
        {label}
      </span>
      <span className="font-mono text-xs text-text">{children}</span>
    </div>
  );
}

export function HealthCard() {
  const { data, isLoading, isError, error, refetch } = useHealth();

  return (
    <Card>
      <CardHeader
        icon={<Activity size={16} className="text-info" />}
        title="系统健康 Health"
        action={
          <Link
            to="/system"
            className="text-xs text-info hover:underline"
          >
            详情 Health →
          </Link>
        }
      />
      {isLoading ? (
        <LoadingSkeleton rows={4} />
      ) : isError || !data ? (
        // fail-closed: never optimistic green; surface daemon-unreachable.
        <ErrorState
          title="daemon 不可达"
          message={error instanceof Error ? error.message : undefined}
          onRetry={() => void refetch()}
        />
      ) : (
        <dl className="flex flex-col text-sm">
          <HealthLine ok={data.status === 'up'} label="daemon">
            {data.status === 'up' ? 'UP' : data.status === 'degraded' ? 'DEGRADED' : 'DOWN'}
          </HealthLine>
          <HealthLine ok={data.audit_writable} label="audit store">
            {data.audit_writable ? 'WRITABLE' : 'READ-ONLY'}
          </HealthLine>
          <div className="flex items-center justify-between gap-3 py-1">
            <span className="flex items-center gap-2 text-text-muted">
              <Activity size={14} className="text-text-muted" aria-hidden />
              policy_rev
            </span>
            {/* policy_rev is a machine fact rendered as a string (no Number). */}
            <SnowflakeId id={data.policy_rev} head={6} tail={4} />
          </div>
          <div className="flex items-center justify-between gap-3 py-1">
            <span className="flex items-center gap-2 text-text-muted">
              <Activity size={14} className="text-text-muted" aria-hidden />
              uptime
            </span>
            <span className="font-mono text-xs text-text">
              {formatDuration(data.uptime_ms)}
            </span>
          </div>
          <CapacityBar watermark={data.audit_watermark} />
        </dl>
      )}
    </Card>
  );
}

function CapacityBar({ watermark }: { watermark: number }) {
  const pct = Math.round(Math.min(1, Math.max(0, watermark)) * 100);
  const tone =
    watermark >= WATERMARK_RED ? 'deny' : watermark >= WATERMARK_AMBER ? 'warn' : 'allow';
  const barCls = { allow: 'bg-allow', warn: 'bg-warn', deny: 'bg-deny' }[tone];
  const labelCls = { allow: 'text-text-muted', warn: 'text-warn', deny: 'text-deny' }[tone];
  return (
    <div className="mt-2 flex flex-col gap-1 pt-2">
      <div className="flex items-center justify-between text-xs">
        <span className="flex items-center gap-2 text-text-muted">
          <HardDrive size={14} aria-hidden />
          audit store 容量
        </span>
        <span className={cn('font-mono', labelCls)}>
          {pct}%{tone !== 'allow' && ' · 逼近上限'}
        </span>
      </div>
      <div
        className="h-2 overflow-hidden rounded-badge bg-surface-2"
        role="meter"
        aria-label="audit store 容量水位"
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <div className={cn('h-full', barCls)} style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}
