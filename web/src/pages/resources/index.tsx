import { useMemo, useState } from 'react';
import { Plus } from 'lucide-react';
import {
  Badge,
  CapabilityBadge,
  ConfirmDialog,
  DataTable,
  FormDrawer,
  ResourceCodeBadge,
  type Column,
} from '../../components';
import { useResources } from '../../api/hooks';
import { ConflictError } from '../../api/client';
import type {
  Adapter,
  Capability,
  PageQuery,
  ResourceRow,
} from '../../api/types';
import { ResourceForm } from './components/ResourceForm';
import { SummaryPreview } from './components/SummaryPreview';
import { DiscoverView } from './components/DiscoverView';
import { RowActions } from './components/RowActions';
import { declaresHighRisk, type ResourceFormValues } from './resourceForm';
import { buildResourcePayload } from './buildPayload';
import { usePostResource, useDiscoverResource } from './hooks/useResourceWrites';

/**
 * 资源 Resources (09-resources.md). List skeleton (title + primary action +
 * filters + forced-pagination DataTable + row actions) over `GET /v1/resources`;
 * the drawer hosts the sectioned access/edit form, its summary preview, or the
 * discover secondary view. Writes follow the unified flow: form → summary →
 * (danger) confirm → invalidate → success toast / fail (no view change) / 409.
 *
 * Fail-closed throughout: load → skeleton, error → ErrorState (no stale/fake
 * rows), empty → EmptyState. The library never shows real addresses, secret
 * hashes, or plaintext — only codenames and `vault://` references.
 */

const ALL = '__all__';

type DrawerMode =
  | { kind: 'closed' }
  | { kind: 'form'; editing: ResourceRow | null }
  | { kind: 'summary'; editing: ResourceRow | null; values: ResourceFormValues }
  | { kind: 'discover'; resource: ResourceRow };

interface Toast {
  tone: 'success' | 'error';
  text: string;
}

export function ResourcesPage() {
  const [page, setPage] = useState<PageQuery>({ page_no: 1, page_size: 20 });
  const [query, setQuery] = useState('');
  const [adapterFilter, setAdapterFilter] = useState<string>(ALL);
  const [statusFilter, setStatusFilter] = useState<string>(ALL);
  const [drawer, setDrawer] = useState<DrawerMode>({ kind: 'closed' });
  const [confirm, setConfirm] = useState<null | {
    title: string;
    body: React.ReactNode;
    confirmWord?: string;
    onConfirm: () => void;
  }>(null);
  const [toast, setToast] = useState<Toast | null>(null);

  const { data, isLoading, isError, error, refetch } = useResources(page);
  const postResource = usePostResource();
  const discover = useDiscoverResource();

  // Client-side filter within the current server page (server owns paging).
  const rows = useMemo(() => {
    const items = data?.items ?? [];
    return items.filter((r) => {
      if (adapterFilter !== ALL && r.adapter !== adapterFilter) return false;
      if (statusFilter !== ALL) {
        const enabled = statusFilter === 'enabled';
        if (r.enable_flag !== enabled) return false;
      }
      if (query.trim()) {
        const q = query.trim().toLowerCase();
        const inCode = r.code.toLowerCase().includes(q);
        const inLabel = r.labels.some(
          (l) => `${l.key}=${l.value}`.toLowerCase().includes(q),
        );
        if (!inCode && !inLabel) return false;
      }
      return true;
    });
  }, [data, adapterFilter, statusFilter, query]);

  function closeDrawer() {
    setDrawer({ kind: 'closed' });
  }

  // ── write flow: summary → (danger) confirm → POST ───────────────────────────
  function submitPayload(
    values: ResourceFormValues,
    editing: ResourceRow | null,
    enableFlag?: boolean,
  ) {
    const body = buildResourcePayload(values, editing, enableFlag);
    postResource.mutate(body, {
      onSuccess: () => {
        closeDrawer();
        setConfirm(null);
        setToast({
          tone: 'success',
          text: `资源 ${values.code} 已${editing ? '修订' : '接入'}`,
        });
      },
      onError: (err) => {
        setConfirm(null);
        // 409 optimistic-lock conflict: prompt refresh-and-retry; view unchanged.
        const is409 = err instanceof ConflictError;
        setToast({
          tone: 'error',
          text: is409
            ? '他人已改、请刷新重试（乐观锁冲突 409）'
            : `${editing ? '修订' : '接入'}失败：${err.message}`,
        });
      },
    });
  }

  function onFormValid(values: ResourceFormValues, editing: ResourceRow | null) {
    setDrawer({ kind: 'summary', editing, values });
  }

  function onSummaryConfirm(values: ResourceFormValues, editing: ResourceRow | null) {
    const highRisk = declaresHighRisk(values);
    if (highRisk.length > 0) {
      setConfirm({
        title: '声明高危动词面',
        body: (
          <span>
            资源 <span className="font-mono">{values.code}</span> 将声明高危动词：
            <span className="font-mono text-deny"> {highRisk.join(' · ')}</span>
            。确认接入？
          </span>
        ),
        onConfirm: () => submitPayload(values, editing),
      });
      return;
    }
    submitPayload(values, editing);
  }

  function onToggleEnable(row: ResourceRow) {
    const values: ResourceFormValues = {
      code: row.code,
      adapter: row.adapter,
      transport: (['ssh', 'ssm', 'direct'] as string[]).includes(row.transport)
        ? (row.transport as ResourceFormValues['transport'])
        : 'direct',
      engine_enforced: true,
      address: '',
      labels: row.labels.map((l) => ({ key: l.key, value: l.value })),
      tiers: row.tiers.map((t) => ({ tier: t.tier, capabilities: t.capabilities })),
    };
    if (row.enable_flag) {
      // Disabling is a danger action: explicit confirm (设计 §4.6).
      setConfirm({
        title: '停用资源',
        body: (
          <span>
            停用 <span className="font-mono">{row.code}</span> 将使其 Scope 内授权
            不可达，确认？
          </span>
        ),
        confirmWord: row.code,
        onConfirm: () => submitPayload(values, row, false),
      });
    } else {
      // Enabling is symmetric but non-destructive: write directly.
      submitPayload(values, row, true);
    }
  }

  function onDiscover(row: ResourceRow) {
    setDrawer({ kind: 'discover', resource: row });
    discover.reset();
    discover.mutate(row.code);
  }

  const columns: Column<ResourceRow>[] = [
    {
      key: 'code',
      header: 'code',
      sortValue: (r) => r.code,
      render: (r) => (
        <button
          type="button"
          onClick={() => setDrawer({ kind: 'form', editing: r })}
          className="hover:underline"
        >
          <ResourceCodeBadge code={r.code} adapter={r.adapter} transport={r.transport} />
        </button>
      ),
    },
    {
      key: 'adapter',
      header: 'adapter',
      sortValue: (r) => r.adapter,
      render: (r) => <Badge className="border-border text-text-muted">{r.adapter}</Badge>,
    },
    {
      key: 'transport',
      header: 'transport',
      render: (r) => <Badge className="border-border text-text-muted">{r.transport}</Badge>,
    },
    {
      key: 'tiers',
      header: 'tiers',
      render: (r) => (
        <span className="flex flex-wrap gap-1" title={r.tiers.map((t) => t.tier).join(' · ')}>
          {r.tiers.map((t) => (
            <Badge key={t.tier} className="border-border text-text-muted" title={t.capabilities.join(', ')}>
              {t.tier}
            </Badge>
          ))}
        </span>
      ),
    },
    {
      key: 'caps',
      header: 'capabilities',
      render: (r) => {
        const caps = uniqueCaps(r);
        return (
          <span className="flex flex-wrap gap-1">
            {caps.map((c) => (
              <CapabilityBadge key={c} capability={c} />
            ))}
          </span>
        );
      },
    },
    {
      key: 'labels',
      header: 'labels',
      render: (r) =>
        r.labels.length === 0 ? (
          <span className="text-text-muted">—</span>
        ) : (
          <span className="flex flex-wrap gap-1">
            {r.labels.map((l) => (
              <Badge key={`${l.key}=${l.value}`} className="border-border text-text-muted font-mono">
                {l.key}={l.value}
              </Badge>
            ))}
          </span>
        ),
    },
    {
      key: 'status',
      header: '状态',
      sortValue: (r) => (r.enable_flag ? 0 : 1),
      render: (r) =>
        r.enable_flag ? (
          <Badge className="border-allow/50 text-allow">启用</Badge>
        ) : (
          <Badge className="border-warn/50 text-warn">停用</Badge>
        ),
    },
  ];

  return (
    <section className="flex flex-col gap-4">
      {/* 标题 + 主操作 */}
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-medium">资源 Resources</h1>
        </div>
        <button
          type="button"
          onClick={() => setDrawer({ kind: 'form', editing: null })}
          className="inline-flex items-center gap-1 rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
        >
          <Plus size={16} /> 接入资源
        </button>
      </header>

      {toast && (
        <div
          role={toast.tone === 'error' ? 'alert' : 'status'}
          className={
            toast.tone === 'error'
              ? 'rounded-card border border-deny/40 bg-deny/5 px-3 py-2 text-sm text-deny'
              : 'rounded-card border border-allow/40 bg-allow/5 px-3 py-2 text-sm text-allow'
          }
        >
          {toast.text}
        </div>
      )}

      {/* 筛选条 */}
      <div className="flex flex-wrap items-center gap-2 rounded-card border border-border bg-surface-2 px-3 py-2">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="代号/标签 ⌕"
          aria-label="筛选 代号或标签"
          className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
        />
        <select
          value={adapterFilter}
          onChange={(e) => setAdapterFilter(e.target.value)}
          aria-label="按 adapter 筛选"
          className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
        >
          <option value={ALL}>adapter（全部）</option>
          {(['postgres', 'http', 'docker'] as Adapter[]).map((a) => (
            <option key={a} value={a}>
              {a}
            </option>
          ))}
        </select>
        <select
          value={statusFilter}
          onChange={(e) => setStatusFilter(e.target.value)}
          aria-label="按状态筛选"
          className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
        >
          <option value={ALL}>状态（全部）</option>
          <option value="enabled">启用</option>
          <option value="disabled">停用</option>
        </select>
      </div>

      {/* DataTable（强制分页 + 三态 + 行操作） */}
      <DataTable
        columns={columns}
        rows={rows}
        total={data?.total ?? 0}
        page={page}
        onPageChange={setPage}
        rowKey={(r) => r.id}
        loading={isLoading}
        error={isError ? { message: error instanceof Error ? error.message : '加载失败' } : null}
        onRetry={() => void refetch()}
        emptyTitle="尚无资源"
        emptyAction={
          <button
            type="button"
            onClick={() => setDrawer({ kind: 'form', editing: null })}
            className="inline-flex items-center gap-1 rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
          >
            <Plus size={16} /> 接入资源
          </button>
        }
        rowActions={(r) => (
          <RowActions
            row={r}
            onDiscover={onDiscover}
            onEdit={(row) => setDrawer({ kind: 'form', editing: row })}
            onToggleEnable={onToggleEnable}
          />
        )}
      />

      {/* 抽屉：表单 / 摘要 / 探测 */}
      {drawer.kind === 'form' && (
        <FormDrawer
          open
          title={drawer.editing ? `编辑 ${drawer.editing.code}` : '接入资源'}
          onClose={closeDrawer}
          footer={
            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={closeDrawer}
                className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
              >
                取消
              </button>
              <button
                type="submit"
                form="resource-form"
                className="rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
              >
                预览摘要 →
              </button>
            </div>
          }
        >
          <ResourceForm
            formId="resource-form"
            editing={drawer.editing}
            onValid={(values) => onFormValid(values, drawer.editing)}
            onCancel={closeDrawer}
          />
        </FormDrawer>
      )}

      {drawer.kind === 'summary' && (
        <FormDrawer
          open
          title="摘要预览"
          onClose={() => setDrawer({ kind: 'form', editing: drawer.editing })}
          footer={
            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setDrawer({ kind: 'form', editing: drawer.editing })}
                className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
              >
                返回修改
              </button>
              <button
                type="button"
                disabled={postResource.isPending}
                onClick={() => onSummaryConfirm(drawer.values, drawer.editing)}
                className="rounded-card bg-info px-3 py-1.5 text-sm text-white hover:enabled:brightness-110 disabled:opacity-40"
              >
                确认提交
              </button>
            </div>
          }
        >
          <SummaryPreview values={drawer.values} editing={Boolean(drawer.editing)} />
        </FormDrawer>
      )}

      {drawer.kind === 'discover' && (
        <FormDrawer
          open
          title={`Discover: ${drawer.resource.code}`}
          onClose={closeDrawer}
        >
          <DiscoverView
            resource={drawer.resource}
            surface={discover.data}
            loading={discover.isPending}
            error={discover.error}
            onRetry={() => discover.mutate(drawer.resource.code)}
            onConfigure={() => {
              closeDrawer();
              setToast({
                tone: 'success',
                text: '探测完成，可前往细则页配置',
              });
            }}
          />
        </FormDrawer>
      )}

      {/* 危险确认 */}
      {confirm && (
        <ConfirmDialog
          open
          title={confirm.title}
          body={confirm.body}
          confirmWord={confirm.confirmWord}
          onConfirm={confirm.onConfirm}
          onCancel={() => setConfirm(null)}
        />
      )}
    </section>
  );
}

/** Folded, de-duplicated capability set across a resource's tiers. */
function uniqueCaps(r: ResourceRow): Capability[] {
  const seen = new Set<Capability>();
  for (const t of r.tiers) for (const c of t.capabilities) seen.add(c);
  return [...seen];
}
