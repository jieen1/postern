import { Plus } from 'lucide-react';
import { EmptyState, ErrorState, LoadingSkeleton } from '../../components';
import { ApiError } from '../../api/client';
import type { CredentialRow, PrincipalRow } from '../../api/types';
import { CredentialActionCard } from './CredentialActionCard';
import { tallyCredentials } from './schema';

/**
 * 右栏凭证区（§二 / §三 / §六.2）。承载选中主体的网关凭证，三态 fail-closed：
 *  - 未选主体：中性引导态（非错误）。
 *  - 加载中：骨架卡。
 *  - 错误：ErrorState + 明示"无法确认凭证有效/吊销状态"，**绝不**把任何凭证渲染
 *    为生效（不确定即受限）。
 *  - 空：EmptyState，如实陈述后果。
 *
 * 凭证以卡片列表呈现（非密集表），因每条字段少但状态语义重。
 */
export function CredentialPanel({
  principal,
  creds,
  loading,
  error,
  onRetry,
  onCreate,
  onRevoke,
  onDelete,
  now = Date.now(),
}: {
  principal: PrincipalRow | null;
  creds: CredentialRow[];
  loading: boolean;
  error: { message?: string } | null;
  onRetry: () => void;
  onCreate: () => void;
  onRevoke: (cred: CredentialRow) => void;
  onDelete: (cred: CredentialRow) => void;
  now?: number;
}) {
  // 未选主体：中性引导，不是错误。
  if (!principal) {
    return (
      <section aria-label="凭证" className="flex h-full flex-col">
        <h2 className="mb-3 text-lg font-medium">凭证 Credentials</h2>
        <EmptyState title="选择左侧一个主体查看其网关凭证" />
      </section>
    );
  }

  return (
    <section aria-label="凭证" className="flex h-full flex-col">
      <div className="mb-3 flex items-center justify-between">
        <h2 className="text-lg font-medium">
          凭证 Credentials · <span className="font-mono text-base">{principal.name}</span>
        </h2>
        <button
          type="button"
          onClick={onCreate}
          className="inline-flex items-center gap-1 rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
        >
          <Plus size={14} /> 新建凭证
        </button>
      </div>

      {loading ? (
        <div aria-busy="true">
          <LoadingSkeleton rows={3} />
        </div>
      ) : error ? (
        // fail-closed：错误态不显任何卡片、不把任何凭证当"生效"。
        isNotImplemented(error) ? (
          <EmptyState title="凭证功能暂不可用（开发中）" />
        ) : (
          <ErrorState
            title="凭证加载失败，无法确认状态"
            message={error.message}
            onRetry={onRetry}
          />
        )
      ) : creds.length === 0 ? (
        <EmptyState title="该主体暂无网关凭证" />
      ) : (
        <CredentialList
          creds={creds}
          now={now}
          onRevoke={onRevoke}
          onDelete={onDelete}
        />
      )}
    </section>
  );
}

function CredentialList({
  creds,
  now,
  onRevoke,
  onDelete,
}: {
  creds: CredentialRow[];
  now: number;
  onRevoke: (cred: CredentialRow) => void;
  onDelete: (cred: CredentialRow) => void;
}) {
  const tally = tallyCredentials(creds, now);
  return (
    <div className="flex flex-col gap-3">
      <div className="rounded-card border border-border bg-surface-2 p-2 text-xs text-text-muted">
        共 {creds.length} 凭证 · 生效 {tally.active} · 吊销 {tally.revoked} · 过期{' '}
        {tally.expired}
        {tally.near_expiry > 0 && (
          <span className="text-warn"> · {tally.near_expiry} 即将过期</span>
        )}
      </div>
      <ul className="flex flex-col gap-3">
        {creds.map((cred) => (
          <li key={cred.id}>
            <CredentialActionCard
              cred={cred}
              now={now}
              onRevoke={onRevoke}
              onDelete={onDelete}
            />
          </li>
        ))}
      </ul>
    </div>
  );
}

function isNotImplemented(error: { message?: string }): boolean {
  return error instanceof ApiError && error.status === 501;
}
