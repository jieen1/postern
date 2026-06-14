import { useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { cn } from '../../lib/cn';

/**
 * Danger confirmation for "运行红队自检" (04-verify.md §4.1 / §4.3).
 *
 * The design mandates an explicit CHECKBOX acknowledgment ("我理解这会发起真实
 * 数据面探针并写审计") plus an action-summary preview (not a policy diff). The
 * shared `ConfirmDialog` only supports a typed confirm-word gate, so this page
 * uses a focused local dialog that follows the same danger-modal shell while
 * carrying the checkbox the spec requires. (See notes: candidate to promote a
 * checkbox variant onto the shared ConfirmDialog.)
 */
export function RunVerifyConfirm({
  open,
  policyRev,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  /** Current snapshot revision the run will reflect (string — id discipline). */
  policyRev: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const [ack, setAck] = useState(false);
  if (!open) return null;

  function close() {
    setAck(false);
    onCancel();
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="运行红队自检？"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-md rounded-card border border-border bg-surface p-5 shadow-lg">
        <div className="mb-3 flex items-center gap-2">
          <AlertTriangle className="text-deny" size={18} />
          <h2 className="text-lg font-medium">运行红队自检？</h2>
        </div>

        <div className="mb-4 text-sm text-text-muted">
          <p className="mb-2">本次动作摘要（不改任何策略）：</p>
          <ul className="flex list-disc flex-col gap-1 pl-5">
            <li>以临时低权 Principal 自发 9 条应被拒探针</li>
            <li>不改任何策略（无 policy_rev 前进）</li>
            <li>
              反映当前快照（policy_rev{' '}
              <span className="font-mono text-xs text-text">{policyRev ?? '—'}</span>）
            </li>
          </ul>
        </div>

        <label className="mb-4 flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            checked={ack}
            onChange={(e) => setAck(e.target.checked)}
            className="mt-0.5"
            aria-label="我理解这会发起真实数据面探针并写审计"
          />
          <span>我理解这会发起真实数据面探针并写审计。</span>
        </label>

        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={close}
            className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
          >
            取消
          </button>
          <button
            type="button"
            disabled={!ack}
            onClick={() => {
              setAck(false);
              onConfirm();
            }}
            className={cn(
              'rounded-card px-3 py-1.5 text-sm text-white disabled:opacity-40',
              'bg-deny hover:enabled:brightness-110',
            )}
          >
            运行
          </button>
        </div>
      </div>
    </div>
  );
}
