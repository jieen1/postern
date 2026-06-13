/**
 * ExpansionPreview — renders the daemon-reported expansion of a scope spec
 * (07-bindings.md §2.2, §2.3, §六). Built from base components (ResourceCodeBadge
 * + CapabilityBadge); holds ZERO expansion logic of its own.
 *
 * Fail-closed states (§3.2 / §六, all non-leaking):
 *  - loading: "展开计算中…" — NEVER an optimistic guessed resource set.
 *  - error (probe unreachable): "无法计算展开——按未授权对待" — does NOT fall
 *    back to "show all resources".
 *  - parse_error (异常 C): red "选择器语法不可解析——将不授予任何资源".
 *  - empty set (异常 B): amber "展开为 0 个资源（无匹配标签）" — a fact, not an
 *    error, not a pass.
 */

import { AlertTriangle, Loader2, ShieldAlert } from 'lucide-react';
import { CapabilityBadge, ResourceCodeBadge } from '@/components';
import type { ExpansionPreview as PreviewData, PreviewGrantCell } from './api';

export function ExpansionPreview({
  data,
  loading,
  error,
}: {
  data: PreviewData | undefined;
  loading: boolean;
  /** Probe failed (network / non-2xx) — fail-closed, treat as unauthorized. */
  error: boolean;
}) {
  if (loading) {
    return (
      <p
        role="status"
        className="flex items-center gap-2 text-xs text-text-muted"
      >
        <Loader2 size={14} className="animate-spin" />
        展开计算中…
      </p>
    );
  }

  // Fail-closed: probe unreachable ⇒ treat as unauthorized, never "show all".
  if (error) {
    return (
      <p
        role="alert"
        className="flex items-center gap-2 rounded-card border border-deny/40 bg-deny/5 px-3 py-2 text-xs text-deny"
      >
        <ShieldAlert size={14} />
        无法计算展开——按未授权对待
      </p>
    );
  }

  if (!data) return null;

  // 异常 C: unparseable selector — red, fail-closed, will grant nothing.
  if (data.parse_error) {
    return (
      <p
        role="alert"
        className="flex items-center gap-2 rounded-card border border-deny/40 bg-deny/5 px-3 py-2 text-xs text-deny"
      >
        <AlertTriangle size={14} />
        选择器语法不可解析——将不授予任何资源
      </p>
    );
  }

  // 异常 B: empty set — amber FACT (write still legal, grants nothing).
  if (data.expanded_resources.length === 0) {
    return (
      <p
        className="flex items-center gap-2 rounded-card border border-warn/40 bg-warn/5 px-3 py-2 text-xs text-warn"
        data-testid="expansion-empty"
      >
        <AlertTriangle size={14} />
        展开为 0 个资源（无匹配标签）
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-2 text-xs">
      <p className="text-text-muted">
        当前匹配{' '}
        <span className="font-medium text-text" data-testid="expansion-count">
          {data.expanded_resources.length}
        </span>{' '}
        个资源：
      </p>
      <div className="flex flex-wrap gap-1">
        {data.expanded_resources.map((code) => (
          <ResourceCodeBadge key={code} code={code} />
        ))}
      </div>
      {data.grants.length > 0 && (
        <details className="mt-1">
          <summary className="cursor-pointer text-text-muted hover:text-text">
            在该 Role 下将授予的 (资源×动词) 预览
          </summary>
          <GrantMatrix grants={data.grants} />
        </details>
      )}
    </div>
  );
}

function GrantMatrix({ grants }: { grants: PreviewGrantCell[] }) {
  // Group cells by resource for a compact per-resource verb list.
  const byResource = new Map<string, PreviewGrantCell[]>();
  for (const cell of grants) {
    const list = byResource.get(cell.resource) ?? [];
    list.push(cell);
    byResource.set(cell.resource, list);
  }
  return (
    <ul className="mt-2 flex flex-col gap-1">
      {[...byResource.entries()].map(([resource, cells]) => (
        <li key={resource} className="flex flex-wrap items-center gap-1">
          <ResourceCodeBadge code={resource} />
          <span className="text-text-muted">:</span>
          {cells.map((cell) => (
            <span
              key={cell.capability}
              className="inline-flex items-center gap-1"
              title={cell.tier ? `tier ${cell.tier}` : undefined}
            >
              <CapabilityBadge capability={cell.capability} />
              {cell.tier && (
                <span className="font-mono text-text-muted">{cell.tier}</span>
              )}
            </span>
          ))}
        </li>
      ))}
    </ul>
  );
}
