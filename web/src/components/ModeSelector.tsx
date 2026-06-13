import { useState } from 'react';
import { MODES, type Mode } from '../api/types';
import { ModeBadge } from './ModeBadge';
import { ConfirmDialog } from './ConfirmDialog';
import { cn } from '../lib/cn';

/**
 * Mode selector (设计系统 §4): single-select over the closed 4-value mode set
 * with an optional TTL. Switching to `freeze` is high-risk and routes through a
 * confirm-word dialog (anti-misclick). Emits `(mode, ttlMs|null)` on commit.
 */
export function ModeSelector({
  value,
  ttlMs,
  onChange,
  disabled,
}: {
  value: Mode;
  ttlMs?: number | null;
  onChange: (mode: Mode, ttlMs: number | null) => void;
  disabled?: boolean;
}) {
  const [ttlMinutes, setTtlMinutes] = useState<string>(
    ttlMs ? String(Math.round(ttlMs / 60000)) : '',
  );
  const [pendingFreeze, setPendingFreeze] = useState(false);

  function commit(mode: Mode) {
    const parsed = ttlMinutes.trim() === '' ? null : Number(ttlMinutes) * 60000;
    const ttl = parsed !== null && Number.isFinite(parsed) && parsed > 0 ? parsed : null;
    onChange(mode, ttl);
  }

  function select(mode: Mode) {
    if (mode === 'freeze') {
      setPendingFreeze(true);
      return;
    }
    commit(mode);
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-2" role="radiogroup" aria-label="模式">
        {MODES.map((mode) => (
          <button
            key={mode}
            type="button"
            role="radio"
            aria-checked={value === mode}
            disabled={disabled}
            onClick={() => select(mode)}
            className={cn(
              'rounded-card border px-3 py-2 text-left disabled:opacity-50',
              value === mode ? 'border-info bg-surface-2' : 'border-border hover:bg-surface-2',
            )}
          >
            <ModeBadge mode={mode} />
          </button>
        ))}
      </div>

      <label className="flex items-center gap-2 text-sm text-text-muted">
        TTL（分钟，留空=永久）
        <input
          type="number"
          min={1}
          value={ttlMinutes}
          onChange={(e) => setTtlMinutes(e.target.value)}
          disabled={disabled}
          className="w-24 rounded-card border border-border bg-bg px-2 py-1"
        />
      </label>

      <ConfirmDialog
        open={pendingFreeze}
        title="切换为 FREEZE（全局拉闸）"
        body="freeze 将拒绝该辖区一切动词。这是高危应急动作。"
        confirmWord="freeze"
        confirmLabel="冻结"
        onConfirm={() => {
          setPendingFreeze(false);
          commit('freeze');
        }}
        onCancel={() => setPendingFreeze(false)}
      />
    </div>
  );
}
