import { useMemo, useRef, useState } from 'react';
import { useAudit } from '../../api/hooks';
import { getAudit } from '../../api/endpoints';
import { PAGE_DEFAULT_SIZE, type AuditQuery, type PageQuery } from '../../api/types';
import { AuditFilterBar, EMPTY_FILTERS, type AuditFilters } from './AuditFilterBar';
import { AuditList } from './AuditList';
import { ExportMenu } from './ExportMenu';

/**
 * 审计 Audit (02-audit) — the read-only audit event-stream view.
 *
 * Query the control plane (`GET /v1/audit`) by since/principal/kind/decision,
 * reverse-chron + force-paginated (§七 DB_PAGINATION_MANDATORY). No write path:
 * audit is read + export only (§四) — no FormDrawer, no ConfirmDialog, no
 * optimistic-lock 409. Snowflake ids stay strings end-to-end (§3.4).
 *
 * APPLIED filters (what drives the request) are kept separate from the DRAFT in
 * the filter bar, so typing does not refetch until "应用" — and applying always
 * resets to page 1 (§4.1). Sorting/paging are server-driven; the page never
 * fetches the full set to slice/sort client-side.
 */
export function AuditPage() {
  const [applied, setApplied] = useState<AuditFilters>(EMPTY_FILTERS);
  const [draft, setDraft] = useState<AuditFilters>(EMPTY_FILTERS);
  const [page, setPage] = useState<PageQuery>({
    page_no: 1,
    page_size: PAGE_DEFAULT_SIZE,
  });
  const filterRef = useRef<HTMLInputElement>(null);

  const query: AuditQuery = useMemo(
    () => buildQuery(applied, page),
    [applied, page],
  );

  const { data, isLoading, isError, error, refetch, isFetching } = useAudit(query);

  function applyFilters() {
    setApplied(draft);
    // Applying a new filter always returns to page 1 (§4.1).
    setPage((p) => ({ page_no: 1, page_size: p.page_size }));
  }

  function clearFilters() {
    setDraft(EMPTY_FILTERS);
    setApplied(EMPTY_FILTERS);
    setPage((p) => ({ page_no: 1, page_size: p.page_size }));
  }

  // Export reads the current filtered window (same source as the human render).
  async function exportRows(): Promise<unknown[]> {
    const res = await getAudit(query);
    return res.items;
  }

  return (
    <section className="flex flex-col gap-4">
      <header className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-medium">审计 Audit</h1>
          <p className="mt-1 text-sm text-text-muted">
            事件流 · 倒序（ts 新→旧）· 强制分页（后端）
          </p>
        </div>
        <ExportMenu query={query} onExport={exportRows} disabled={isError} />
      </header>

      <AuditFilterBar
        draft={draft}
        onDraftChange={setDraft}
        onApply={applyFilters}
        onClear={clearFilters}
        total={data?.total}
        busy={isFetching}
        inputRef={filterRef}
      />

      <AuditList
        events={data?.items}
        total={data?.total}
        page={page}
        onPageChange={setPage}
        loading={isLoading}
        error={isError ? { message: errorMessage(error) } : null}
        onRetry={() => void refetch()}
        onClearFilters={clearFilters}
      />
    </section>
  );
}

/** Map applied filters + page into the wire AuditQuery (omitting empty fields). */
function buildQuery(filters: AuditFilters, page: PageQuery): AuditQuery {
  const q: AuditQuery = { page_no: page.page_no, page_size: page.page_size };
  if (filters.since) q.since = new Date(filters.since).toISOString();
  if (filters.principal) q.principal = filters.principal;
  if (filters.kind) q.kind = filters.kind;
  if (filters.decision !== 'all') q.decision = filters.decision;
  return q;
}

function errorMessage(error: unknown): string | undefined {
  if (error instanceof Error) return error.message;
  return undefined;
}

export default AuditPage;
