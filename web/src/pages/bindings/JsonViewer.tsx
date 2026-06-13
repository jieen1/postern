/**
 * JsonViewer — mono, read-only, pretty-printed machine-fact view (设计系统 §4
 * "JsonViewer / SqlText"). The base design system lists this component but the
 * shared `components/` index does not yet export one, so it is implemented here
 * locally and reported as a "promote to shared" candidate. It is what-you-see-
 * is-what-you-send for the selector spec preview (07-bindings.md §2.2): the
 * exact JSON text the create call will submit, never re-derived.
 */

export function JsonViewer({
  value,
  label,
}: {
  /** Pre-serialized JSON text (already the wire form). */
  value: string;
  label?: string;
}) {
  // Pretty-print for readability; if `value` is not valid JSON, show it raw
  // (fail-soft display only — the submitted spec is still `value` verbatim).
  let pretty = value;
  try {
    pretty = JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    pretty = value;
  }
  return (
    <pre
      aria-label={label}
      className="overflow-x-auto rounded-card border border-border bg-surface-2 px-3 py-2 font-mono text-xs text-text"
    >
      {pretty}
    </pre>
  );
}
