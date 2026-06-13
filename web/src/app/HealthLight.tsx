import { useHealth } from '../api/hooks';
import { cn } from '../lib/cn';

/**
 * Daemon health light (设计系统 §5 顶栏常驻). Green up / amber degraded / red
 * down. Fail-closed: a query error or no data shows as down, never optimistic.
 */
export function HealthLight() {
  const { data, isError, isLoading } = useHealth();
  const status = isError ? 'down' : (data?.status ?? (isLoading ? 'loading' : 'down'));

  const cls = {
    up: 'bg-allow',
    degraded: 'bg-warn',
    down: 'bg-deny',
    loading: 'bg-text-muted animate-pulse',
  }[status];

  const label =
    status === 'up'
      ? `健康 · rev ${data?.policy_rev ?? '?'}`
      : status === 'degraded'
        ? '降级'
        : status === 'loading'
          ? '连接中'
          : '不可达';

  return (
    <span className="inline-flex items-center gap-2 text-xs text-text-muted" title={label}>
      <span className={cn('h-2 w-2 rounded-full', cls)} />
      {label}
    </span>
  );
}
