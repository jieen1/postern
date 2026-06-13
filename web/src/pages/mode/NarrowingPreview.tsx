import { useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { useGrants } from '../../api/hooks';
import { CapabilityBadge } from '../../components/CapabilityBadge';
import { ResourceCodeBadge } from '../../components/ResourceCodeBadge';
import { EmptyState, ErrorState, LoadingSkeleton } from '../../components/States';
import { CAPABILITIES, type Capability, type Mode } from '../../api/types';

/**
 * Narrowing-effect preview (11-mode.md §五/§六/§七): expandable, read-only.
 * Reads GET /v1/grants (your_grants — the principal's OWN scope-bounded world)
 * and shows, per resource, the RBAC-original verbs vs the verbs that survive a
 * given mode. Verifies the narrowing effect WITHOUT recomputing authorization:
 * the per-mode admitted set is core's built-in constant table, applied here only
 * for the preview contrast.
 *
 * Fail-closed (§六): grants read failure → ErrorState (no fabricated rows);
 * missing data → EmptyState (no default-filling). Scope-bounded: out-of-scope /
 * nonexistent resources are simply absent (DENY_RESPONSE_SCOPE_BOUNDED).
 */

/** Verbs each mode admits — core constant-table semantics (Freeze admits none). */
const MODE_ADMITS: Record<Mode, ReadonlySet<Capability>> = {
  normal: new Set(CAPABILITIES),
  observe: new Set<Capability>(['observe', 'query']),
  maintain: new Set<Capability>(['observe', 'query', 'mutate', 'execute']),
  freeze: new Set<Capability>(),
};

function asCapabilities(names: string[]): Capability[] {
  // your_grants values are capability-name strings; keep only closed-set verbs
  // in canonical order (never invent a verb).
  return CAPABILITIES.filter((c) => names.includes(c));
}

export function NarrowingPreview({ mode }: { mode: Mode }) {
  const [open, setOpen] = useState(false);
  const { data, isLoading, isError, error, refetch } = useGrants();

  const admits = MODE_ADMITS[mode];
  const entries = data ? Object.entries(data.your_grants) : [];

  return (
    <section aria-label="收窄影响预览" className="rounded-card border border-border bg-surface">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 px-4 py-3 text-left text-sm font-medium hover:bg-surface-2"
      >
        {open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
        收窄影响预览（按 {mode} 模式对比 RBAC 原始动词 vs 剩余动词）
      </button>

      {open && (
        <div className="border-t border-border px-4 py-3">
          {isLoading ? (
            <LoadingSkeleton rows={3} />
          ) : isError ? (
            <ErrorState
              title="无法读取授权世界"
              message={error instanceof Error ? error.message : undefined}
              onRetry={() => void refetch()}
            />
          ) : entries.length === 0 ? (
            <EmptyState
              title="无可对比的授权数据"
              hint="当前作用域内无 your_grants 数据；按 fail-closed 显空，不补默认。"
            />
          ) : (
            <table className="w-full border-collapse text-sm">
              <thead>
                <tr className="border-b border-border text-left text-text-muted">
                  <th className="px-2 py-1 font-medium">资源</th>
                  <th className="px-2 py-1 font-medium">RBAC 原始动词</th>
                  <th className="px-2 py-1 font-medium">该模式后剩余</th>
                </tr>
              </thead>
              <tbody>
                {entries.map(([resource, names]) => {
                  const original = asCapabilities(names);
                  const remaining = original.filter((c) => admits.has(c));
                  return (
                    <tr key={resource} className="border-b border-border last:border-0 align-top">
                      <td className="px-2 py-1.5">
                        <ResourceCodeBadge code={resource} />
                      </td>
                      <td className="px-2 py-1.5">
                        <span className="flex flex-wrap gap-1">
                          {original.map((c) => (
                            <CapabilityBadge key={c} capability={c} />
                          ))}
                        </span>
                      </td>
                      <td className="px-2 py-1.5">
                        {remaining.length === 0 ? (
                          <span className="text-xs text-deny">全部被拒</span>
                        ) : (
                          <span className="flex flex-wrap gap-1">
                            {remaining.map((c) => (
                              <CapabilityBadge key={c} capability={c} />
                            ))}
                          </span>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>
      )}
    </section>
  );
}
