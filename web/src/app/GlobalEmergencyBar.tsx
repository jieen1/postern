import { useState } from 'react';
import { Snowflake } from 'lucide-react';
import { useModeState, useSetMode } from '../api/hooks';
import { ModeBadge } from '../components/ModeBadge';
import { ConfirmDialog } from '../components/ConfirmDialog';
import type { ModeStateRow } from '../api/types';

/**
 * Global emergency area (设计系统 §4 / §5): always in the top bar. Shows the
 * current GLOBAL mode badge and a one-tap Freeze switch (≤2 steps, prominent
 * but anti-misclick via a confirm-word dialog). When freeze is in effect a
 * full-width red pulsing banner is rendered by AppShell.
 */
function globalRow(rows: ModeStateRow[] | undefined): ModeStateRow | undefined {
  return rows?.find((r) => r.scope === null);
}

export function isFrozen(rows: ModeStateRow[] | undefined): boolean {
  return globalRow(rows)?.effective_mode === 'freeze';
}

export function GlobalEmergencyBar() {
  const { data } = useModeState();
  const setMode = useSetMode();
  const [confirm, setConfirm] = useState(false);

  const row = globalRow(data);
  const frozen = row?.effective_mode === 'freeze';

  return (
    <div className="flex items-center gap-2">
      {row && <ModeBadge mode={row.effective_mode} />}

      <button
        type="button"
        onClick={() => setConfirm(true)}
        disabled={frozen || setMode.isPending}
        className="inline-flex items-center gap-1 rounded-card border border-freeze/60 px-2 py-1 text-xs text-freeze hover:bg-freeze/10 disabled:opacity-50"
        title="全局冻结（应急拉闸）"
      >
        <Snowflake size={14} />
        {frozen ? '已冻结' : 'Freeze'}
      </button>

      <ConfirmDialog
        open={confirm}
        title="全局冻结（应急拉闸）"
        body="freeze 立即拒绝所有辖区的一切动词。仅在应急时使用。"
        confirmWord="freeze"
        confirmLabel="冻结"
        onConfirm={() => {
          setConfirm(false);
          setMode.mutate({
            scope: null,
            mode: 'freeze',
            version: row?.version ?? 0,
          });
        }}
        onCancel={() => setConfirm(false)}
      />
    </div>
  );
}

export { globalRow };
