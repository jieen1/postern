import { useState } from 'react';
import { Check, Copy } from 'lucide-react';
import { cn } from '../../lib/cn';

/**
 * Read-only mono text panel for the import preview / export preview area.
 *
 * 设计文档 §5 lists `JsonViewer / SqlText` as the intended component, but that
 * component is not yet in the shared library (web/src/components has none). This
 * is a minimal local stand-in: mono font, read-only, copyable, scrollable. No
 * syntax highlighting (kept deliberately small per simplicity-first).
 *
 * suggest_shared: promote a `MonoTextView` / `JsonViewer` / `SqlText` to the
 * shared component library; multiple pages (export, audit, denials) want it.
 */
export function MonoTextView({
  text,
  label,
  emptyHint,
  className,
}: {
  text: string;
  label?: string;
  emptyHint?: string;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard may be unavailable; the text is still selectable.
    }
  }

  return (
    <div
      className={cn(
        'flex flex-col rounded-card border border-border bg-surface-2',
        className,
      )}
    >
      <div className="flex items-center justify-between border-b border-border px-3 py-1.5">
        <span className="font-mono text-xs text-text-muted">{label ?? 'preview'}</span>
        {text.length > 0 && (
          <button
            type="button"
            onClick={copy}
            aria-label="复制内容"
            title="复制内容"
            className="inline-flex items-center gap-1 text-xs text-text-muted hover:text-text"
          >
            {copied ? <Check size={12} className="text-allow" /> : <Copy size={12} />}
            复制
          </button>
        )}
      </div>
      {text.length === 0 ? (
        <div className="px-3 py-6 text-center text-xs text-text-muted">
          {emptyHint ?? '（空）'}
        </div>
      ) : (
        <pre className="max-h-72 overflow-auto px-3 py-2 font-mono text-xs leading-relaxed text-text">
          {text}
        </pre>
      )}
    </div>
  );
}
