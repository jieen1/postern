import { useState } from 'react';
import { Ban } from 'lucide-react';
import { Link, useNavigate } from 'react-router-dom';
import { useDenials } from '../../api/hooks';
import {
  CapabilityBadge,
  DataTable,
  ResourceCodeBadge,
  StageChip,
  type Column,
} from '../../components';
import { PAGE_DEFAULT_SIZE, type DenialSummaryRow, type DenialWindow, type PageQuery } from '../../api/types';
import { Card, CardHeader } from './Card';

/**
 * DenialsTopTable (01-dashboard §2 / §3): the near-window deny aggregation
 * board — the page's largest information block, embodying "拒绝引导一切". It is
 * the only TRUE cross-principal aggregate read (GET /v1/denials/summary), with
 * forced pagination (page_no/page_size). Counts only; direction is adjudicated
 * by a human. No reason text, no real addresses, no out-of-scope existence.
 */

const WINDOWS: readonly DenialWindow[] = ['24h', '7d', '30d'] as const;

export function DenialsTopTable() {
  const [window, setWindow] = useState<DenialWindow>('7d');
  const [page, setPage] = useState<PageQuery>({ page_no: 1, page_size: PAGE_DEFAULT_SIZE });
  const navigate = useNavigate();
  const { data, isLoading, isError, error, refetch } = useDenials(window, page);

  // Jump to Audit prefiltered to this principal's deny stream (no reason
  // detail leaks on the Dashboard; it is read field-by-field on the Audit page).
  function toAudit(row: DenialSummaryRow) {
    const params = new URLSearchParams({ decision: 'deny' });
    if (row.principal) params.set('principal', row.principal);
    navigate(`/audit?${params.toString()}`);
  }

  const columns: Column<DenialSummaryRow>[] = [
    {
      key: 'principal',
      header: '主体',
      render: (r) => (
        <span className="font-mono text-xs">{r.principal ?? '—'}</span>
      ),
    },
    {
      key: 'resource',
      header: '资源',
      render: (r) => <ResourceCodeBadge code={r.resource} />,
    },
    {
      key: 'capability',
      header: '动词',
      render: (r) => <CapabilityBadge capability={r.capability} />,
    },
    {
      key: 'stage',
      header: '阶段',
      render: (r) => <StageChip stage={r.stage} />,
    },
    {
      key: 'count',
      header: '计数',
      // Default descending: highest-frequency denials surface at the top.
      sortValue: (r) => r.count,
      className: 'text-right tabular-nums',
      render: (r) => <span className="font-mono text-text">{r.count}</span>,
    },
  ];

  return (
    <Card>
      <CardHeader
        icon={<Ban size={16} className="text-deny" />}
        title="最近高频拒绝 Denials"
        action={
          <label className="flex items-center gap-2 text-xs text-text-muted">
            窗口
            <select
              value={window}
              aria-label="拒绝窗口"
              onChange={(e) => {
                setWindow(e.target.value as DenialWindow);
                setPage({ page_no: 1, page_size: page.page_size });
              }}
              className="rounded-card border border-border bg-surface px-2 py-1"
            >
              {WINDOWS.map((w) => (
                <option key={w} value={w}>
                  {w}
                </option>
              ))}
            </select>
          </label>
        }
      />

      <DataTable
        columns={columns}
        rows={data?.items ?? []}
        total={data?.total ?? 0}
        page={page}
        onPageChange={setPage}
        rowKey={(r) =>
          `${r.principal_id ?? r.principal ?? '?'}:${r.resource}:${r.capability}:${r.stage}`
        }
        loading={isLoading}
        error={isError ? { message: error instanceof Error ? error.message : undefined } : null}
        onRetry={() => void refetch()}
        emptyTitle={`近 ${window} 无拒绝记录`}
        rowActions={(row) => (
          <button
            type="button"
            onClick={() => toAudit(row)}
            className="rounded-card border border-border px-2 py-1 text-xs text-info hover:bg-surface-2"
          >
            → audit
          </button>
        )}
      />

      <div className="flex items-center justify-between text-xs text-text-muted">
        <span>仅聚合计数，方向由人裁决</span>
        <Link to="/denials" className="text-info hover:underline">
          全部 → Denials
        </Link>
      </div>
    </Card>
  );
}
