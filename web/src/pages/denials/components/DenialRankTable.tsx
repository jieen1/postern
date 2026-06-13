import { Fragment, useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import {
  CapabilityBadge,
  EmptyState,
  ErrorState,
  LoadingSkeleton,
  PAGE_SIZE_OPTIONS,
  ResourceCodeBadge,
  StageChip,
  clampPageSize,
} from '../../../components';
import type { DenialSummaryRow, PageQuery } from '../../../api/types';
import { cn } from '../../../lib/cn';
import { groupKey, isAlerting } from '../lib';
import { DenialDetailPanel } from './DenialDetailPanel';
import { DenialRowActions } from './DenialRowActions';

/**
 * DenialRankTable (聚合榜) — the page-local ranking table. It deliberately does
 * NOT use the shared <DataTable> because that component has no row-expansion;
 * the design requires an inline read-only detail panel per group. It DOES reuse
 * the shared pagination contract (clampPageSize / PAGE_SIZE_OPTIONS) and the
 * three shared states, so the DB_PAGINATION_MANDATORY discipline (default 20,
 * clamp [1,200], server-driven) is unchanged.
 *
 * Rows arrive already paged from the server; client-side sorting/expansion acts
 * only within the current page (never re-fetches the whole set).
 */

type SortDir = 'asc' | 'desc';
type SortKey = 'principal' | 'stage' | 'capability' | 'count';

export function DenialRankTable({
  rows,
  total,
  page,
  onPageChange,
  loading,
  error,
  onRetry,
  highlightKey,
}: {
  rows: DenialSummaryRow[];
  total: number;
  page: PageQuery;
  onPageChange: (next: PageQuery) => void;
  loading?: boolean;
  error?: { message?: string } | null;
  onRetry?: () => void;
  /** groupKey of a row to visually highlight (from an alert "locate"). */
  highlightKey?: string | null;
}) {
  const [expanded, setExpanded] = useState<string | null>(null);
  // Default sort: count descending (倒序 count) — the spec's primary order.
  const [sort, setSort] = useState<{ key: SortKey; dir: SortDir }>({
    key: 'count',
    dir: 'desc',
  });

  if (error) {
    // Fail-closed: error replaces the whole table; no fake/empty ranking.
    return (
      <ErrorState
        title="无法读取拒绝聚合"
        message={error.message}
        onRetry={onRetry}
      />
    );
  }

  const pageSize = clampPageSize(page.page_size);
  const pageCount = Math.max(1, Math.ceil(total / pageSize));
  const pageNo = Math.min(Math.max(1, page.page_no), pageCount);

  const sortedRows = sortRows(rows, sort);

  return (
    <div className="flex flex-col gap-3">
      <div className="overflow-x-auto rounded-card border border-border">
        <table className="w-full border-collapse text-sm">
          <thead>
            <tr className="border-b border-border bg-surface-2 text-left text-text-muted">
              <th className="w-8 px-2 py-2" />
              <SortHeader
                label="主体"
                col="principal"
                sort={sort}
                onSort={setSort}
              />
              <th className="px-3 py-2 font-medium">资源</th>
              <SortHeader label="阶段" col="stage" sort={sort} onSort={setSort} />
              <SortHeader
                label="动词"
                col="capability"
                sort={sort}
                onSort={setSort}
              />
              <SortHeader
                label="次数"
                col="count"
                sort={sort}
                onSort={setSort}
                className="text-right"
              />
              <th className="px-3 py-2 font-medium">样本 digest</th>
              <th className="px-3 py-2" />
            </tr>
          </thead>
          <tbody>
            {loading ? (
              <tr>
                <td colSpan={8} className="p-3">
                  <LoadingSkeleton />
                </td>
              </tr>
            ) : sortedRows.length === 0 ? (
              <tr>
                <td colSpan={8} className="p-0">
                  <EmptyState
                    title="该窗口内无被拒事件"
                    hint="这是真实的好消息（非错误）。可去 Verify 页主动自检，或放宽窗口范围。"
                  />
                </td>
              </tr>
            ) : (
              sortedRows.map((row) => {
                const key = groupKey(row);
                const open = expanded === key;
                const alerting = isAlerting(row);
                const highlighted = highlightKey === key;
                return (
                  <Fragment key={key}>
                    <tr
                      data-group-key={key}
                      className={cn(
                        'border-b border-border last:border-0',
                        // Left amber edge for over-threshold groups.
                        alerting && 'border-l-2 border-l-warn',
                        highlighted ? 'bg-warn/10' : 'hover:bg-surface-2',
                      )}
                    >
                      <td className="px-2 py-2 align-top">
                        <button
                          type="button"
                          onClick={() => setExpanded(open ? null : key)}
                          aria-expanded={open}
                          aria-label={open ? '收起细节' : '展开细节'}
                          className="text-text-muted hover:text-text"
                        >
                          {open ? (
                            <ChevronDown size={14} aria-hidden />
                          ) : (
                            <ChevronRight size={14} aria-hidden />
                          )}
                        </button>
                      </td>
                      <td className="px-3 py-2 align-top">
                        <span className="inline-flex items-center gap-1">
                          {alerting && (
                            <span
                              className="text-warn"
                              title={`超阈值告警组`}
                              aria-label="超阈值告警组"
                            >
                              ⚠
                            </span>
                          )}
                          <span className="text-text">{row.principal ?? '—'}</span>
                        </span>
                      </td>
                      <td className="px-3 py-2 align-top">
                        <ResourceCodeBadge code={row.resource} />
                      </td>
                      <td className="px-3 py-2 align-top">
                        <StageChip stage={row.stage} />
                      </td>
                      <td className="px-3 py-2 align-top">
                        <CapabilityBadge capability={row.capability} />
                      </td>
                      <td className="px-3 py-2 text-right align-top font-mono font-medium">
                        {row.count}
                      </td>
                      <td className="px-3 py-2 align-top font-mono text-xs text-text-muted">
                        {row.intent_digest || '—'}
                      </td>
                      <td className="px-3 py-2 text-right align-top">
                        <DenialRowActions row={row} />
                      </td>
                    </tr>
                    {open && (
                      <tr className="border-b border-border last:border-0">
                        <td colSpan={8} className="p-0">
                          <DenialDetailPanel row={row} />
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })
            )}
          </tbody>
        </table>
      </div>

      <div className="flex items-center justify-between text-sm text-text-muted">
        <div>
          共 {total} 组 · 第 {pageNo}/{pageCount} 页
        </div>
        <div className="flex items-center gap-2">
          <label className="flex items-center gap-1">
            每页
            <select
              aria-label="每页组数"
              value={pageSize}
              onChange={(e) =>
                onPageChange({
                  page_no: 1,
                  page_size: clampPageSize(Number(e.target.value)),
                })
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

function sortRows(
  rows: DenialSummaryRow[],
  sort: { key: SortKey; dir: SortDir },
): DenialSummaryRow[] {
  const factor = sort.dir === 'asc' ? 1 : -1;
  const acc = (r: DenialSummaryRow): string | number => {
    switch (sort.key) {
      case 'principal':
        return r.principal ?? '';
      case 'stage':
        return r.stage;
      case 'capability':
        return r.capability;
      case 'count':
        return r.count;
    }
  };
  return [...rows].sort((a, b) => {
    const av = acc(a);
    const bv = acc(b);
    if (av < bv) return -1 * factor;
    if (av > bv) return 1 * factor;
    return 0;
  });
}

function SortHeader({
  label,
  col,
  sort,
  onSort,
  className,
}: {
  label: string;
  col: SortKey;
  sort: { key: SortKey; dir: SortDir };
  onSort: (s: { key: SortKey; dir: SortDir }) => void;
  className?: string;
}) {
  const active = sort.key === col;
  return (
    <th className={cn('px-3 py-2 font-medium', className)}>
      <button
        type="button"
        onClick={() =>
          onSort({ key: col, dir: active && sort.dir === 'desc' ? 'asc' : 'desc' })
        }
        aria-sort={active ? (sort.dir === 'asc' ? 'ascending' : 'descending') : 'none'}
        className="inline-flex items-center gap-1 hover:text-text"
      >
        {label}
        {active && (sort.dir === 'asc' ? '▲' : '▼')}
      </button>
    </th>
  );
}
