import type { ReactNode } from 'react';
import { X } from 'lucide-react';

/**
 * Right-side form drawer (设计系统 §4 / §7) for create/edit write operations.
 * Pages put their RHF+Zod form inside; the unified write flow (summary preview
 * → danger confirm → invalidate → policy_rev advanced / 409 refresh) is owned
 * by each page. This is the shell + open/close + a footer slot only.
 */
export function FormDrawer({
  open,
  title,
  onClose,
  children,
  footer,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}) {
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-40 flex justify-end">
      <div
        className="absolute inset-0 bg-black/40"
        onClick={onClose}
        aria-hidden="true"
      />
      <aside
        role="dialog"
        aria-label={title}
        aria-modal="true"
        className="relative z-10 flex h-full w-full max-w-md flex-col border-l border-border bg-surface"
      >
        <header className="flex items-center justify-between border-b border-border px-4 py-3">
          <h2 className="text-lg font-medium">{title}</h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭"
            className="text-text-muted hover:text-text"
          >
            <X size={18} />
          </button>
        </header>
        <div className="flex-1 overflow-y-auto px-4 py-4">{children}</div>
        {footer && (
          <footer className="border-t border-border px-4 py-3">{footer}</footer>
        )}
      </aside>
    </div>
  );
}
