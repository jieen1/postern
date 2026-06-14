import { useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  AlertTriangle,
  ArrowRight,
  CheckCircle2,
  HelpCircle,
  Play,
  XCircle,
} from 'lucide-react';
import { useHealth, useVerify } from '../../api/hooks';
import { ApiError } from '../../api/client';
import type { VerifyReport } from '../../api/types';
import { EmptyState, ErrorState, InlineSpinner, SnowflakeId } from '../../components';
import { cn } from '../../lib/cn';
import { formatTime } from '../../lib/format';
import { PROBE_ORDER } from './probeCatalog';
import { ProbeRow } from './ProbeRow';
import { RunVerifyConfirm } from './RunVerifyConfirm';

/**
 * 红队自检 Verify (04-verify.md).
 *
 * A single explicit, confirmable, traceable ACTION — not a list CRUD. It runs
 * the nine red-team probes against the CURRENT policy snapshot and reports each
 * PASS/FAIL with the verbatim `gap_note` on FAIL. fail-closed throughout:
 *  - never run yet → EmptyState guide,
 *  - running → button disabled + progress, stale prior result greyed,
 *  - request failed / incomplete report → ErrorState + overall "未知", never
 *    fake nine, never a fake green.
 * The verdict is ONLY ever green when a COMPLETE report with `all_pass=true`
 * arrives. The page changes no policy (no version, no 409).
 */

/** Names allowed in a well-formed report (the nine fixed probes). */
const KNOWN_PROBES = new Set<string>(PROBE_ORDER);

/**
 * Report-integrity gate (§6.2): a report is only valid when it carries exactly
 * the nine known probes with the right field shapes. Anything else is treated
 * as an "unknown" failure — a missing/extra item is NEVER counted as PASS.
 */
function validateReport(report: VerifyReport): { ok: true } | { ok: false; reason: string } {
  const { items, all_pass } = report;
  if (!Array.isArray(items)) return { ok: false, reason: '报告缺少 items 数组' };
  if (items.length !== PROBE_ORDER.length) {
    return { ok: false, reason: `报告项数为 ${items.length}，应为 ${PROBE_ORDER.length}` };
  }
  const names = new Set<string>();
  for (const it of items) {
    if (typeof it?.name !== 'string' || typeof it?.pass !== 'boolean') {
      return { ok: false, reason: '报告项字段缺失或类型错误' };
    }
    if (!KNOWN_PROBES.has(it.name)) {
      return { ok: false, reason: `报告含未知探针名 ${it.name}` };
    }
    names.add(it.name);
  }
  if (names.size !== PROBE_ORDER.length) {
    return { ok: false, reason: '报告探针名重复或缺项' };
  }
  // The verdict word must match the per-item facts (no silently-green report).
  const computed = items.every((it) => it.pass);
  if (computed !== all_pass) {
    return { ok: false, reason: 'all_pass 与逐项结果不一致' };
  }
  return { ok: true };
}

function errorMessage(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 401 || err.status === 403) {
      return '无权运行控制面动作（控制面认证失败）。';
    }
    return `自检未能运行：${err.message}`;
  }
  return '自检未能运行：控制面不可达 / 超时。';
}

export function VerifyPage() {
  const health = useHealth();
  const verify = useVerify();
  const navigate = useNavigate();

  const [confirmOpen, setConfirmOpen] = useState(false);
  // Start ts of the current/last run — anchors the audit deep-link time window.
  const [runStartedAt, setRunStartedAt] = useState<string | null>(null);
  const policyRev = health.data?.policy_rev ?? null;

  const report = verify.data ?? null;
  const integrity = useMemo(
    () => (report ? validateReport(report) : null),
    [report],
  );
  const reportValid = integrity?.ok === true;

  const running = verify.isPending;
  // Error = the request failed OR a response arrived that fails integrity.
  const errored = verify.isError || (report !== null && integrity?.ok === false);
  const neverRun = !running && !errored && report === null;

  function run() {
    setRunStartedAt(new Date().toISOString());
    verify.mutate();
  }

  function goToAudit() {
    const params = new URLSearchParams();
    params.set('principal', 'verify-probe');
    if (runStartedAt) params.set('since', runStartedAt);
    navigate(`/audit?${params.toString()}`);
  }

  // Overall verdict: green ONLY on a complete report with all_pass=true.
  const passCount = report?.items.filter((i) => i.pass).length ?? 0;
  const total = report?.items.length ?? PROBE_ORDER.length;
  const allPass = reportValid && report?.all_pass === true;

  return (
    <section className="flex flex-col gap-4">
      <header>
        <h1 className="text-2xl font-medium">红队自检 Verify</h1>
      </header>

      {/* ── 触发区 ── */}
      <div className="flex flex-col gap-3 rounded-card border border-border bg-surface p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex flex-col gap-1 text-sm">
            <span className="text-text-muted">
              {running ? '上次运行（陈旧）' : '快照'}
            </span>
            <span className="flex items-center gap-2 font-mono text-xs">
              <span className="text-text-muted">policy_rev</span>
              {policyRev ? (
                <SnowflakeId id={policyRev} />
              ) : (
                <span className="text-text-muted">—</span>
              )}
              {runStartedAt && (
                <>
                  <span className="text-text-muted">·</span>
                  <span className="text-text-muted">
                    上次运行 {formatTime(runStartedAt)}
                  </span>
                </>
              )}
            </span>
          </div>
          <button
            type="button"
            onClick={() => setConfirmOpen(true)}
            disabled={running}
            className={cn(
              'inline-flex items-center gap-2 rounded-card px-3 py-1.5 text-sm text-white',
              'bg-deny hover:enabled:brightness-110 disabled:opacity-40',
            )}
          >
            {running ? (
              <InlineSpinner label="运行中…" />
            ) : (
              <>
                <Play size={14} />
                运行自检
              </>
            )}
          </button>
        </div>
      </div>

      {/* ── 整体判定横幅 ── */}
      <VerdictBanner
        state={
          running
            ? 'running'
            : errored
              ? 'unknown'
              : neverRun
                ? 'idle'
                : allPass
                  ? 'pass'
                  : 'fail'
        }
        passCount={passCount}
        total={total}
      />

      {/* ── 结果区（三态 fail-closed）── */}
      {errored ? (
        <ErrorState
          title="自检未能运行"
          message={
            verify.isError
              ? errorMessage(verify.error)
              : `报告不完整，判定无效：${integrity && integrity.ok === false ? integrity.reason : ''}`
          }
          onRetry={() => setConfirmOpen(true)}
        />
      ) : neverRun ? (
        <EmptyState
          title="尚未运行红队自检"
          action={
            <button
              type="button"
              onClick={() => setConfirmOpen(true)}
              className="inline-flex items-center gap-2 rounded-card bg-deny px-3 py-1.5 text-sm text-white hover:brightness-110"
            >
              <Play size={14} />
              运行自检
            </button>
          }
        />
      ) : reportValid && report ? (
        <>
          <div
            className={cn('flex flex-col gap-2', running && 'opacity-50')}
            aria-busy={running}
          >
            {report.items.map((item) => (
              <ProbeRow key={item.name} item={item} />
            ))}
          </div>
          <div>
            <button
              type="button"
              onClick={goToAudit}
              className="inline-flex items-center gap-2 rounded-card border border-border px-3 py-1.5 text-sm text-info hover:bg-surface-2"
            >
              查看探针审计留痕
              <ArrowRight size={14} />
            </button>
          </div>
        </>
      ) : null}

      <RunVerifyConfirm
        open={confirmOpen}
        policyRev={policyRev}
        onCancel={() => setConfirmOpen(false)}
        onConfirm={() => {
          setConfirmOpen(false);
          run();
        }}
      />
    </section>
  );
}

type VerdictState = 'idle' | 'running' | 'pass' | 'fail' | 'unknown';

function VerdictBanner({
  state,
  passCount,
  total,
}: {
  state: VerdictState;
  passCount: number;
  total: number;
}) {
  const visual = {
    idle: {
      cls: 'border-border bg-surface text-text-muted',
      icon: <HelpCircle size={18} className="text-text-muted" />,
      label: '尚未运行',
      hint: '',
    },
    running: {
      cls: 'border-border bg-surface text-text-muted',
      icon: <HelpCircle size={18} className="text-text-muted" />,
      label: '运行中…',
      hint: '',
    },
    pass: {
      cls: 'border-allow/40 bg-allow/5 text-allow',
      icon: <CheckCircle2 size={18} className="text-allow" />,
      label: 'ALL PASS',
      hint: '',
    },
    fail: {
      cls: 'border-deny/40 bg-deny/5 text-deny',
      icon: <XCircle size={18} className="text-deny" />,
      label: 'VERIFY FAILED',
      hint: '下方标红项指出具体失败位置。',
    },
    unknown: {
      cls: 'border-warn/40 bg-warn/5 text-warn',
      icon: <AlertTriangle size={18} className="text-warn" />,
      label: '自检未能运行',
      hint: '',
    },
  }[state];

  const showCount = state === 'pass' || state === 'fail';

  return (
    <div
      role="status"
      aria-label={`整体判定：${visual.label}`}
      className={cn(
        'flex flex-col gap-1 rounded-card border px-4 py-3',
        visual.cls,
      )}
    >
      <div className="flex items-center gap-2 text-lg font-medium">
        {visual.icon}
        <span>{visual.label}</span>
        {showCount && (
          <span className="font-mono text-sm">
            ({passCount}/{total})
          </span>
        )}
      </div>
      {visual.hint && <p className="text-xs text-text-muted">{visual.hint}</p>}
    </div>
  );
}

export default VerifyPage;
