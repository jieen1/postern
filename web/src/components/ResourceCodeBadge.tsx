import { Boxes, Database, Globe } from 'lucide-react';
import type { Adapter } from '../api/types';
import { cn } from '../lib/cn';

/**
 * Resource code badge (设计系统 §4): mono code + a small adapter/transport icon.
 * NEVER renders a real address — only the codename.
 */
const ADAPTER_ICON: Record<Adapter, typeof Database> = {
  postgres: Database,
  http: Globe,
  docker: Boxes,
};

export function ResourceCodeBadge({
  code,
  adapter,
  transport,
  className,
}: {
  code: string;
  adapter?: Adapter;
  transport?: string;
  className?: string;
}) {
  const Icon = adapter ? ADAPTER_ICON[adapter] : undefined;
  const title = [adapter, transport].filter(Boolean).join(' · ') || undefined;
  return (
    <span
      title={title}
      className={cn(
        'inline-flex items-center gap-1 rounded-badge border border-border bg-surface-2 px-2 py-0.5 font-mono text-xs',
        className,
      )}
    >
      {Icon && <Icon size={12} className="text-text-muted" />}
      {code}
    </span>
  );
}
