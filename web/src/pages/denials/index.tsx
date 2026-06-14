import { useMemo, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { useDenials } from '../../api/hooks';
import { ErrorState } from '../../components';
import type { DenialWindow, PageQuery } from '../../api/types';
import { PAGE_DEFAULT_SIZE } from '../../api/types';
import { alertingRows, WINDOW_LABEL, WINDOW_OPTIONS } from './lib';
import { AlertBand } from './components/AlertBand';
import { WindowSummaryBar } from './components/WindowSummaryBar';
import { DenialRankTable } from './components/DenialRankTable';

/**
 * 拒绝分析 Denials — read-only aggregation view of recent deny events grouped
 * by (principal × resource × stage × capability), reverse-sorted by count, with
 * an alert band for over-threshold groups. The page only presents AGGREGATE
 * FACTS and routes the human to the right rule editor; it has NO write
 * operation and NO allow control (E7). Three states are fail-closed: loading →
 * skeleton, error → ErrorState (never fake/empty data), empty → neutral "no
 * denials" message clearly distinct from error.
 *
 * Source: GET /v1/denials/summary?window=…&page_no=…&page_size=… (control plane;
 * the Agent is physically unable to reach it).
 */
export function DenialsPage() {
  const [window, setWindow] = useState<DenialWindow>('7d');
  const [page, setPage] = useState<PageQuery>({
    page_no: 1,
    page_size: PAGE_DEFAULT_SIZE,
  });
  const [locateKey, setLocateKey] = useState<string | null>(null);

  const query = useDenials(window, page);
  const items = query.data?.items;
  const total = query.data?.total ?? 0;

  // Memoize over the stable query payload (not a freshly-spread array) so the
  // `rows`/`alerts` identity only changes when the data actually changes.
  const rows = useMemo(() => items ?? [], [items]);
  // Alerts are derived over the current page (no server alert-event feed yet —
  // see lib.ts). They are hidden entirely when none, never a "0 alerts" bar.
  const alerts = useMemo(() => alertingRows(rows), [rows]);

  function changeWindow(next: DenialWindow) {
    setWindow(next);
    setPage({ page_no: 1, page_size: page.page_size });
    setLocateKey(null);
  }

  function locate(key: string) {
    setLocateKey(key);
    document
      .querySelector(`[data-group-key="${cssEscape(key)}"]`)
      ?.scrollIntoView({ block: 'center' });
  }

  // A definitive load error replaces the whole content (fail-closed): no alert
  // band, no summary bar, no empty ranking that could read as "no denials".
  if (query.isError) {
    return (
      <section aria-labelledby="denials-title">
        <Header
          window={window}
          onWindow={changeWindow}
          onRefresh={() => query.refetch()}
          refreshing={query.isFetching}
        />
        <div className="mt-4">
          <ErrorState
            title="无法读取拒绝聚合"
            message={errMessage(query.error)}
            onRetry={() => query.refetch()}
          />
        </div>
      </section>
    );
  }

  return (
    <section aria-labelledby="denials-title" className="flex flex-col gap-4">
      <Header
        window={window}
        onWindow={changeWindow}
        onRefresh={() => query.refetch()}
        refreshing={query.isFetching}
      />

      <AlertBand rows={alerts} onLocate={locate} />

      <WindowSummaryBar window={window} groupCount={total} />

      <DenialRankTable
        rows={rows}
        total={total}
        page={page}
        onPageChange={setPage}
        loading={query.isLoading}
        error={null}
        onRetry={() => query.refetch()}
        highlightKey={locateKey}
      />
    </section>
  );
}

function Header({
  window,
  onWindow,
  onRefresh,
  refreshing,
}: {
  window: DenialWindow;
  onWindow: (w: DenialWindow) => void;
  onRefresh: () => void;
  refreshing: boolean;
}) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h1 id="denials-title" className="text-2xl font-medium">
          拒绝分析 Denials
        </h1>
        <p className="mt-1 text-sm text-text-muted">近期被拒请求聚合</p>
      </div>
      <div className="flex items-center gap-2">
        <label className="flex items-center gap-1 text-sm text-text-muted">
          窗口
          <select
            aria-label="窗口"
            value={window}
            onChange={(e) => onWindow(e.target.value as DenialWindow)}
            className="rounded-card border border-border bg-surface px-2 py-1 text-text"
          >
            {WINDOW_OPTIONS.map((w) => (
              <option key={w} value={w}>
                {WINDOW_LABEL[w]}（{w}）
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          onClick={onRefresh}
          disabled={refreshing}
          className="inline-flex items-center gap-1 rounded-card border border-border px-3 py-1 text-sm text-text-muted hover:bg-surface-2 hover:text-text disabled:opacity-50"
        >
          <RefreshCw
            size={14}
            className={refreshing ? 'animate-spin' : undefined}
            aria-hidden
          />
          刷新
        </button>
      </div>
    </div>
  );
}

function errMessage(err: unknown): string | undefined {
  if (err && typeof err === 'object' && 'message' in err) {
    const m = (err as { message?: unknown }).message;
    if (typeof m === 'string') return m;
  }
  return undefined;
}

/** Minimal CSS.escape fallback for the locate selector (jsdom-safe). */
function cssEscape(value: string): string {
  if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') {
    return CSS.escape(value);
  }
  return value.replace(/["\\]/g, '\\$&');
}
