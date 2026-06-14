import { useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { RefreshCw } from 'lucide-react';
import {
  ConfirmDialog,
  DataTable,
  EmptyState,
  ErrorState,
  LoadingSkeleton,
  ResourceCodeBadge,
  CapabilityBadge,
  SnowflakeId,
  TtlBadge,
  type Column,
} from '../../components';
import {
  ConflictError,
  useGrants,
  useHealth,
  usePrincipals,
  useElevateGrant,
  useRevokeGrant,
} from '../../api';
import type { PageQuery, TempGrantRow } from '../../api/types';
import { formatTime } from '../../lib/format';
import { GrantMatrix } from './GrantMatrix';
import { GrantCellDrawer } from './GrantCellDrawer';
import { ElevateDrawer } from './ElevateDrawer';
import { buildMatrix, liveTempGrants, type MatrixCell } from './matrix';
import { ttlToMs, type ElevateForm } from './elevateSchema';

/**
 * 授权矩阵 Grants — single-principal detail view (web/docs/05-grants.md).
 * Reads the daemon's `your_grants` + live `temp_grants` and renders them as a
 * Resource × Capability matrix; the two writes (临时提权 elevate / 吊销 revoke)
 * follow the unified flow: form → summary preview → danger confirm → invalidate
 * → success / failure / 409. Zero security logic — daemon facts are rendered
 * verbatim; absence == default deny; out-of-scope is indistinguishable.
 */
export function GrantsPage() {
  // Cross-page deep link (web/docs/05-grants.md): Denials jumps here with
  // /grants?principal=<p>&resource=<r> to echo that exact cell. Read the params
  // once as initial state; the user can freely change both afterwards.
  const [searchParams] = useSearchParams();
  const linkedPrincipal = searchParams.get('principal') ?? undefined;
  const linkedResource = searchParams.get('resource') ?? '';

  const [principal, setPrincipal] = useState<string | undefined>(linkedPrincipal);
  const [resourceFilter, setResourceFilter] = useState(linkedResource);
  const [onlyGranted, setOnlyGranted] = useState(false);
  const [tempPage, setTempPage] = useState<PageQuery>({ page_no: 1, page_size: 20 });

  // Snapshot "now" once per render so matrix + countdown agree within a frame.
  const now = Date.now();

  const principalsQuery = usePrincipals({ page_no: 1, page_size: 200 });
  const healthQuery = useHealth();

  // Default the principal selector to the first principal once loaded. A
  // deep-linked principal is only honored once it's confirmed to exist in the
  // loaded list; an unknown one falls back to the default (no phantom value).
  const principalOptions = principalsQuery.data?.items ?? [];
  const knownPrincipal =
    principal !== undefined && principalOptions.some((p) => p.name === principal)
      ? principal
      : undefined;

  // Query grants for the validated principal (undefined ⇒ daemon's default),
  // so the matrix always matches the principal the selector shows.
  const grantsQuery = useGrants(knownPrincipal, tempPage);

  const elevate = useElevateGrant();
  const revoke = useRevokeGrant();

  const selectedPrincipal = knownPrincipal ?? principalOptions[0]?.name;

  // Drawers / dialogs.
  const [cellDrawer, setCellDrawer] = useState<MatrixCell | null>(null);
  const [elevateOpen, setElevateOpen] = useState(false);
  const [pendingElevate, setPendingElevate] = useState<ElevateForm | null>(null);
  const [pendingRevoke, setPendingRevoke] = useState<TempGrantRow | null>(null);

  const matrixRows = useMemo(() => {
    if (!grantsQuery.data) return [];
    return buildMatrix(grantsQuery.data, now);
  }, [grantsQuery.data, now]);

  const liveTemp = useMemo(() => {
    if (!grantsQuery.data) return [];
    return liveTempGrants(grantsQuery.data, now);
  }, [grantsQuery.data, now]);

  // Page the temp_grants list client-side (the request already carried
  // page_no/page_size; the list view paginates the live rows).
  const pagedTemp = useMemo(() => {
    const start = (tempPage.page_no - 1) * tempPage.page_size;
    return liveTemp.slice(start, start + tempPage.page_size);
  }, [liveTemp, tempPage]);

  const filteredRows = useMemo(() => {
    return matrixRows.filter((row) => {
      if (resourceFilter && !row.resource.includes(resourceFilter)) return false;
      if (onlyGranted && row.allDeny) return false;
      return true;
    });
  }, [matrixRows, resourceFilter, onlyGranted]);

  const resourceCodes = matrixRows.map((r) => r.resource);
  const busy = elevate.isPending || revoke.isPending;

  function openElevate() {
    elevate.reset();
    setElevateOpen(true);
  }

  function confirmElevate() {
    if (!pendingElevate || !selectedPrincipal) return;
    const form = pendingElevate;
    elevate.mutate(
      {
        principal: selectedPrincipal,
        resource: form.resource,
        capability: form.capability,
        ttl_ms: ttlToMs(form.ttlValue, form.ttlUnit),
      },
      {
        onSuccess: () => {
          setPendingElevate(null);
          setElevateOpen(false);
        },
        // On failure, dismiss the confirm but KEEP the drawer open: the error
        // (incl. 409) surfaces in the drawer; the local matrix is unchanged.
        onError: () => setPendingElevate(null),
      },
    );
  }

  function confirmRevoke() {
    if (!pendingRevoke) return;
    const row = pendingRevoke;
    revoke.mutate(
      { id: row.id, version: row.version },
      {
        onSuccess: () => {
          setPendingRevoke(null);
          setCellDrawer(null);
        },
      },
    );
  }

  const tempColumns: Column<TempGrantRow>[] = [
    { key: 'id', header: 'id', render: (r) => <SnowflakeId id={r.id} /> },
    {
      key: 'resource',
      header: 'resource',
      render: (r) => <ResourceCodeBadge code={r.resource} />,
    },
    {
      key: 'capability',
      header: 'cap',
      render: (r) => <CapabilityBadge capability={r.capability} />,
    },
    {
      key: 'granted_at',
      header: 'granted_at',
      render: (r) => <span className="font-mono text-xs">{formatTime(r.granted_at)}</span>,
    },
    {
      key: 'ttl',
      header: 'TTL 剩余',
      render: (r) => <TtlBadge expiresAt={r.expires_at} now={now} />,
    },
  ];

  const matrixError = grantsQuery.isError;

  return (
    <section className="flex flex-col gap-6">
      <header className="flex items-center justify-between">
        <h1 className="text-2xl font-medium">授权矩阵 Grants</h1>
        <button
          type="button"
          onClick={openElevate}
          disabled={matrixError || !selectedPrincipal}
          className="rounded-card bg-deny px-3 py-1.5 text-sm text-white disabled:opacity-40 hover:enabled:brightness-110"
        >
          + Elevate 提权
        </button>
      </header>

      {/* Principal 选择 + policy_rev 对账锚点 */}
      <div className="flex flex-wrap items-center gap-4 rounded-card border border-border bg-surface px-4 py-3 text-sm">
        <label className="flex items-center gap-2">
          <span className="text-text-muted">Principal</span>
          <select
            value={selectedPrincipal ?? ''}
            onChange={(e) => {
              setPrincipal(e.target.value || undefined);
              setTempPage({ page_no: 1, page_size: tempPage.page_size });
            }}
            disabled={principalsQuery.isLoading}
            aria-label="选择 Principal"
            className="rounded-card border border-border bg-surface px-2 py-1 font-mono"
          >
            {principalOptions.map((p) => (
              <option key={p.id} value={p.name}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <span className="text-text-muted">
          policy_rev:{' '}
          <span className="font-mono text-text">
            {healthQuery.data?.policy_rev ?? '—'}
          </span>
        </span>
        <button
          type="button"
          onClick={() => {
            void grantsQuery.refetch();
            void healthQuery.refetch();
          }}
          className="inline-flex items-center gap-1 rounded-card border border-border px-2 py-1 hover:bg-surface-2"
        >
          <RefreshCw size={14} /> 刷新
        </button>
      </div>

      {/* 生效授权矩阵 */}
      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-4 text-sm">
          <h2 className="font-medium">生效授权矩阵 (Resource × Capability)</h2>
          <input
            type="search"
            value={resourceFilter}
            onChange={(e) => setResourceFilter(e.target.value)}
            placeholder="资源代号筛选…"
            aria-label="资源代号筛选"
            className="rounded-card border border-border bg-surface px-2 py-1"
          />
          <label className="flex items-center gap-1">
            <input
              type="checkbox"
              checked={onlyGranted}
              onChange={(e) => setOnlyGranted(e.target.checked)}
            />
            只看已授格
          </label>
          <span className="flex items-center gap-3 text-xs text-text-muted">
            <span className="text-allow">✅ 持久</span>
            <span className="text-warn">⏱ 临时</span>
            <span>❌ 默认拒绝</span>
          </span>
        </div>

        {grantsQuery.isLoading ? (
          <LoadingSkeleton rows={6} />
        ) : matrixError ? (
          <ErrorState
            title="无法读取授权矩阵"
            message={
              grantsQuery.error instanceof Error
                ? grantsQuery.error.message
                : '未知错误'
            }
            onRetry={() => void grantsQuery.refetch()}
          />
        ) : filteredRows.length === 0 ? (
          <EmptyState
            title={`${selectedPrincipal ?? '该主体'} 当前无任何生效授权（默认拒绝世界）`}
          />
        ) : (
          <GrantMatrix rows={filteredRows} now={now} onSelectCell={setCellDrawer} />
        )}
      </div>

      {/* 当前生效临时授权 */}
      <div className="flex flex-col gap-3">
        <h2 className="text-sm font-medium">当前生效临时授权 (temp_grants)</h2>
        <DataTable
          columns={tempColumns}
          rows={pagedTemp}
          total={liveTemp.length}
          page={tempPage}
          onPageChange={setTempPage}
          rowKey={(r) => r.id}
          loading={grantsQuery.isLoading}
          error={matrixError ? { message: '无法读取临时授权' } : null}
          onRetry={() => void grantsQuery.refetch()}
          emptyTitle="当前无生效临时授权"
          rowActions={(row) => (
            <button
              type="button"
              onClick={() => setPendingRevoke(row)}
              className="rounded-card border border-deny/50 px-2 py-1 text-xs text-deny hover:bg-deny/10"
            >
              吊销
            </button>
          )}
        />
      </div>

      {/* 格 provenance 抽屉 */}
      {cellDrawer && (
        <GrantCellDrawer
          cell={cellDrawer}
          now={now}
          onClose={() => setCellDrawer(null)}
          onRevoke={(c) => c.temp && setPendingRevoke(c.temp)}
        />
      )}

      {/* Elevate 表单抽屉 */}
      {selectedPrincipal && (
        <ElevateDrawer
          open={elevateOpen}
          principal={selectedPrincipal}
          resources={resourceCodes}
          now={now}
          submitting={busy}
          conflict={elevate.error instanceof ConflictError}
          errorMessage={
            elevate.error instanceof Error && !(elevate.error instanceof ConflictError)
              ? elevate.error.message
              : null
          }
          onClose={() => {
            setElevateOpen(false);
            elevate.reset();
          }}
          onSubmit={(form) => setPendingElevate(form)}
        />
      )}

      {/* Elevate 危险确认（扩权） */}
      <ConfirmDialog
        open={pendingElevate !== null}
        title="确认临时提权（扩权）"
        confirmWord={pendingElevate?.resource}
        confirmLabel="确认提权"
        body={
          pendingElevate && selectedPrincipal ? (
            <span>
              将给 <span className="font-mono">{selectedPrincipal}</span> 在{' '}
              <span className="font-mono">{pendingElevate.resource}</span> 上临时授予{' '}
              <span className="font-mono">{pendingElevate.capability}</span>，
              {pendingElevate.ttlValue}
              {pendingElevate.ttlUnit === 'minute'
                ? ' 分钟'
                : pendingElevate.ttlUnit === 'hour'
                  ? ' 小时'
                  : ' 天'}
              后自动回收。这会扩大该主体的授权面，输入资源代号确认。
            </span>
          ) : null
        }
        onConfirm={confirmElevate}
        onCancel={() => setPendingElevate(null)}
      />

      {/* Revoke 危险确认（收权） */}
      <ConfirmDialog
        open={pendingRevoke !== null}
        title="确认吊销临时授权（收权）"
        confirmLabel="确认吊销"
        body={
          pendingRevoke ? (
            <span>
              将立即吊销{' '}
              <span className="font-mono">{selectedPrincipal}</span> 在{' '}
              <span className="font-mono">{pendingRevoke.resource}</span> 上的临时{' '}
              <span className="font-mono">{pendingRevoke.capability}</span>
              ，立即关闭。
              {revoke.error instanceof ConflictError && (
                <span className="mt-2 block text-warn" role="alert">
                  他人已改该临时授权，请刷新重读后重试（409）。
                </span>
              )}
            </span>
          ) : null
        }
        onConfirm={confirmRevoke}
        onCancel={() => {
          setPendingRevoke(null);
          revoke.reset();
        }}
      />
    </section>
  );
}

export default GrantsPage;
