import { Clock } from 'lucide-react';
import { Badge } from './Badge';
import { ttlRemainingMs, formatDuration } from '../lib/format';

/**
 * TTL countdown badge (设计系统 §4): shows remaining time from an absolute
 * expiry; turns amber as expiry nears and red/`expired` past it.
 */
export function TtlBadge({
  expiresAt,
  nearMs = 15 * 60_000,
  now = Date.now(),
}: {
  expiresAt: string | null;
  nearMs?: number;
  now?: number;
}) {
  const remaining = ttlRemainingMs(expiresAt, now);
  if (remaining === null) {
    return <Badge className="border-border text-text-muted">no TTL</Badge>;
  }
  const expired = remaining <= 0;
  const near = !expired && remaining <= nearMs;
  const cls = expired
    ? 'border-deny/50 text-deny'
    : near
      ? 'border-warn/50 text-warn'
      : 'border-border text-text-muted';
  return (
    <Badge className={cls}>
      <Clock size={12} />
      {formatDuration(remaining)}
    </Badge>
  );
}
