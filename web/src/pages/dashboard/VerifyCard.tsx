import { CheckCircle2, ShieldCheck, XCircle } from 'lucide-react';
import { Link } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { EmptyState } from '../../components';
import type { VerifyReport } from '../../api/types';
import { Card, CardHeader } from './Card';

/**
 * VerifyCard (01-dashboard §2 / §3): summary of the LAST red-team run. The
 * Dashboard does NOT actively trigger verify (running it is expensive) — it
 * reads the client-cached last report. The cache is populated by the Verify
 * page; until a run exists this shows "尚未运行红队自检" (never a fabricated
 * PASS). Only summary counts here; per-probe gap_notes live on the Verify page.
 *
 * The query has no `queryFn` and never refetches: it is a pure cache reader of
 * the key the Verify page writes via setQueryData(LAST_VERIFY_KEY, report).
 */
export const LAST_VERIFY_KEY = ['verify', 'last'] as const;

function useLastVerify() {
  return useQuery<VerifyReport | undefined>({
    queryKey: LAST_VERIFY_KEY,
    // This card never fetches verify (running it is expensive and belongs to
    // the Verify page). The queryFn is inert (disabled) and only present so the
    // cache reader doesn't warn; the real value is written by the Verify page.
    queryFn: () => undefined,
    enabled: false,
    initialData: undefined,
  });
}

export function VerifyCard() {
  const { data: report } = useLastVerify();

  return (
    <Card>
      <CardHeader
        icon={<ShieldCheck size={16} className="text-info" />}
        title="红队自检 Verify"
        action={
          <Link to="/verify" className="text-xs text-info hover:underline">
            运行 → Verify
          </Link>
        }
      />
      {!report ? (
        <EmptyState
          title="尚未运行红队自检"
          hint="运行是高耗动作，请到 Verify 页触发"
          action={
            <Link
              to="/verify"
              className="rounded-card border border-border px-3 py-1.5 text-sm text-info hover:bg-surface-2"
            >
              前往 Verify →
            </Link>
          }
        />
      ) : (
        <VerifySummary report={report} />
      )}
    </Card>
  );
}

function VerifySummary({ report }: { report: VerifyReport }) {
  const total = report.items.length;
  const pass = report.items.filter((i) => i.pass).length;
  const failCount = total - pass;
  return (
    <div className="flex flex-col gap-2 text-sm">
      <div className="flex items-center gap-2">
        {report.all_pass ? (
          <CheckCircle2 size={18} className="text-allow" aria-hidden />
        ) : (
          <XCircle size={18} className="text-deny" aria-hidden />
        )}
        <span className="font-mono">
          {pass}/{total} PASS
        </span>
        {failCount > 0 && (
          // Stated as fact only; no system-generated "suggestion" (§原则二).
          <span className="font-mono text-deny">✗{failCount}</span>
        )}
      </div>
      <p className="text-xs text-text-muted">
        {report.all_pass
          ? '全部防线通过'
          : `${failCount} 项失败 — 逐条 gap_note 在 Verify 页查看`}
      </p>
    </div>
  );
}
