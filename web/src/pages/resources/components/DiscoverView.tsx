import { useMemo, useState } from 'react';
import { AlertTriangle, ArrowUpRight } from 'lucide-react';
import {
  CapabilityBadge,
  ErrorState,
  InlineSpinner,
} from '../../../components';
import type { CapabilitySurface, ResourceRow } from '../../../api/types';

/**
 * Capability-surface probe view (设计 §2.3 / §4.3). Lives inside the drawer as a
 * secondary view. Top banner makes the boundary explicit: DISCOVERY ≠
 * AUTHORIZATION — unselected objects are denied by default (公理一). Selecting
 * objects only produces inputs for the constraints page (08); nothing is
 * authorized here. Errors are fail-closed: no fabricated objects, scrubbed text.
 */
export function DiscoverView({
  resource,
  surface,
  loading,
  error,
  onRetry,
  onConfigure,
}: {
  resource: ResourceRow;
  surface: CapabilitySurface | undefined;
  loading: boolean;
  error: Error | null;
  onRetry: () => void;
  /** Hand the selected objects to the constraints page (08). */
  onConfigure: (objects: string[]) => void;
}) {
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const objects = useMemo(() => surface?.objects ?? [], [surface]);

  function toggle(obj: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(obj)) next.delete(obj);
      else next.add(obj);
      return next;
    });
  }

  return (
    <div className="flex flex-col gap-4">
      <div
        role="note"
        className="flex items-start gap-2 rounded-card border border-warn/40 bg-warn/5 px-3 py-2 text-xs text-warn"
      >
        <AlertTriangle size={14} className="mt-0.5 shrink-0" />
        <span>
          发现 ≠ 授权 —— 探测只列「有哪些对象」，未圈选对象一律默认拒绝。
        </span>
      </div>

      <p className="text-xs text-text-muted">
        经 <span className="font-mono">{resource.transport}</span> 真实连接
        <span className="font-mono"> {resource.code}</span> 探测 ·
        CapabilitySurface（只读事实）
      </p>

      {loading ? (
        <div role="status" aria-label="探测中" className="text-sm">
          <InlineSpinner label={`经 ${resource.transport} 连接探测中…`} />
        </div>
      ) : error ? (
        // Fail-closed: scrubbed gap text, no real port/IP, no fake objects.
        <ErrorState
          title="探测失败"
          message={error.message}
          onRetry={onRetry}
        />
      ) : surface ? (
        <>
          <section>
            <h3 className="mb-2 text-sm font-medium">探得能力 capabilities</h3>
            {surface.capabilities.length === 0 ? (
              <p className="text-xs text-text-muted">（无）</p>
            ) : (
              <div className="flex flex-wrap gap-1">
                {surface.capabilities.map((c) => (
                  <CapabilityBadge key={c} capability={c} />
                ))}
              </div>
            )}
          </section>

          <section>
            <h3 className="mb-2 text-sm font-medium">
              探得对象 objects（圈选纳入授权细则）
            </h3>
            {objects.length === 0 ? (
              <p className="text-xs text-text-muted">（无对象）</p>
            ) : (
              <ul className="flex flex-col gap-1">
                {objects.map((obj) => (
                  <li key={obj}>
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={selected.has(obj)}
                        onChange={() => toggle(obj)}
                        aria-label={`选择对象 ${obj}`}
                      />
                      <span className="font-mono text-xs">{obj}</span>
                    </label>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <div className="flex items-center justify-between border-t border-border pt-3">
            <span className="text-xs text-text-muted">已选 {selected.size} 项</span>
            <button
              type="button"
              disabled={selected.size === 0}
              onClick={() => onConfigure([...selected])}
              className="inline-flex items-center gap-1 rounded-card border border-border px-3 py-1.5 text-sm text-info hover:enabled:bg-surface-2 disabled:opacity-40"
            >
              以选中对象去配置细则（08）
              <ArrowUpRight size={14} />
            </button>
          </div>
        </>
      ) : null}
    </div>
  );
}
