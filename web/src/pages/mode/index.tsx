import { useMemo, useState } from 'react';
import { useModeState, useSetMode } from '../../api/hooks';
import { ConflictError } from '../../api/client';
import { ModeBadge } from '../../components/ModeBadge';
import { TtlBadge } from '../../components/TtlBadge';
import { ResourceCodeBadge } from '../../components/ResourceCodeBadge';
import { SnowflakeId } from '../../components/SnowflakeId';
import {
  DataTable,
  type Column,
} from '../../components/DataTable';
import { ErrorState, LoadingSkeleton } from '../../components/States';
import { formatTime } from '../../lib/format';
import {
  MODES,
  PAGE_DEFAULT_SIZE,
  type Mode,
  type ModeStateRow,
  type ModeSetRequest,
  type PageQuery,
} from '../../api/types';
import { GlobalModeCard } from './GlobalModeCard';
import { NarrowingPreview } from './NarrowingPreview';
import {
  ModeSwitchDrawer,
  type ModeSwitchTarget,
  type SubmitState,
} from './ModeSwitchDrawer';
import { effectiveSource } from './mode-facts';

/**
 * 模式 Mode (11-mode.md): the runtime tightening console. A board (global card +
 * resource-override table) plus a switch drawer (the only write). Effective mode
 * (`global.meet(scoped)` = strictest) is computed in core and only displayed;
 * the frontend never recomputes it. All states are fail-closed: read failure →
 * ErrorState (no fake data, no assumed normal); writes carry the optimistic-lock
 * version and surface 409 verbatim without silent retry.
 */

function splitRows(rows: ModeStateRow[] | undefined) {
  const global = rows?.find((r) => r.scope === null) ?? null;
  const overrides = (rows ?? []).filter((r) => r.scope !== null);
  return { global, overrides };
}

export function ModePage() {
  const { data, isLoading, isError, error, refetch } = useModeState();
  const setMode = useSetMode();

  const [page, setPage] = useState<PageQuery>({
    page_no: 1,
    page_size: PAGE_DEFAULT_SIZE,
  });
  const [modeFilter, setModeFilter] = useState<Mode | ''>('');
  const [codeFilter, setCodeFilter] = useState('');
  const [target, setTarget] = useState<ModeSwitchTarget | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);

  const { global, overrides } = splitRows(data);

  // Client-side filter the override rows (resource code / mode), then page.
  const filtered = useMemo(() => {
    const code = codeFilter.trim().toLowerCase();
    return overrides.filter((r) => {
      if (modeFilter && r.mode !== modeFilter) return false;
      if (code && !(r.scope ?? '').toLowerCase().includes(code)) return false;
      return true;
    });
  }, [overrides, modeFilter, codeFilter]);

  const start = (page.page_no - 1) * page.page_size;
  const pageRows = filtered.slice(start, start + page.page_size);

  const globalMode: Mode = global?.effective_mode ?? 'normal';

  // ── Write-flow plumbing ─────────────────────────────────────────────────────
  const submitState: SubmitState = {
    pending: setMode.isPending,
    conflict: setMode.error instanceof ConflictError,
    error:
      setMode.error && !(setMode.error instanceof ConflictError)
        ? setMode.error instanceof Error
          ? setMode.error.message
          : '写入失败'
        : null,
  };

  function openSwitch(t: ModeSwitchTarget) {
    setMode.reset();
    setTarget(t);
    setDrawerOpen(true);
  }

  function openGlobalSwitch() {
    openSwitch({
      scope: null,
      currentMode: global?.mode ?? 'normal',
      version: global?.version ?? 0,
    });
  }

  function openGlobalFallback() {
    openSwitch({
      scope: null,
      currentMode: global?.mode ?? 'normal',
      version: global?.version ?? 0,
      initialMode: 'normal',
    });
  }

  function submit(req: ModeSetRequest) {
    setMode.mutate(req, {
      onSuccess: () => {
        setDrawerOpen(false);
        setTarget(null);
      },
      // On 409 / error we KEEP the drawer open and DO NOT touch local view; the
      // drawer footer renders the verbatim conflict/error from submitState.
    });
  }

  // ── Three-state fail-closed (read) ──────────────────────────────────────────
  if (isError) {
    return (
      <section className="flex flex-col gap-4">
        <Header onSwitch={openGlobalSwitch} disabled />
        <ErrorState
          title="无法读取当前模式"
          message={
            (error instanceof Error ? error.message : '请重试') +
            ' — 控制面不可达时本页无法确认或更改安全状态。'
          }
          onRetry={() => void refetch()}
        />
      </section>
    );
  }

  const columns: Column<ModeStateRow>[] = [
    {
      key: 'scope',
      header: '资源代号',
      render: (r) => <ResourceCodeBadge code={r.scope ?? ''} />,
      sortValue: (r) => r.scope ?? '',
    },
    {
      key: 'local',
      header: '本地模式',
      render: (r) => <ModeBadge mode={r.mode} />,
      sortValue: (r) => r.mode,
    },
    {
      key: 'effective',
      header: '有效模式(取严)',
      render: (r) => (
        <span className="inline-flex items-center gap-1">
          <ModeBadge mode={r.effective_mode} />
          <span className="text-[10px] text-text-muted">
            {effectiveSource(r.mode, r.effective_mode) === 'global' ? '←全局' : '←本地'}
          </span>
        </span>
      ),
      sortValue: (r) => r.effective_mode,
    },
    {
      key: 'ttl',
      header: 'TTL',
      render: (r) => <TtlBadge expiresAt={r.expires_at} />,
    },
    {
      key: 'meta',
      header: '生效自 / by',
      render: (r) => (
        <span className="text-xs text-text-muted">
          <span className="font-mono">{formatTime(r.updated_at)}</span>
          {' · '}
          {r.updated_by ?? '—'}
        </span>
      ),
    },
    {
      key: 'rev',
      header: 'policy_rev',
      render: (r) => <SnowflakeId id={r.policy_rev} />,
    },
  ];

  return (
    <section className="flex flex-col gap-4">
      <Header onSwitch={openGlobalSwitch} />

      <p className="text-sm text-text-muted">
        当前各辖区运行模式与覆盖关系；最严者生效（freeze &gt; observe &gt; maintain &gt; normal）。
      </p>

      {isLoading ? (
        <LoadingSkeleton rows={3} />
      ) : (
        <GlobalModeCard
          row={global}
          onSwitch={openGlobalSwitch}
          onFallback={openGlobalFallback}
        />
      )}

      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-medium">资源级覆盖 (Resource overrides)</h2>
        <div className="flex flex-wrap items-center gap-2">
          <input
            type="search"
            aria-label="筛选资源代号"
            placeholder="筛选: 资源代号"
            value={codeFilter}
            onChange={(e) => {
              setCodeFilter(e.target.value);
              setPage((p) => ({ ...p, page_no: 1 }));
            }}
            className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
          />
          <label className="flex items-center gap-1 text-sm text-text-muted">
            模式
            <select
              aria-label="筛选模式"
              value={modeFilter}
              onChange={(e) => {
                setModeFilter(e.target.value as Mode | '');
                setPage((p) => ({ ...p, page_no: 1 }));
              }}
              className="rounded-card border border-border bg-surface px-2 py-1"
            >
              <option value="">全部</option>
              {MODES.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </label>
        </div>
      </div>

      <DataTable
        columns={columns}
        rows={pageRows}
        total={filtered.length}
        page={page}
        onPageChange={setPage}
        rowKey={(r) => r.scope ?? 'global'}
        loading={isLoading}
        emptyTitle={`当前无资源级模式覆盖，全部辖区继承全局模式 ${globalMode}`}
        rowActions={(r) => (
          <div className="flex justify-end gap-2">
            <button
              type="button"
              onClick={() =>
                openSwitch({
                  scope: r.scope,
                  currentMode: r.mode,
                  version: r.version,
                })
              }
              className="rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
            >
              切换此资源
            </button>
            <button
              type="button"
              onClick={() =>
                openSwitch({
                  scope: r.scope,
                  currentMode: r.mode,
                  version: r.version,
                  initialMode: 'normal',
                })
              }
              className="rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
            >
              回落继承
            </button>
          </div>
        )}
      />

      <NarrowingPreview mode={globalMode} />

      <ModeSwitchDrawer
        open={drawerOpen}
        target={target}
        submitState={submitState}
        onSubmit={submit}
        onClose={() => {
          setDrawerOpen(false);
          setTarget(null);
          setMode.reset();
        }}
      />
    </section>
  );
}

function Header({
  onSwitch,
  disabled,
}: {
  onSwitch: () => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-center justify-between">
      <h1 className="text-2xl font-medium">模式 Mode</h1>
      <button
        type="button"
        onClick={onSwitch}
        disabled={disabled}
        className="rounded-card border border-border bg-surface px-3 py-1.5 text-sm hover:enabled:bg-surface-2 disabled:opacity-50"
      >
        切换模式
      </button>
    </div>
  );
}

export default ModePage;
