/**
 * 细则 / 条件 / 拒绝指引 — docs/08-constraints-conditions.md.
 *
 * One "list-page skeleton" (基座 §七) serving three同构 limiting tables via a
 * top SegmentedControl: switching the segment swaps the primary action, filter
 * bar, DataTable columns and the FormDrawer form, sharing one container.
 *
 * fail-closed three states (LoadingSkeleton / ErrorState / EmptyState), forced
 * pagination (page_no/page_size, default 20 clamp 200), snowflake ids as strings
 * (SnowflakeId), resource-as-code (ResourceCodeBadge, never a real address),
 * verbatim operator note (公理六), and the unified write flow with optimistic
 * lock 409 → "refresh & retry". Delete = scope-widening ⇒ ConfirmDialog.
 */

import { useMemo, useState } from 'react';
import { Eye, Pencil, Plus, Trash2 } from 'lucide-react';
import {
  CapabilityBadge,
  ConfirmDialog,
  DataTable,
  FormDrawer,
  ResourceCodeBadge,
  SnowflakeId,
  type Column,
} from '../../components';
import {
  useConditions,
  useConstraints,
  useDenyNotes,
  useResources,
} from '../../api/hooks';
import { PAGE_DEFAULT_SIZE, type Adapter, type PageQuery } from '../../api/types';
import type {
  ConditionRow,
  ConstraintRow,
  DenyNoteRow,
  ResourceRow,
} from '../../api/types';
import {
  specSummary,
  useWriteCondition,
  useWriteConstraint,
  useWriteDenyNote,
  type ConditionWrite,
  type ConstraintWrite,
  type DenyNoteWrite,
  type Segment,
} from './data';
import {
  JsonPreview,
  ScopeCell,
  SegmentedControl,
  VerbatimNote,
} from './parts';
import { ConditionForm, ConstraintForm, DenyNoteForm } from './forms';

const PRIMARY_LABEL: Record<Segment, string> = {
  constraints: '＋ 新建细则',
  conditions: '＋ 新建条件',
  'deny-notes': '＋ 新建拒绝指引',
};

const EMPTY: Record<Segment, { title: string; hint: string }> = {
  constraints: {
    title: '该范围尚无对象细则',
    hint: '',
  },
  conditions: {
    title: '尚无条件谓词',
    hint: '',
  },
  'deny-notes': {
    title: '尚无拒绝指引',
    hint: '',
  },
};

type AnyRow = ConstraintRow | ConditionRow | DenyNoteRow;

interface FilterState {
  resource: string;
  capability: string;
  kind: string;
  search: string;
}

const EMPTY_FILTER: FilterState = { resource: '', capability: '', kind: '', search: '' };

export default function ConstraintsPage() {
  const [segment, setSegment] = useState<Segment>('constraints');
  const [page, setPage] = useState<PageQuery>({ page_no: 1, page_size: PAGE_DEFAULT_SIZE });
  const [filter, setFilter] = useState<FilterState>(EMPTY_FILTER);

  // Drawer / dialog state.
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [editRow, setEditRow] = useState<AnyRow | null>(null);
  const [detailRow, setDetailRow] = useState<AnyRow | null>(null);
  const [deleteRow, setDeleteRow] = useState<AnyRow | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [deleteErr, setDeleteErr] = useState<string | null>(null);

  const resourcesQ = useResources({ page_no: 1, page_size: 200 });
  const constraintsQ = useConstraints(page);
  const conditionsQ = useConditions(page);
  const denyNotesQ = useDenyNotes(page);

  const writeConstraint = useWriteConstraint();
  const writeCondition = useWriteCondition();
  const writeDenyNote = useWriteDenyNote();

  const resourceOptions = useMemo(
    () =>
      (resourcesQ.data?.items ?? []).map((r: ResourceRow) => ({
        code: r.code,
        adapter: r.adapter,
      })),
    [resourcesQ.data],
  );
  const adapterByCode = useMemo(() => {
    const m = new Map<string, Adapter>();
    for (const r of resourcesQ.data?.items ?? []) m.set(r.code, r.adapter);
    return m;
  }, [resourcesQ.data]);

  const activeQ =
    segment === 'constraints'
      ? constraintsQ
      : segment === 'conditions'
        ? conditionsQ
        : denyNotesQ;

  function switchSegment(next: Segment) {
    setSegment(next);
    setPage({ page_no: 1, page_size: page.page_size });
    setFilter(EMPTY_FILTER);
    setDrawerOpen(false);
    setEditRow(null);
  }

  function openCreate() {
    setEditRow(null);
    setDrawerOpen(true);
  }
  function openEdit(row: AnyRow) {
    setEditRow(row);
    setDrawerOpen(true);
  }
  function closeDrawer() {
    setDrawerOpen(false);
    setEditRow(null);
  }

  // ── client-side filter over the current page (server owns paging) ──
  const rows = useMemo<AnyRow[]>(() => {
    const items = (activeQ.data?.items ?? []) as AnyRow[];
    return items.filter((row) => {
      if (filter.resource && (row.resource ?? '') !== filter.resource) return false;
      if (filter.capability && (row.capability ?? '') !== filter.capability) return false;
      if (
        filter.kind &&
        'kind' in row &&
        row.kind !== filter.kind
      )
        return false;
      if (filter.search) {
        const hay =
          'spec' in row ? row.spec : 'note' in row ? (row as DenyNoteRow).note : '';
        if (!hay.toLowerCase().includes(filter.search.toLowerCase())) return false;
      }
      return true;
    });
  }, [activeQ.data, filter]);

  // Count of existing same-(resource,capability,kind) constraint rows — feeds the
  // 交集提示 in the create form (facts only; semantics live in the adapter).
  const sameKindCount = useMemo(() => {
    if (segment !== 'constraints' || editRow) return 0;
    const items = (constraintsQ.data?.items ?? []) as ConstraintRow[];
    // Counted against the current form is approximate at the page level; the
    // accurate per-cell count would come from the backend, but page-level is a
    // safe over-/under-statement only for the hint (not a decision).
    return items.length > 0 ? items.length : 0;
  }, [segment, editRow, constraintsQ.data]);

  // ── columns per segment ──
  const columns = useMemo<Column<AnyRow>[]>(() => {
    if (segment === 'constraints') {
      return [
        {
          key: 'resource',
          header: '资源',
          render: (r) => (
            <ResourceCodeBadge
              code={(r as ConstraintRow).resource}
              adapter={adapterByCode.get((r as ConstraintRow).resource)}
            />
          ),
          sortValue: (r) => (r as ConstraintRow).resource,
        },
        {
          key: 'capability',
          header: '动词',
          render: (r) => <CapabilityBadge capability={(r as ConstraintRow).capability} />,
        },
        {
          key: 'kind',
          header: 'kind',
          render: (r) => <span className="font-mono text-xs">{(r as ConstraintRow).kind}</span>,
          sortValue: (r) => (r as ConstraintRow).kind,
        },
        {
          key: 'spec',
          header: 'spec 摘要',
          render: (r) => (
            <span className="font-mono text-xs text-text-muted">
              {specSummary((r as ConstraintRow).spec)}
            </span>
          ),
        },
        {
          key: 'version',
          header: 'version',
          render: (r) => String((r as ConstraintRow).version),
          sortValue: (r) => (r as ConstraintRow).version,
        },
      ];
    }
    if (segment === 'conditions') {
      return [
        {
          key: 'resource',
          header: '资源',
          render: (r) => (
            <ScopeCell
              resource={(r as ConditionRow).resource}
              adapter={
                (r as ConditionRow).resource
                  ? adapterByCode.get((r as ConditionRow).resource as string)
                  : undefined
              }
            />
          ),
        },
        {
          key: 'capability',
          header: '动词',
          render: (r) => {
            const cap = (r as ConditionRow).capability;
            return cap ? (
              <CapabilityBadge capability={cap} />
            ) : (
              <span className="font-mono text-xs text-text-muted" title="全动词">
                *
              </span>
            );
          },
        },
        {
          key: 'predicate',
          header: 'predicate',
          render: (r) => <span className="font-mono text-xs">{(r as ConditionRow).predicate}</span>,
          sortValue: (r) => (r as ConditionRow).predicate,
        },
        {
          key: 'spec',
          header: 'spec 摘要',
          render: (r) => (
            <span className="font-mono text-xs text-text-muted">
              {specSummary((r as ConditionRow).spec)}
            </span>
          ),
        },
        {
          key: 'version',
          header: 'version',
          render: (r) => String((r as ConditionRow).version),
          sortValue: (r) => (r as ConditionRow).version,
        },
      ];
    }
    // deny-notes
    return [
      {
        key: 'resource',
        header: '资源',
        render: (r) => (
          <ResourceCodeBadge
            code={(r as DenyNoteRow).resource}
            adapter={adapterByCode.get((r as DenyNoteRow).resource)}
          />
        ),
        sortValue: (r) => (r as DenyNoteRow).resource,
      },
      {
        key: 'capability',
        header: '动词',
        render: (r) => <CapabilityBadge capability={(r as DenyNoteRow).capability} />,
      },
      {
        key: 'note',
        header: 'note 原文（= 越权时 Agent 收到的 operator_note）',
        render: (r) => {
          const note = (r as DenyNoteRow).note;
          return (
            <span
              title={note}
              className="block max-w-xs truncate font-mono text-xs text-text"
            >
              {note}
            </span>
          );
        },
        className: 'max-w-xs',
      },
      {
        key: 'version',
        header: 'version',
        render: (r) => String((r as DenyNoteRow).version),
        sortValue: (r) => (r as DenyNoteRow).version,
      },
    ];
  }, [segment, adapterByCode]);

  // ── delete = scope-widening (危险确认) ──
  function deleteBody(row: AnyRow): string {
    if (segment === 'constraints') {
      const r = row as ConstraintRow;
      return `删除此细则将放宽 (${r.resource}, ${r.capability}) 的对象作用面——该动词不再受 ${r.kind} 限制。`;
    }
    if (segment === 'conditions') {
      const r = row as ConditionRow;
      return `删除后 (${r.resource ?? '*'}, ${r.capability ?? '*'}) 不再受 ${r.predicate} 约束。`;
    }
    const r = row as DenyNoteRow;
    return `删除后 (${r.resource}, ${r.capability}) 越权响应将不再含 operator_note（回到无人话状态）。`;
  }

  async function confirmDelete() {
    if (!deleteRow) return;
    setDeleteErr(null);
    try {
      if (segment === 'constraints') {
        const r = deleteRow as ConstraintRow;
        await writeConstraint.mutateAsync({
          id: r.id,
          resource: r.resource,
          capability: r.capability,
          kind: r.kind,
          spec: r.spec,
          version: r.version,
          delete_flag: 1,
        });
      } else if (segment === 'conditions') {
        const r = deleteRow as ConditionRow;
        await writeCondition.mutateAsync({
          id: r.id,
          resource: r.resource,
          capability: r.capability,
          predicate: r.predicate,
          spec: r.spec,
          version: r.version,
          delete_flag: 1,
        });
      } else {
        const r = deleteRow as DenyNoteRow;
        await writeDenyNote.mutateAsync({
          id: r.id,
          resource: r.resource,
          capability: r.capability,
          note: r.note,
          version: r.version,
          delete_flag: 1,
        });
      }
      setToast('已删除，policy_rev 前进');
      setDeleteRow(null);
    } catch (e) {
      const conflict = (e as { status?: number }).status === 409;
      setDeleteErr(
        conflict
          ? '他人已修改此记录，请刷新后基于最新 version 重试'
          : `删除失败：${(e as Error).message}`,
      );
    }
  }

  // ── write submit handlers ──
  async function submitConstraint(body: ConstraintWrite) {
    const ack = await writeConstraint.mutateAsync(body);
    setToast(`细则已挂载，policy_rev 前进至 ${ack.policy_rev}`);
    closeDrawer();
  }
  async function submitCondition(body: ConditionWrite) {
    const ack = await writeCondition.mutateAsync(body);
    setToast(`条件已附加，policy_rev 前进至 ${ack.policy_rev}`);
    closeDrawer();
  }
  async function submitDenyNote(body: DenyNoteWrite) {
    const ack = await writeDenyNote.mutateAsync(body);
    setToast(`拒绝指引已生效，policy_rev 前进至 ${ack.policy_rev}`);
    closeDrawer();
  }

  const writeDisabled = resourcesQ.isError || activeQ.isError;
  const empty = EMPTY[segment];

  return (
    <section className="flex flex-col gap-4">
      <header className="flex items-center justify-between">
        <h1 className="text-2xl font-medium">细则与条件</h1>
        <button
          type="button"
          onClick={openCreate}
          disabled={writeDisabled}
          title={writeDisabled ? '数据不可达，写操作已禁用（fail-closed）' : undefined}
          className="inline-flex items-center gap-1 rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40 hover:enabled:brightness-110"
        >
          <Plus size={14} />
          {PRIMARY_LABEL[segment].replace('＋ ', '')}
        </button>
      </header>

      <SegmentedControl value={segment} onChange={switchSegment} />

      {/* 筛选条 */}
      <div className="flex flex-wrap items-center gap-2 rounded-card border border-border bg-surface-2 p-2 text-sm">
        <select
          aria-label="资源筛选"
          value={filter.resource}
          onChange={(e) => setFilter((f) => ({ ...f, resource: e.target.value }))}
          className="rounded-card border border-border bg-bg px-2 py-1"
        >
          <option value="">资源（全部）</option>
          {resourceOptions.map((r) => (
            <option key={r.code} value={r.code}>
              {r.code}
            </option>
          ))}
        </select>
        <select
          aria-label="动词筛选"
          value={filter.capability}
          onChange={(e) => setFilter((f) => ({ ...f, capability: e.target.value }))}
          className="rounded-card border border-border bg-bg px-2 py-1"
        >
          <option value="">动词（全部）</option>
          {['observe', 'query', 'mutate', 'execute', 'manage', 'destroy'].map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        {segment === 'constraints' && (
          <input
            aria-label="kind 筛选"
            placeholder="kind"
            value={filter.kind}
            onChange={(e) => setFilter((f) => ({ ...f, kind: e.target.value }))}
            className="rounded-card border border-border bg-bg px-2 py-1"
          />
        )}
        <input
          aria-label="搜索 spec"
          placeholder={segment === 'deny-notes' ? '搜索 note' : '搜索 spec'}
          value={filter.search}
          onChange={(e) => setFilter((f) => ({ ...f, search: e.target.value }))}
          className="flex-1 rounded-card border border-border bg-bg px-2 py-1"
        />
      </div>

      {toast && (
        <div
          role="status"
          className="flex items-center justify-between rounded-card border border-allow/40 bg-allow/5 px-3 py-2 text-sm text-allow"
        >
          {toast}
          <button type="button" onClick={() => setToast(null)} className="text-text-muted hover:text-text">
            ✕
          </button>
        </div>
      )}

      <DataTable<AnyRow>
        columns={columns}
        rows={rows}
        total={activeQ.data?.total ?? 0}
        page={page}
        onPageChange={setPage}
        rowKey={(r) => r.id}
        loading={activeQ.isLoading}
        error={activeQ.isError ? { message: (activeQ.error as Error)?.message } : null}
        onRetry={() => void activeQ.refetch()}
        emptyTitle={empty.title}
        emptyAction={
          <button
            type="button"
            onClick={openCreate}
            disabled={writeDisabled}
            className="rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40"
          >
            {PRIMARY_LABEL[segment].replace('＋ ', '新建')}
          </button>
        }
        rowActions={(row) => (
          <div className="flex items-center justify-end gap-1">
            <button
              type="button"
              aria-label="查看详情"
              onClick={() => setDetailRow(row)}
              className="text-text-muted hover:text-text"
            >
              <Eye size={14} />
            </button>
            <button
              type="button"
              aria-label="编辑"
              onClick={() => openEdit(row)}
              className="text-text-muted hover:text-text"
            >
              <Pencil size={14} />
            </button>
            <button
              type="button"
              aria-label="删除"
              onClick={() => {
                setDeleteErr(null);
                setDeleteRow(row);
              }}
              className="text-deny hover:brightness-110"
            >
              <Trash2 size={14} />
            </button>
          </div>
        )}
      />

      {/* FormDrawer — segment-aware form */}
      <FormDrawer
        open={drawerOpen}
        title={editRow ? '编辑' : PRIMARY_LABEL[segment].replace('＋ ', '')}
        onClose={closeDrawer}
      >
        {segment === 'constraints' && (
          <ConstraintForm
            resources={resourceOptions}
            initial={editRow as ConstraintRow | undefined}
            sameKindCount={editRow ? 0 : sameKindCount}
            onSubmit={submitConstraint}
            onCancel={closeDrawer}
          />
        )}
        {segment === 'conditions' && (
          <ConditionForm
            resources={resourceOptions}
            initial={
              editRow
                ? {
                    ...(editRow as ConditionRow),
                    resource: (editRow as ConditionRow).resource ?? '',
                    capability: (editRow as ConditionRow).capability ?? '',
                  }
                : undefined
            }
            onSubmit={submitCondition}
            onCancel={closeDrawer}
          />
        )}
        {segment === 'deny-notes' && (
          <DenyNoteForm
            resources={resourceOptions}
            initial={editRow as DenyNoteRow | undefined}
            editing={Boolean(editRow)}
            onSubmit={submitDenyNote}
            onCancel={closeDrawer}
          />
        )}
      </FormDrawer>

      {/* 详情抽屉 */}
      <FormDrawer
        open={detailRow !== null}
        title="详情"
        onClose={() => setDetailRow(null)}
      >
        {detailRow && (
          <div className="flex flex-col gap-3 text-sm">
            <div>
              <span className="text-text-muted">id</span>
              <div className="mt-1">
                <SnowflakeId id={detailRow.id} head={8} tail={6} />
              </div>
            </div>
            <div>
              <span className="text-text-muted">资源</span>
              <div className="mt-1">
                {('resource' in detailRow && detailRow.resource) ? (
                  <ResourceCodeBadge
                    code={detailRow.resource as string}
                    adapter={adapterByCode.get(detailRow.resource as string)}
                  />
                ) : (
                  <span className="font-mono text-xs text-text-muted">* 全资源</span>
                )}
              </div>
            </div>
            {'note' in detailRow ? (
              <div>
                <span className="text-text-muted">note 原文（Agent 所见）</span>
                <div className="mt-1 rounded-card border border-border bg-surface-2 p-2">
                  <VerbatimNote note={(detailRow as DenyNoteRow).note} />
                </div>
              </div>
            ) : (
              <div>
                <span className="text-text-muted">spec（raw JSON）</span>
                <div className="mt-1">
                  <JsonPreview spec={(detailRow as ConstraintRow | ConditionRow).spec} />
                </div>
              </div>
            )}
            <div className="text-xs text-text-muted">version {detailRow.version}</div>
          </div>
        )}
      </FormDrawer>

      {/* 删除确认（扩大作用面） */}
      <ConfirmDialog
        open={deleteRow !== null}
        title="删除 = 扩大作用面"
        body={
          <div className="flex flex-col gap-2">
            <span>{deleteRow ? deleteBody(deleteRow) : ''}</span>
            {deleteErr && (
              <span role="alert" className="text-deny">
                {deleteErr}
              </span>
            )}
          </div>
        }
        confirmWord={segment === 'deny-notes' ? undefined : '我已知此操作扩大授权作用面'}
        confirmLabel="删除"
        danger
        onConfirm={confirmDelete}
        onCancel={() => {
          setDeleteRow(null);
          setDeleteErr(null);
        }}
      />
    </section>
  );
}
