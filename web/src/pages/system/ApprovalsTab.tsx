import { useState } from 'react';
import { RefreshCw, Info } from 'lucide-react';
import {
  DataTable,
  EmptyState,
  ErrorState,
  CapabilityBadge,
  DecisionBadge,
  TtlBadge,
  SnowflakeId,
  FormDrawer,
  ConfirmDialog,
  ResourceCodeBadge,
  type Column,
} from '../../components';
import { useApprovals, useSettings } from '../../api/hooks';
import { ConflictError } from '../../api/client';
import { useAdjudicateApproval, type AdjudicateWrite } from './hooks';
import type { ApprovalItem, PageQuery, SettingRow } from '../../api/types';

/**
 * Tab: Approvals (审批队列). Default-disabled → almost always an EmptyState.
 * escalate folds to deny (ESCALATE_FOLDS_TO_DENY); on-timeout / restart are
 * permanently deny — the UI offers no "timeout allow". Row `[裁决]` appears
 * ONLY when approval.enabled=true. Adjudication carries the item's optimistic
 * version; "本次允许" (allow_once) is a danger action → ConfirmDialog. 409 →
 * "他人已改，请刷新重读".
 */
export function ApprovalsTab() {
  const [page, setPage] = useState<PageQuery>({ page_no: 1, page_size: 20 });
  const settings = useSettings();
  const approvals = useApprovals(page);
  const adjudicate = useAdjudicateApproval();

  const [active, setActive] = useState<ApprovalItem | null>(null);

  const approvalEnabled = readApprovalEnabled(settings.data);

  const columns: Column<ApprovalItem>[] = [
    {
      key: 'id',
      header: 'id',
      render: (r) => <SnowflakeId id={r.id} />,
    },
    { key: 'principal', header: 'principal', render: (r) => r.principal },
    {
      key: 'resource',
      header: 'resource',
      render: (r) => <ResourceCodeBadge code={r.resource} />,
    },
    {
      key: 'capability',
      header: 'capability',
      render: (r) => <CapabilityBadge capability={r.capability} />,
    },
    {
      key: 'status',
      header: '状态',
      render: (r) => <DecisionBadge decision="escalate_denied" reason={r.status} />,
    },
    {
      key: 'expires_at',
      header: '到期 (TTL)',
      render: (r) => <TtlBadge expiresAt={r.expires_at} />,
    },
  ];

  // The settings read is the source of approvalEnabled; if it errors we
  // fail-closed and treat approval as disabled (no adjudication controls).
  return (
    <section aria-label="审批队列" className="flex flex-col gap-3">
      <header className="flex items-center justify-between">
        <h2 className="text-lg font-medium">审批队列 Approvals</h2>
        <button
          type="button"
          onClick={() => approvals.refetch()}
          className="inline-flex items-center gap-1 rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
        >
          <RefreshCw size={14} />
          刷新
        </button>
      </header>

      <div className="flex items-start gap-2 rounded-card border border-info/40 bg-info/5 px-3 py-2 text-sm text-text-muted">
        <Info size={16} className="mt-0.5 shrink-0 text-info" />
        <p>
          审批默认关闭。escalate 单元恒折叠为 deny，挂起项超时恒拒、重启恒拒。
          当前 approval.enabled = <span className="font-mono text-text">{String(approvalEnabled)}</span>
          （见「设置」Tab）。
        </p>
      </div>

      {approvals.isError ? (
        <ErrorState
          message={(approvals.error as Error).message}
          onRetry={() => approvals.refetch()}
        />
      ) : !approvals.isLoading && (approvals.data?.total ?? 0) === 0 ? (
        <EmptyState
          title="审批未启用，无挂起项。"
          hint="预设决定一切——escalate 在审批关闭下即刻 deny，无需人工裁决。如需启用，去「设置」Tab 开启 approval.enabled（高危确认）。"
        />
      ) : (
        <DataTable<ApprovalItem>
          columns={columns}
          rows={approvals.data?.items ?? []}
          total={approvals.data?.total ?? 0}
          page={page}
          onPageChange={setPage}
          rowKey={(r) => r.id}
          loading={approvals.isLoading}
          rowActions={
            approvalEnabled
              ? (row) => (
                  <button
                    type="button"
                    onClick={() => setActive(row)}
                    className="rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
                  >
                    裁决
                  </button>
                )
              : undefined
          }
        />
      )}

      {active && (
        <AdjudicateDrawer
          item={active}
          onClose={() => setActive(null)}
          onSubmit={(body) =>
            adjudicate.mutateAsync(body).then(
              () => {
                setActive(null);
                adjudicate.reset();
              },
              () => {
                /* error surfaced via adjudicate.error in the drawer */
              },
            )
          }
          pending={adjudicate.isPending}
          error={adjudicate.error}
        />
      )}
    </section>
  );
}

function readApprovalEnabled(rows: SettingRow[] | undefined): boolean {
  const row = rows?.find((r) => r.key === 'approval.enabled');
  return row?.value === 'true';
}

function AdjudicateDrawer({
  item,
  onClose,
  onSubmit,
  pending,
  error,
}: {
  item: ApprovalItem;
  onClose: () => void;
  onSubmit: (body: AdjudicateWrite) => void;
  pending: boolean;
  error: unknown;
}) {
  // Default decision is deny (the safe side); allow_once is a danger escalation.
  const [decision, setDecision] = useState<'deny' | 'allow_once'>('deny');
  const [confirming, setConfirming] = useState(false);

  // We do not have a per-item `version` field on ApprovalItem in the contract;
  // policy_rev is the optimistic anchor the daemon checks for adjudication.
  // It is a snowflake-discipline u64 (string) — pass it through verbatim,
  // NEVER Number()-parse it (>2^53 silently drops precision and would corrupt
  // the optimistic-lock anchor / stale-write detection). See base §3.4/§8.
  const version = item.policy_rev;
  const conflict = error instanceof ConflictError;

  function submit() {
    if (decision === 'allow_once') {
      setConfirming(true);
      return;
    }
    onSubmit({ id: item.id, version, decision });
  }

  return (
    <FormDrawer
      open
      title="审批裁决"
      onClose={onClose}
      footer={
        <div className="flex flex-col gap-2">
          {conflict && (
            <p role="alert" className="text-xs text-deny">
              他人已改，请刷新重读后再裁决。
            </p>
          )}
          {Boolean(error) && !conflict && (
            <p role="alert" className="text-xs text-deny">
              {(error as Error).message}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              onClick={onClose}
              className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
            >
              取消
            </button>
            <button
              type="button"
              disabled={pending}
              onClick={submit}
              className="rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40"
            >
              提交裁决
            </button>
          </div>
        </div>
      }
    >
      <div className="flex flex-col gap-4 text-sm">
        <dl className="flex flex-col gap-2">
          <Fact label="id">
            <SnowflakeId id={item.id} />
          </Fact>
          <Fact label="principal">{item.principal}</Fact>
          <Fact label="resource">
            <ResourceCodeBadge code={item.resource} />
          </Fact>
          <Fact label="capability">
            <CapabilityBadge capability={item.capability} />
          </Fact>
          <Fact label="reason">
            <span className="font-mono text-xs text-text-muted">{item.status}</span>
          </Fact>
        </dl>

        <fieldset className="flex flex-col gap-2">
          <legend className="mb-1 font-medium">裁决</legend>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="decision"
              checked={decision === 'deny'}
              onChange={() => setDecision('deny')}
            />
            拒绝（默认）
          </label>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="decision"
              checked={decision === 'allow_once'}
              onChange={() => setDecision('allow_once')}
            />
            本次允许（单次放行扩权，高危）
          </label>
        </fieldset>
        <p className="text-xs text-text-muted">
          超时/未裁的项恒 deny（系统侧 on_timeout=deny），无「超时放行」。
        </p>
      </div>

      <ConfirmDialog
        open={confirming}
        title="确认：本次允许"
        body="单次放行将临时为该主体扩权一次。超时/重启仍恒 deny，绝不超时放行。"
        onConfirm={() => {
          setConfirming(false);
          onSubmit({ id: item.id, version, decision: 'allow_once' });
        }}
        onCancel={() => setConfirming(false)}
      />
    </FormDrawer>
  );
}

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[110px_1fr] items-center gap-2">
      <dt className="font-mono text-xs text-text-muted">{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}
