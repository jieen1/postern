import type { ReactNode } from 'react';
import { cn } from '../../lib/cn';

/**
 * Local card container for the Dashboard grid (01-dashboard §2). The Dashboard
 * is an observation panel, not a list page, so cards are composed from base
 * components inside this thin surface wrapper (tokens only — no magic values).
 * Suggested for promotion to the shared library if other observation pages
 * adopt the same card grid.
 */
export function Card({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <section
      className={cn(
        'flex flex-col gap-3 rounded-card border border-border bg-surface p-4',
        className,
      )}
    >
      {children}
    </section>
  );
}

export function CardHeader({
  icon,
  title,
  action,
}: {
  icon?: ReactNode;
  title: string;
  action?: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2">
      <h2 className="flex items-center gap-2 text-sm font-medium text-text">
        {icon}
        {title}
      </h2>
      {action && <div className="ml-auto">{action}</div>}
    </div>
  );
}
