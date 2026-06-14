/**
 * Roles page (06-roles.md) — the信任等级 (verb-set) rule editor.
 *
 * Base list-page skeleton: title + primary action + filter bar + read-only
 * LadderGraph + DataTable (forced pagination) + row actions + FormDrawer. Write
 * flow goes through RoleFormDrawer (RHF+Zod → summary → submit); delete goes
 * through the danger ConfirmDialog. All three states are fail-closed: loading →
 * skeleton, error → ErrorState (no fake roles, no stale ladder), empty →
 * EmptyState. `admin` has no entry anywhere (structural absence).
 *
 * SPA holds ZERO security logic — the list's effective verb sets are the
 * daemon-reported facts, never recomputed here.
 */

import { useMemo, useState } from 'react';
import { Plus, Search } from 'lucide-react';
import {
  ConfirmDialog,
  DataTable,
  ErrorState,
  LoadingSkeleton,
  SnowflakeId,
  type Column,
} from '../../components';
import { useRoles } from '../../api/hooks';
import { ConflictError } from '../../api/client';
import {
  CAPABILITIES,
  type Capability,
  type PageQuery,
  type Role,
} from '../../api/types';
import { LadderGraph } from './LadderGraph';
import { CapabilityActionBadge } from './CapabilityActionBadge';
import { RoleRowActions } from './RoleRowActions';
import { RoleFormDrawer } from './RoleFormDrawer';
import { useRoleWrite } from './useRoleWrite';

type RoleTypeFilter = 'all' | 'ladder' | 'narrow';

export function RolesPage() {
  const [page, setPage] = useState<PageQuery>({ page_no: 1, page_size: 20 });
  const [nameFilter, setNameFilter] = useState('');
  const [verbFilter, setVerbFilter] = useState<Capability | ''>('');
  const [typeFilter, setTypeFilter] = useState<RoleTypeFilter>('all');

  const [drawerOpen, setDrawerOpen] = useState(false);
  const [editing, setEditing] = useState<Role | null>(null);
  const [deleting, setDeleting] = useState<Role | null>(null);
  const [deleteAcked, setDeleteAcked] = useState(false);
  const [banner, setBanner] = useState<
    { kind: 'ok'; policyRev: string } | { kind: 'err'; message: string } | null
  >(null);

  const query = useRoles(page);
  const del = useRoleWrite();

  const allRoles = useMemo(() => query.data?.items ?? [], [query.data]);

  // Client-side filtering WITHIN the current page only (server owns paging).
  const filteredRoles = useMemo(() => {
    return allRoles.filter((r) => {
      if (nameFilter && !r.name.toLowerCase().includes(nameFilter.toLowerCase())) {
        return false;
      }
      if (verbFilter && !r.effective.some((rc) => rc.capability === verbFilter)) {
        return false;
      }
      if (typeFilter === 'ladder' && r.inherits_from.length === 0) return false;
      if (typeFilter === 'narrow' && r.inherits_from.length > 0) return false;
      return true;
    });
  }, [allRoles, nameFilter, verbFilter, typeFilter]);

  function openCreate() {
    setEditing(null);
    setDrawerOpen(true);
  }

  function openEdit(role: Role) {
    setEditing(role);
    setDrawerOpen(true);
  }

  function confirmDelete() {
    if (!deleting) return;
    const target = deleting;
    del.mutate(
      { id: target.id, name: target.name, capabilities: [], inherits_from: [], version: target.version, delete_flag: 1 },
      {
        onSuccess: (ack) => {
          setDeleting(null);
          setDeleteAcked(false);
          setBanner({ kind: 'ok', policyRev: ack.policy_rev });
        },
        onError: (err) => {
          setDeleting(null);
          setDeleteAcked(false);
          setBanner({
            kind: 'err',
            message:
              err instanceof ConflictError
                ? '他人已改或该角色已被删除，请刷新后重试（409）。'
                : err.message,
          });
        },
      },
    );
  }

  const columns: Column<Role>[] = [
    {
      key: 'name',
      header: '名称',
      render: (r) => <span className="font-medium">{r.name}</span>,
      sortValue: (r) => r.name,
    },
    {
      key: 'id',
      header: 'id',
      render: (r) => <SnowflakeId id={r.id} />,
    },
    {
      key: 'effective',
      header: '有效动词集',
      render: (r) => (
        <span className="flex flex-wrap gap-1">
          {r.effective.length === 0 ? (
            <span className="text-text-muted">—</span>
          ) : (
            r.effective.map((rc) => (
              <CapabilityActionBadge key={rc.capability} capability={rc.capability} action={rc.action} />
            ))
          )}
        </span>
      ),
    },
    {
      key: 'inherits',
      header: '继承自',
      render: (r) =>
        r.inherits_from.length === 0 ? (
          <span className="text-text-muted">—</span>
        ) : (
          <span className="flex flex-wrap gap-1 font-mono text-xs">
            {r.inherits_from.join(' · ')}
          </span>
        ),
    },
    {
      key: 'version',
      header: 'ver',
      render: (r) => <span className="font-mono text-xs">{r.version}</span>,
      sortValue: (r) => r.version,
    },
  ];

  return (
    <section className="flex flex-col gap-4">
      <header className="flex items-start justify-between">
        <div>
          <h1 className="text-2xl font-medium">角色 Roles</h1>
        </div>
        <button
          type="button"
          onClick={openCreate}
          className="inline-flex items-center gap-1 rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
        >
          <Plus size={14} />
          新建角色
        </button>
      </header>

      {/* Write-result banner (success / failure). */}
      {banner?.kind === 'ok' && (
        <div role="status" className="rounded-card border border-allow/50 bg-allow/10 px-3 py-2 text-sm text-allow">
          角色已保存。
        </div>
      )}
      {banner?.kind === 'err' && (
        <div role="alert" className="rounded-card border border-deny/50 bg-deny/10 px-3 py-2 font-mono text-xs text-deny">
          {banner.message}
        </div>
      )}

      {/* LadderGraph — fail-closed: hidden during loading/error (no半截阶梯). */}
      {query.isLoading ? (
        <div className="rounded-card border border-border bg-surface p-4">
          <LoadingSkeleton rows={3} />
        </div>
      ) : query.isError ? (
        <ErrorState
          title="无法加载角色"
          message={(query.error as Error)?.message ?? '控制面不可达'}
          onRetry={() => query.refetch()}
        />
      ) : (
        <LadderGraph roles={allRoles} />
      )}

      {/* Filter bar. */}
      <div className="flex flex-wrap items-center gap-2 rounded-card border border-border bg-surface px-3 py-2">
        <label className="flex items-center gap-1 text-sm">
          <Search size={14} className="text-text-muted" />
          <input
            value={nameFilter}
            onChange={(e) => setNameFilter(e.target.value)}
            placeholder="名称…"
            aria-label="按名称筛选"
            className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
          />
        </label>
        <label className="flex items-center gap-1 text-sm">
          动词
          <select
            value={verbFilter}
            onChange={(e) => setVerbFilter(e.target.value as Capability | '')}
            aria-label="按动词筛选"
            className="rounded-card border border-border bg-surface px-2 py-1 text-sm"
          >
            <option value="">全部</option>
            {CAPABILITIES.map((cap) => (
              <option key={cap} value={cap}>
                {cap}
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-1 text-sm">
          类型
          <select
            value={typeFilter}
            onChange={(e) => setTypeFilter(e.target.value as RoleTypeFilter)}
            aria-label="按类型筛选"
            className="rounded-card border border-border bg-surface px-2 py-1 text-sm"
          >
            <option value="all">全部</option>
            <option value="ladder">阶梯</option>
            <option value="narrow">窄角色</option>
          </select>
        </label>
      </div>

      <DataTable
        columns={columns}
        rows={filteredRoles}
        total={query.data?.total ?? 0}
        page={page}
        onPageChange={setPage}
        rowKey={(r) => r.id}
        loading={query.isLoading}
        error={query.isError ? { message: (query.error as Error)?.message } : null}
        onRetry={() => query.refetch()}
        emptyTitle="尚无任何角色"
        rowActions={(role) => (
          <RoleRowActions role={role} onEdit={openEdit} onDelete={setDeleting} />
        )}
      />

      <RoleFormDrawer
        open={drawerOpen}
        editing={editing}
        roles={allRoles}
        onClose={() => setDrawerOpen(false)}
        onSaved={(policyRev) => {
          setDrawerOpen(false);
          setBanner({ kind: 'ok', policyRev });
        }}
      />

      <ConfirmDialog
        open={Boolean(deleting)}
        title="删除角色（逻辑删除·终态）"
        confirmLabel="删除"
        onCancel={() => {
          setDeleting(null);
          setDeleteAcked(false);
        }}
        onConfirm={() => {
          if (deleteAcked) confirmDelete();
        }}
        body={
          deleting && (
            <div className="flex flex-col gap-2">
              <p>{`角色「${deleting.name}」将被逻辑删除（不可恢复）。`}</p>
              <div className="flex flex-wrap gap-1">
                {deleting.effective.map((rc) => (
                  <CapabilityActionBadge key={rc.capability} capability={rc.capability} action={rc.action} />
                ))}
              </div>
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={deleteAcked}
                  onChange={(e) => setDeleteAcked(e.target.checked)}
                  aria-label="我已知晓影响"
                />
                我已知晓影响
              </label>
              {!deleteAcked && (
                <span className="text-xs text-text-muted">需勾选「我已知晓影响」后方可删除</span>
              )}
            </div>
          )
        }
      />
    </section>
  );
}

export default RolesPage;
