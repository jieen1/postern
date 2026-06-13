import { SnowflakeId } from '../../components/SnowflakeId';
import { ModeBadge } from '../../components/ModeBadge';
import { TtlBadge } from '../../components/TtlBadge';
import { formatTime } from '../../lib/format';
import { MODE_NARROWING } from './mode-facts';
import type { ModeStateRow } from '../../api/types';

/**
 * Global jurisdiction card (本页特有组装，11-mode.md §二/§五): a single,
 * un-paged row pinned atop the board, SAME-SOURCE as the top-bar
 * GlobalEmergencyBar (both read/write the `/v1/mode` global row).
 *
 * When the store holds no explicit global row, the global mode is `normal`
 * (store-absence = normal, rendered as fact). The card surfaces effective mode,
 * TTL, the meta sub-row (updated_at / updated_by / policy_rev) and two write
 * actions ("切换" / "回落 normal"). It does NOT submit — the page owns the write
 * flow; the card only signals intent via the callbacks.
 */
export function GlobalModeCard({
  row,
  onSwitch,
  onFallback,
  disabled,
}: {
  /** The global mode-state row, or null when the store holds none (= normal). */
  row: ModeStateRow | null;
  onSwitch: () => void;
  onFallback: () => void;
  disabled?: boolean;
}) {
  // Store-absence is normal (§三 空态：无显式全局行即 normal).
  const effective = row?.effective_mode ?? 'normal';
  const isNormal = effective === 'normal';

  return (
    <section
      aria-label="全局辖区"
      className="rounded-card border border-border bg-surface p-4"
    >
      <div className="flex flex-wrap items-center gap-3">
        <span className="text-sm font-medium text-text">全局辖区 (Global)</span>
        <ModeBadge mode={effective} />
        <span className="text-xs text-text-muted">作用域: 全局</span>
        <TtlBadge expiresAt={row?.expires_at ?? null} />
      </div>

      <p className="mt-2 text-xs text-text-muted">{MODE_NARROWING[effective]}</p>

      <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-text-muted">
        <span>
          生效自 <span className="font-mono">{formatTime(row?.updated_at)}</span>
        </span>
        <span>
          by <span className="font-mono">{row?.updated_by ?? '—'}</span>
        </span>
        <span className="inline-flex items-center gap-1">
          policy_rev <SnowflakeId id={row?.policy_rev ?? '0'} />
        </span>
      </div>

      <div className="mt-3 flex flex-wrap gap-2">
        <button
          type="button"
          onClick={onSwitch}
          disabled={disabled}
          className="rounded-card border border-border px-3 py-1.5 text-sm hover:enabled:bg-surface-2 disabled:opacity-50"
        >
          切换全局模式
        </button>
        <button
          type="button"
          onClick={onFallback}
          disabled={disabled || isNormal}
          className="rounded-card border border-border px-3 py-1.5 text-sm hover:enabled:bg-surface-2 disabled:opacity-50"
          title={isNormal ? '已是 normal' : '显式回落到 normal'}
        >
          回落 normal
        </button>
      </div>
    </section>
  );
}
