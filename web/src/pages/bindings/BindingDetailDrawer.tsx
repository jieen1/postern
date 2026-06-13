/**
 * BindingDetailDrawer — read-only "查看展开" drawer (07-bindings.md §2.3).
 * Shows the binding metadata + the daemon-reported expansion result. Empty set
 * renders an EmptyState fact ("展开为 0 个资源（无匹配标签）"), not an error.
 */

import { FormDrawer, SnowflakeId, EmptyState, ResourceCodeBadge } from '@/components';
import type { Binding } from '@/api/types';
import { JsonViewer } from './JsonViewer';
import { parseResourceSpec } from './scope';

export function BindingDetailDrawer({
  binding,
  onClose,
}: {
  binding: Binding | null;
  onClose: () => void;
}) {
  if (!binding) return null;
  const resources = binding.expanded_resources;
  return (
    <FormDrawer open title="绑定展开详情" onClose={onClose}>
      <div className="flex flex-col gap-4 text-sm">
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2">
          <dt className="text-text-muted">id</dt>
          <dd>
            <SnowflakeId id={binding.id} />
          </dd>
          <dt className="text-text-muted">principal</dt>
          <dd className="font-mono">{binding.principal}</dd>
          <dt className="text-text-muted">role</dt>
          <dd className="font-mono">{binding.role}</dd>
          <dt className="text-text-muted">scope 类型</dt>
          <dd className="font-mono">{binding.scope_kind}</dd>
          <dt className="text-text-muted">version</dt>
          <dd className="font-mono">{binding.version}</dd>
        </dl>

        <div>
          <p className="mb-1 font-medium">scope spec</p>
          {binding.scope_kind === 'selector' ? (
            <JsonViewer value={binding.scope_spec} label="scope spec" />
          ) : (
            <div className="flex flex-wrap gap-1">
              {parseResourceSpec(binding.scope_spec).map((code) => (
                <ResourceCodeBadge key={code} code={code} />
              ))}
            </div>
          )}
        </div>

        <div>
          <p className="mb-1 font-medium">展开结果（当前快照）</p>
          {resources.length === 0 ? (
            <EmptyState
              title="展开为 0 个资源（无匹配标签）"
              hint="该 binding 当前不授予任何资源"
            />
          ) : (
            <div className="flex flex-wrap gap-1" data-testid="detail-expanded">
              {resources.map((code) => (
                <ResourceCodeBadge key={code} code={code} />
              ))}
            </div>
          )}
        </div>
      </div>
    </FormDrawer>
  );
}
