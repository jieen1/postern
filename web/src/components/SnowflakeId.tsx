import { useState } from 'react';
import { Check, Copy } from 'lucide-react';
import { truncateId } from '../lib/format';
import { cn } from '../lib/cn';

/**
 * Snowflake id cell (设计系统 §3.4): mono font, middle-truncated, full value on
 * hover (title), one-click copy. The id is a STRING throughout — this component
 * never converts it to a number, so precision is never lost.
 */
export function SnowflakeId({
  id,
  className,
  head,
  tail,
}: {
  id: string;
  className?: string;
  head?: number;
  tail?: number;
}) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(id);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard may be unavailable (no-op; the full id is still in the title).
    }
  }

  return (
    <span className={cn('inline-flex items-center gap-1 font-mono text-xs', className)}>
      <span title={id} className="text-text">
        {truncateId(id, head, tail)}
      </span>
      <button
        type="button"
        onClick={copy}
        aria-label="复制完整 id"
        title="复制完整 id"
        className="text-text-muted hover:text-text"
      >
        {copied ? <Check size={12} className="text-allow" /> : <Copy size={12} />}
      </button>
    </span>
  );
}
