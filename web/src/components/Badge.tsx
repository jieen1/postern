import type { ReactNode } from 'react';
import { cn } from '../lib/cn';

/**
 * Base pill (radius-badge per §3.3). Status is conveyed by color + text/icon,
 * never color alone (§7 a11y). All semantic badges build on this.
 */
export function Badge({
  children,
  className,
  title,
}: {
  children: ReactNode;
  className?: string;
  title?: string;
}) {
  return (
    <span
      title={title}
      className={cn(
        'inline-flex items-center gap-1 rounded-badge border px-2 py-0.5 text-xs font-medium leading-none',
        className,
      )}
    >
      {children}
    </span>
  );
}
