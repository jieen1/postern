import { Construction } from 'lucide-react';

/**
 * Page placeholder for the scaffold: renders the page title + its primary API +
 * an "实现中" marker. Page bodies are built on top of this base later; the
 * route is fully navigable now.
 */
export function PlaceholderPage({ title, api }: { title: string; api: string }) {
  return (
    <section>
      <h1 className="text-2xl font-medium">{title}</h1>
      <p className="mt-1 font-mono text-xs text-text-muted">{api}</p>
      <div className="mt-6 flex items-center gap-2 rounded-card border border-dashed border-border px-4 py-6 text-text-muted">
        <Construction size={18} />
        实现中
      </div>
    </section>
  );
}
