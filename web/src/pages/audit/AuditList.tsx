import {
  AuditEventRow,
  EmptyState,
  ErrorState,
  LoadingSkeleton,
  clampPageSize,
  pairAuditEvents,
  PAGE_SIZE_OPTIONS,
} from '../../components';
import type { AuditEvent, PageQuery } from '../../api/types';

/**
 * Audit list body (02-audit §二/§三): renders the current page of audit events
 * as expandable AuditEventRow cards, grouping two-phase request events (intent
 * + outcome) by request_id. Forced pagination (§七 DB_PAGINATION_MANDATORY) is
 * server-driven — the footer is ALWAYS present (even empty: total=0), and page
 * size is clamped to [1,200].
 *
 * Three states are fail-closed (§3.2): error renders ErrorState (NO rows, no
 * fake data, never a deceptive empty list); empty renders EmptyState with a
 * "clear filters" guide; loading renders a skeleton that keeps the structure.
 */
export function AuditList({
  events,
  total,
  page,
  onPageChange,
  loading,
  error,
  onRetry,
  onClearFilters,
}: {
  events: AuditEvent[] | undefined;
  total: number | undefined;
  page: PageQuery;
  onPageChange: (next: PageQuery) => void;
  loading?: boolean;
  error?: { message?: string } | null;
  onRetry?: () => void;
  onClearFilters?: () => void;
}) {
  const pageSize = clampPageSize(page.page_size);
  const totalCount = total ?? 0;
  const pageCount = Math.max(1, Math.ceil(totalCount / pageSize));
  const pageNo = Math.min(Math.max(1, page.page_no), pageCount);

  // Two-phase pairing happens on the current window only (server owns paging).
  const pairs = events ? pairAuditEvents(events) : [];

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-2" aria-label="审计事件流" role="list">
        {error ? (
          // fail-closed: a query error is NOT an empty list.
          <ErrorState title="审计查询失败" message={error.message} onRetry={onRetry} />
        ) : loading ? (
          <LoadingSkeleton rows={6} />
        ) : pairs.length === 0 ? (
          <EmptyState
            title="当前筛选无匹配事件"
            hint="空 ≠ 错：此筛选下确实没有审计事件。"
            action={
              onClearFilters && (
                <button
                  type="button"
                  onClick={onClearFilters}
                  className="rounded-card border border-border px-3 py-1 text-sm hover:bg-surface-2"
                >
                  清空筛选
                </button>
              )
            }
          />
        ) : (
          pairs.map((pair) => (
            <div role="listitem" key={pair.intent.id ?? pair.intent.request_id ?? rowKey(pair.intent)}>
              <AuditEventRow pair={pair} />
            </div>
          ))
        )}
      </div>

      {/* Forced pagination footer — ALWAYS present, even on empty/error (§二). */}
      <div className="flex items-center justify-between text-sm text-text-muted">
        <div>
          共 {totalCount} 条 · 第 {pageNo}/{pageCount} 页
        </div>
        <div className="flex items-center gap-2">
          <label className="flex items-center gap-1">
            每页
            <select
              aria-label="每页条数"
              value={pageSize}
              onChange={(e) =>
                onPageChange({ page_no: 1, page_size: clampPageSize(Number(e.target.value)) })
              }
              className="rounded-card border border-border bg-surface px-2 py-1"
            >
              {PAGE_SIZE_OPTIONS.map((size) => (
                <option key={size} value={size}>
                  {size}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            disabled={pageNo <= 1}
            onClick={() => onPageChange({ page_no: pageNo - 1, page_size: pageSize })}
            className="rounded-card border border-border px-2 py-1 disabled:opacity-40 hover:enabled:bg-surface-2"
          >
            上一页
          </button>
          <button
            type="button"
            disabled={pageNo >= pageCount}
            onClick={() => onPageChange({ page_no: pageNo + 1, page_size: pageSize })}
            className="rounded-card border border-border px-2 py-1 disabled:opacity-40 hover:enabled:bg-surface-2"
          >
            下一页
          </button>
        </div>
      </div>
    </div>
  );
}

/** Stable fallback key for a standalone event with no id/request_id. */
function rowKey(ev: AuditEvent): string {
  return `${ev.kind}:${ev.ts ?? ''}:${ev.resource}:${ev.policy_rev}`;
}
