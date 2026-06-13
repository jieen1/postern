import type { ReactNode } from 'react';
import { AlertTriangle, Inbox, Loader2 } from 'lucide-react';
import { cn } from '../lib/cn';

/**
 * Three states (设计系统 §4 / §7), all fail-closed:
 *  - LoadingSkeleton: shimmer rows, never fake data.
 *  - ErrorState: red, shows the verbatim error; never a fabricated result.
 *  - EmptyState: neutral, with an optional primary-action call to guide.
 */

export function LoadingSkeleton({ rows = 5 }: { rows?: number }) {
  return (
    <div className="flex flex-col gap-2" role="status" aria-label="加载中">
      <span className="sr-only">加载中</span>
      {Array.from({ length: rows }).map((_, i) => (
        <div
          key={i}
          className="h-8 animate-pulse rounded-card bg-surface-2"
          style={{ opacity: 1 - i * 0.12 }}
        />
      ))}
    </div>
  );
}

export function ErrorState({
  title = '加载失败',
  message,
  onRetry,
}: {
  title?: string;
  message?: string;
  onRetry?: () => void;
}) {
  return (
    <div
      role="alert"
      className="flex flex-col items-center gap-2 rounded-card border border-deny/40 bg-deny/5 px-6 py-8 text-center"
    >
      <AlertTriangle className="text-deny" size={24} />
      <div className="font-medium text-deny">{title}</div>
      {message && <div className="max-w-md font-mono text-xs text-text-muted">{message}</div>}
      {onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="mt-2 rounded-card border border-border px-3 py-1 text-sm hover:bg-surface-2"
        >
          重试
        </button>
      )}
    </div>
  );
}

export function EmptyState({
  title = '暂无数据',
  hint,
  action,
  icon,
}: {
  title?: string;
  hint?: string;
  action?: ReactNode;
  icon?: ReactNode;
}) {
  return (
    <div
      className={cn(
        'flex flex-col items-center gap-2 rounded-card border border-dashed border-border px-6 py-10 text-center',
      )}
    >
      <span className="text-text-muted">{icon ?? <Inbox size={24} />}</span>
      <div className="font-medium text-text">{title}</div>
      {hint && <div className="max-w-md text-sm text-text-muted">{hint}</div>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

export function InlineSpinner({ label }: { label?: string }) {
  return (
    <span className="inline-flex items-center gap-1 text-text-muted">
      <Loader2 size={14} className="animate-spin" />
      {label}
    </span>
  );
}
