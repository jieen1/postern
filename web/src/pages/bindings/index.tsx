/**
 * 绑定 Bindings page (07-bindings.md).
 *
 * "Principal —绑定→ Role × Scope" as a queryable, creatable authorization-
 * jurisdiction list. List page unified skeleton: title + primary action +
 * filters + DataTable (forced pagination) + row actions + FormDrawer create.
 * Writes go through the unified flow (summary preview → confirm → invalidate
 * → success/error/409). All three states are fail-closed; the SPA holds ZERO
 * security logic — expansion and legality are the daemon's, rendered here.
 */

import { useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  Badge,
  ConfirmDialog,
  DataTable,
  ResourceCodeBadge,
  SnowflakeId,
  type Column,
} from '@/components';
import {
  useBindings,
  usePrincipals,
  useResources,
  useRoles,
} from '@/api/hooks';
import { ConflictError } from '@/api/client';
import type { Binding, PageQuery, ScopeKind } from '@/api/types';
import { JsonViewer } from './JsonViewer';
import { parseResourceSpec } from './scope';
import { CreateBindingDrawer } from './CreateBindingDrawer';
import { BindingDetailDrawer } from './BindingDetailDrawer';
import { useDeleteBinding } from './api';

const DEFAULT_PAGE: PageQuery = { page_no: 1, page_size: 20 };

interface Filters {
  principal: string;
  role: string;
  scopeKind: '' | ScopeKind;
  search: string;
}

const EMPTY_FILTERS: Filters = {
  principal: '',
  role: '',
  scopeKind: '',
  search: '',
};

export function BindingsPage() {
  const navigate = useNavigate();
  const [page, setPage] = useState<PageQuery>(DEFAULT_PAGE);
  const [filters, setFilters] = useState<Filters>(EMPTY_FILTERS);

  const [createOpen, setCreateOpen] = useState(false);
  const [detail, setDetail] = useState<Binding | null>(null);
  const [toDelete, setToDelete] = useState<Binding | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const bindingsQuery = useBindings(page);
  const principalsQuery = usePrincipals({ page_size: 200 });
  const rolesQuery = useRoles({ page_size: 200 });
  const resourcesQuery = useResources({ page_size: 200 });
  const del = useDeleteBinding();

  const principals = principalsQuery.data?.items ?? [];
  // `admin` is never offered as a grantable role (契约 §七); the daemon is the
  // real hard-deny, this is a front-end convenience filter only.
  const roles = (rolesQuery.data?.items ?? []).filter((r) => r.name !== 'admin');
  const resourceCodes = (resourcesQuery.data?.items ?? []).map((r) => r.code);

  const allRows = useMemo(
    () => bindingsQuery.data?.items ?? [],
    [bindingsQuery.data],
  );
  // Client-side filter within the current page (server owns paging); never
  // surfaces a hidden-row count (does not leak existence, §3.2).
  const rows = useMemo(
    () =>
      allRows.filter((b) => {
        if (filters.principal && b.principal !== filters.principal) return false;
        if (filters.role && b.role !== filters.role) return false;
        if (filters.scopeKind && b.scope_kind !== filters.scopeKind) return false;
        if (filters.search) {
          const q = filters.search.toLowerCase();
          const hay = `${b.principal} ${b.role} ${b.scope_spec}`.toLowerCase();
          if (!hay.includes(q)) return false;
        }
        return true;
      }),
    [allRows, filters],
  );

  function deleteSummary(b: Binding) {
    const where =
      b.expanded_resources.length > 0
        ? b.expanded_resources.join(', ')
        : '（当前无匹配资源）';
    return `将删除 binding ${b.id}（${b.principal} · ${b.role} · ${b.scope_kind} ${b.scope_spec}）。删除后 ${b.principal} 在 [${where}] 上由该绑定授予的授权随之消失（缩权方向）。`;
  }

  function onConfirmDelete() {
    if (!toDelete) return;
    del.mutate(
      { id: toDelete.id, version: toDelete.version },
      {
        onSuccess: () => {
          setToast('绑定已删除');
          setToDelete(null);
        },
        onError: (err) => {
          if (err instanceof ConflictError) {
            setToast('他人已改、请刷新重试（删除未生效）');
          } else {
            setToast(err instanceof Error ? err.message : '删除失败');
          }
          setToDelete(null);
        },
      },
    );
  }

  const columns: Column<Binding>[] = [
    {
      key: 'id',
      header: 'id',
      className: 'font-mono',
      render: (b) => <SnowflakeId id={b.id} />,
    },
    {
      key: 'principal',
      header: 'Principal',
      sortValue: (b) => b.principal,
      render: (b) => (
        <button
          type="button"
          onClick={() => navigate('/principals')}
          className="font-mono text-info hover:underline"
        >
          {b.principal}
        </button>
      ),
    },
    {
      key: 'role',
      header: 'Role',
      sortValue: (b) => b.role,
      render: (b) => (
        <button
          type="button"
          onClick={() => navigate('/roles')}
          className="font-mono text-info hover:underline"
        >
          {b.role}
        </button>
      ),
    },
    {
      key: 'scope',
      header: 'Scope',
      render: (b) => <ScopeCell binding={b} />,
    },
    {
      key: 'expanded',
      header: '展开',
      sortValue: (b) => b.expanded_resources.length,
      render: (b) => <ExpansionCount count={b.expanded_resources.length} />,
    },
    {
      key: 'version',
      header: 'ver',
      className: 'font-mono',
      sortValue: (b) => b.version,
      render: (b) => b.version,
    },
  ];

  return (
    <section className="flex flex-col gap-4">
      <header className="flex items-start justify-between">
        <div>
          <h1 className="text-2xl font-medium">绑定 Bindings</h1>
        </div>
        <button
          type="button"
          onClick={() => setCreateOpen(true)}
          className="rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
        >
          + 新建绑定
        </button>
      </header>

      <div className="flex flex-wrap items-center gap-2 rounded-card border border-border bg-surface p-3">
        <select
          aria-label="按 Principal 筛选"
          value={filters.principal}
          onChange={(e) =>
            setFilters((f) => ({ ...f, principal: e.target.value }))
          }
          className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
        >
          <option value="">全部 Principal</option>
          {principals.map((p) => (
            <option key={p.id} value={p.name}>
              {p.name}
            </option>
          ))}
        </select>
        <select
          aria-label="按 Role 筛选"
          value={filters.role}
          onChange={(e) => setFilters((f) => ({ ...f, role: e.target.value }))}
          className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
        >
          <option value="">全部 Role</option>
          {roles.map((r) => (
            <option key={r.id} value={r.name}>
              {r.name}
            </option>
          ))}
        </select>
        <select
          aria-label="按 Scope 类型筛选"
          value={filters.scopeKind}
          onChange={(e) =>
            setFilters((f) => ({
              ...f,
              scopeKind: e.target.value as Filters['scopeKind'],
            }))
          }
          className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
        >
          <option value="">全部 Scope 类型</option>
          <option value="selector">selector</option>
          <option value="resource">resource</option>
        </select>
        <input
          aria-label="搜索绑定"
          value={filters.search}
          onChange={(e) => setFilters((f) => ({ ...f, search: e.target.value }))}
          placeholder="搜索 principal/role/spec"
          className="flex-1 rounded-card border border-border bg-bg px-2 py-1 text-sm"
        />
      </div>

      {toast && (
        <div
          role="status"
          className="rounded-card border border-allow/40 bg-allow/5 px-3 py-2 text-sm text-allow"
        >
          {toast}
        </div>
      )}

      <DataTable
        columns={columns}
        rows={rows}
        total={bindingsQuery.data?.total ?? 0}
        page={page}
        onPageChange={setPage}
        rowKey={(b) => b.id}
        loading={bindingsQuery.isLoading}
        error={bindingsQuery.isError ? { message: errMessage(bindingsQuery.error) } : null}
        onRetry={() => bindingsQuery.refetch()}
        emptyTitle="还没有绑定"
        emptyAction={
          <button
            type="button"
            onClick={() => setCreateOpen(true)}
            className="rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
          >
            新建第一条绑定
          </button>
        }
        rowActions={(b) => (
          <div className="flex items-center justify-end gap-1">
            <button
              type="button"
              aria-label={`查看绑定展开 ${b.id}`}
              onClick={() => setDetail(b)}
              className="rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
            >
              展开
            </button>
            <button
              type="button"
              onClick={() => navigate('/grants')}
              className="rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
            >
              Grants
            </button>
            <button
              type="button"
              onClick={() => setToDelete(b)}
              className="rounded-card border border-border px-2 py-1 text-xs text-deny hover:bg-surface-2"
            >
              删除
            </button>
          </div>
        )}
      />

      <CreateBindingDrawer
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        principals={principals}
        roles={roles}
        resourceOptions={resourceCodes}
        onCreated={() => {
          setToast('绑定已创建');
        }}
      />

      <BindingDetailDrawer binding={detail} onClose={() => setDetail(null)} />

      <ConfirmDialog
        open={toDelete !== null}
        title="删除绑定"
        confirmWord="DELETE"
        confirmLabel="确认删除"
        body={toDelete ? deleteSummary(toDelete) : undefined}
        onConfirm={onConfirmDelete}
        onCancel={() => setToDelete(null)}
      />
    </section>
  );
}

function errMessage(error: unknown): string | undefined {
  return error instanceof Error ? error.message : undefined;
}

function ScopeCell({ binding }: { binding: Binding }) {
  if (binding.scope_kind === 'resource') {
    return (
      <div className="flex flex-wrap gap-1">
        {parseResourceSpec(binding.scope_spec).map((code) => (
          <ResourceCodeBadge key={code} code={code} />
        ))}
      </div>
    );
  }
  return (
    <div className="max-w-xs">
      <Badge className="mb-1 border-info/50 text-info">selector</Badge>
      <JsonViewer value={binding.scope_spec} label="selector spec" />
    </div>
  );
}

function ExpansionCount({ count }: { count: number }) {
  if (count === 0) {
    return (
      <Badge className="border-warn/50 text-warn" title="展开为 0 个资源（无匹配标签）">
        0 资源 · 无匹配
      </Badge>
    );
  }
  return <Badge className="border-border text-text-muted">{count} 资源</Badge>;
}

