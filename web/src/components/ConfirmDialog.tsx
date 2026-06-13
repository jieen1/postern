import { useState, type ReactNode } from 'react';
import { AlertTriangle } from 'lucide-react';
import { cn } from '../lib/cn';

/**
 * Danger confirmation (设计系统 §4 / §7 危险动作清单). For destructive / scope-
 * widening / freeze / shutdown / import-overwrite. Optionally requires typing a
 * confirm word (anti-misclick) before the confirm button enables.
 */
export function ConfirmDialog({
  open,
  title,
  body,
  confirmWord,
  confirmLabel = '确认',
  danger = true,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  body?: ReactNode;
  /** If set, the user must type this exact word to enable confirm. */
  confirmWord?: string;
  confirmLabel?: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const [typed, setTyped] = useState('');
  if (!open) return null;
  const ready = !confirmWord || typed === confirmWord;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={title}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-md rounded-card border border-border bg-surface p-5 shadow-lg">
        <div className="mb-3 flex items-center gap-2">
          {danger && <AlertTriangle className="text-deny" size={18} />}
          <h2 className="text-lg font-medium">{title}</h2>
        </div>
        {body && <div className="mb-4 text-sm text-text-muted">{body}</div>}
        {confirmWord && (
          <label className="mb-4 block text-sm">
            输入 <code className="font-mono text-deny">{confirmWord}</code> 以确认：
            <input
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1 font-mono"
              autoFocus
            />
          </label>
        )}
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={() => {
              setTyped('');
              onCancel();
            }}
            className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
          >
            取消
          </button>
          <button
            type="button"
            disabled={!ready}
            onClick={() => {
              setTyped('');
              onConfirm();
            }}
            className={cn(
              'rounded-card px-3 py-1.5 text-sm text-white disabled:opacity-40',
              danger ? 'bg-deny hover:enabled:brightness-110' : 'bg-info hover:enabled:brightness-110',
            )}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
