/**
 * Snowflake-id presentation discipline (设计系统 §3.4).
 *
 * Ids are strings end-to-end; these helpers only format for display and NEVER
 * parse them as numbers. Truncation keeps head+tail with an ellipsis so the
 * mono-font column stays narrow while the full value is one copy away.
 */

/** Truncate a snowflake id's middle: `7300…0123`. Short ids pass through. */
export function truncateId(id: string, head = 4, tail = 4): string {
  if (id.length <= head + tail + 1) return id;
  return `${id.slice(0, head)}…${id.slice(-tail)}`;
}

/** Format an absolute wall-clock ms/ISO string for a dense table cell. */
export function formatTime(value: string | number | null | undefined): string {
  if (value === null || value === undefined || value === '') return '—';
  const ms = typeof value === 'number' ? value : Date.parse(value);
  if (Number.isNaN(ms)) return String(value);
  return new Date(ms).toISOString().replace('T', ' ').replace('.000Z', 'Z');
}

/** Relative TTL label from an absolute expiry; near-expiry handled by caller. */
export function ttlRemainingMs(expiresAt: string | null, now = Date.now()): number | null {
  if (!expiresAt) return null;
  const ms = Date.parse(expiresAt);
  if (Number.isNaN(ms)) return null;
  return ms - now;
}

/** Human duration for a TTL badge, e.g. `2h 5m` / `expired`. */
export function formatDuration(ms: number): string {
  if (ms <= 0) return 'expired';
  const totalMin = Math.floor(ms / 60_000);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return `${Math.floor(ms / 1000)}s`;
}
