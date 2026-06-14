import { CredentialMetaCard } from '../../components';
import type { CredentialRow } from '../../api/types';
import { CredentialStatusBadge } from './CredentialStatusBadge';
import { deriveCredentialStatus } from './schema';

/**
 * 右栏每条凭证：复用基座 `CredentialMetaCard`（元数据 + 永不显 secret_hash/明文）
 * 外加本页派生状态徽章与内联行操作按钮（吊销 / 删除）。
 *
 * 终态纪律（§3.2 / §6.2 吊销不可逆边界）：已吊销凭证的行操作 **收起**——既无
 * "吊销"也无"取消吊销"，反向动作 UI 物理不提供。删除（逻辑删除）与吊销并列，
 * 语义不同，由调用方分别确认。
 */
export function CredentialActionCard({
  cred,
  now = Date.now(),
  onRevoke,
  onDelete,
}: {
  cred: CredentialRow;
  now?: number;
  onRevoke: (cred: CredentialRow) => void;
  onDelete: (cred: CredentialRow) => void;
}) {
  const status = deriveCredentialStatus(cred, now);
  const revoked = status === 'revoked';

  return (
    <div className="flex items-start justify-between gap-2">
      <div className="flex-1">
        <CredentialMetaCard cred={cred} />
      </div>
      <div className="flex flex-col items-end gap-2 pt-1">
        <CredentialStatusBadge status={status} />
        {/* 终态：已吊销不再提供行操作（不可再吊销、不可反向）。 */}
        {!revoked && (
          <div className="flex items-center gap-1">
            <button
              type="button"
              aria-label="吊销凭证"
              onClick={() => onRevoke(cred)}
              className="rounded-card border border-border px-2 py-1 text-xs text-deny hover:bg-surface-2"
            >
              吊销
            </button>
            <button
              type="button"
              aria-label="删除凭证"
              onClick={() => onDelete(cred)}
              className="rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
            >
              删除
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
