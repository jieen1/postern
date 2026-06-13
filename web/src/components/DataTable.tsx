import { useMemo, useState, type ReactNode } from 'react';
import { ArrowDown, ArrowUp, ChevronsUpDown } from 'lucide-react';
import { EmptyState, ErrorState, LoadingSkeleton } from './States';
import {
  PAGE_DEFAULT_SIZE,
  PAGE_MAX_SIZE,
  PAGE_MIN_SIZE,
  type PageQuery,
} from '../api/types';
import { cn } from '../lib/cn';

/**
 * Dense data table (设计系统 §4 / §5 / §8):
 *  - column sorting + per-column accessor rendering,
 *  - FORCED pagination: page_size defaults to 20 and is clamped to [1,200]
 *    (DB_PAGINATION_MANDATORY); the selector only offers legal sizes,
 *  - three states (loading skeleton / fail-closed error / empty),
 *  - server-driven paging via `page` + `onPageChange` (total from the envelope).
 */

export interface Column<T> {
  /** Stable key. */
  key: string;
  header: ReactNode;
  /** Cell renderer. */
  render: (row: T) => ReactNode;
  /** Optional sort accessor; column is sortable iff provided. */
  sortValue?: (row: T) => string | number;
  className?: string;
}

export const PAGE_SIZE_OPTIONS = [20, 50, 100, 200] as const;

/** Clamp a requested page size into [1,200] (mirrors backend PageQuery::clamp). */
export function clampPageSize(size: number): number {
  if (!Number.isFinite(size)) return PAGE_DEFAULT_SIZE;
  return Math.min(PAGE_MAX_SIZE, Math.max(PAGE_MIN_SIZE, Math.trunc(size)));
}

type SortDir = 'asc' | 'desc';

export function DataTable<T>({
  columns,
  rows,
  total,
  page,
  onPageChange,
  rowKey,
  loading,
  error,
  onRetry,
  emptyTitle,
  emptyAction,
  rowActions,
}: {
  columns: Column<T>[];
  rows: T[];
  /** Total across all pages (from the paged envelope). */
  total: number;
  page: PageQuery;
  onPageChange: (next: PageQuery) => void;
  rowKey: (row: T) => string;
  loading?: boolean;
  error?: { message?: string } | null;
  onRetry?: () => void;
  emptyTitle?: string;
  emptyAction?: ReactNode;
  rowActions?: (row: T) => ReactNode;
}) {
  const [sort, setSort] = useState<{ key: string; dir: SortDir } | null>(null);

  // Client-side sort within the current page only (server owns paging).
  const sortedRows = useMemo(() => {
    if (!sort) return rows;
    const col = columns.find((c) => c.key === sort.key);
    if (!col?.sortValue) return rows;
    const acc = col.sortValue;
    const factor = sort.dir === 'asc' ? 1 : -1;
    return [...rows].sort((a, b) => {
      const av = acc(a);
      const bv = acc(b);
      if (av < bv) return -1 * factor;
      if (av > bv) return 1 * factor;
      return 0;
    });
  }, [rows, sort, columns]);

  function toggleSort(key: string) {
    setSort((prev) => {
      if (prev?.key !== key) return { key, dir: 'asc' };
      if (prev.dir === 'asc') return { key, dir: 'desc' };
      return null;
    });
  }

  const pageSize = clampPageSize(page.page_size);
  const pageCount = Math.max(1, Math.ceil(total / pageSize));
  const pageNo = Math.min(Math.max(1, page.page_no), pageCount);

  if (error) {
    return <ErrorState message={error.message} onRetry={onRetry} />;
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="overflow-x-auto rounded-card border border-border">
        <table className="w-full border-collapse text-sm">
          <thead>
            <tr className="border-b border-border bg-surface-2 text-left text-text-muted">
              {columns.map((col) => {
                const sortable = Boolean(col.sortValue);
                const active = sort?.key === col.key;
                return (
                  <th
                    key={col.key}
                    className={cn('px-3 py-2 font-medium', col.className)}
                  >
                    {sortable ? (
                      <button
                        type="button"
                        onClick={() => toggleSort(col.key)}
                        className="inline-flex items-center gap-1 hover:text-text"
                        aria-sort={active ? (sort?.dir === 'asc' ? 'ascending' : 'descending') : 'none'}
                      >
                        {col.header}
                        {active ? (
                          sort?.dir === 'asc' ? (
                            <ArrowUp size={12} />
                          ) : (
                            <ArrowDown size={12} />
                          )
                        ) : (
                          <ChevronsUpDown size={12} className="opacity-50" />
                        )}
                      </button>
                    ) : (
                      col.header
                    )}
                  </th>
                );
              })}
              {rowActions && <th className="px-3 py-2" />}
            </tr>
          </thead>
          <tbody>
            {loading ? (
              <tr>
                <td colSpan={columns.length + (rowActions ? 1 : 0)} className="p-3">
                  <LoadingSkeleton />
                </td>
              </tr>
            ) : sortedRows.length === 0 ? (
              <tr>
                <td colSpan={columns.length + (rowActions ? 1 : 0)} className="p-0">
                  <EmptyState title={emptyTitle} action={emptyAction} />
                </td>
              </tr>
            ) : (
              sortedRows.map((row) => (
                <tr
                  key={rowKey(row)}
                  className="border-b border-border last:border-0 hover:bg-surface-2"
                >
                  {columns.map((col) => (
                    <td key={col.key} className={cn('px-3 py-2', col.className)}>
                      {col.render(row)}
                    </td>
                  ))}
                  {rowActions && (
                    <td className="px-3 py-2 text-right">{rowActions(row)}</td>
                  )}
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>

      <Pagination
        pageNo={pageNo}
        pageSize={pageSize}
        pageCount={pageCount}
        total={total}
        onPageChange={onPageChange}
      />
    </div>
  );
}

function Pagination({
  pageNo,
  pageSize,
  pageCount,
  total,
  onPageChange,
}: {
  pageNo: number;
  pageSize: number;
  pageCount: number;
  total: number;
  onPageChange: (next: PageQuery) => void;
}) {
  return (
    <div className="flex items-center justify-between text-sm text-text-muted">
      <div>
        共 {total} 条 · 第 {pageNo}/{pageCount} 页
      </div>
      <div className="flex items-center gap-2">
        <label className="flex items-center gap-1">
          每页
          <select
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
  );
}
