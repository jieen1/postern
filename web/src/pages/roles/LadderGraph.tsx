/**
 * LadderGraph — read-only inheritance ladder (06-roles.md §五, page-local).
 *
 * Renders the standard rungs `observer ─inherits→ operator ─inherits→ maintainer`
 * with each rung's daemon-reported effective verb set (CapabilityBadge), plus a
 * "floating narrow roles" row for roles with no inheritance. ZERO computation:
 * it only displays the inheritance edges + effective sets the daemon returned.
 * A footnote states the fixed fact that `destroy` never enters any role.
 *
 * fail-closed: this component is never rendered while the list is loading or in
 * error — the page swaps it for the skeleton / error state so no half / stale
 * ladder is ever shown.
 */

import { ArrowRight } from 'lucide-react';
import { CapabilityBadge } from '../../components';
import type { Role } from '../../api/types';
import { CapabilityActionBadge } from './CapabilityActionBadge';

/** A rung is a role that inherits from something OR is named on a chain. We
 * order rungs by inheritance depth so the ladder reads left→right. */
function ladderDepth(role: Role, byName: Map<string, Role>): number {
  let depth = 0;
  let cursor: Role | undefined = role;
  const seen = new Set<string>();
  while (cursor && cursor.inherits_from.length > 0 && !seen.has(cursor.name)) {
    seen.add(cursor.name);
    depth += 1;
    const parentName: string | undefined = cursor.inherits_from[0];
    cursor = parentName ? byName.get(parentName) : undefined;
  }
  return depth;
}

export function LadderGraph({ roles }: { roles: Role[] }) {
  const byName = new Map(roles.map((r) => [r.name, r] as const));

  // Rungs = roles that participate in inheritance (have a parent, or are a parent).
  const rungs = roles
    .filter(
      (r) => r.inherits_from.length > 0 || roles.some((o) => o.inherits_from.includes(r.name)),
    )
    .sort((a, b) => ladderDepth(a, byName) - ladderDepth(b, byName));

  const narrow = roles.filter(
    (r) => r.inherits_from.length === 0 && !roles.some((o) => o.inherits_from.includes(r.name)),
  );

  return (
    <section
      aria-label="继承阶梯"
      className="flex flex-col gap-3 rounded-card border border-border bg-surface p-4"
    >
      <h2 className="text-sm font-medium text-text-muted">继承阶梯（只读）</h2>

      {rungs.length === 0 ? (
        <p className="text-xs text-text-muted">尚无阶梯角色</p>
      ) : (
        <ol className="flex flex-wrap items-stretch gap-2">
          {rungs.map((role, i) => (
            <li key={role.id} className="flex items-stretch gap-2">
              <div className="flex flex-col gap-1 rounded-card border border-border bg-surface-2 px-3 py-2">
                <span className="font-mono text-sm text-text">{role.name}</span>
                <span className="flex flex-wrap gap-1">
                  {role.effective.length === 0 ? (
                    <span className="text-xs text-text-muted">—</span>
                  ) : (
                    role.effective.map((rc) => (
                      <CapabilityActionBadge
                        key={rc.capability}
                        capability={rc.capability}
                        action={rc.action}
                      />
                    ))
                  )}
                </span>
              </div>
              {i < rungs.length - 1 && (
                <span
                  aria-label="inherits"
                  title="inherits"
                  className="flex items-center text-text-muted"
                >
                  <ArrowRight size={16} />
                </span>
              )}
            </li>
          ))}
        </ol>
      )}

      <div className="flex flex-wrap items-center gap-2 border-t border-border pt-3">
        <span className="text-xs text-text-muted">游离窄角色：</span>
        {narrow.length === 0 ? (
          <span className="text-xs text-text-muted">无</span>
        ) : (
          narrow.map((role) => (
            <span key={role.id} className="flex items-center gap-1">
              <span className="font-mono text-xs text-text">{role.name}</span>
              <span className="flex gap-0.5">
                {role.effective.map((rc) => (
                  <CapabilityBadge key={rc.capability} capability={rc.capability} />
                ))}
              </span>
            </span>
          ))
        )}
      </div>

      <p className="border-t border-border pt-2 text-xs text-text-muted">
        destroy 不进任何角色——经单格 + TTL 在授权矩阵显式授予。
      </p>
    </section>
  );
}
