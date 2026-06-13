import { useState } from 'react';
import { MoreVertical } from 'lucide-react';
import { CredentialMetaCard } from '../../components';
import type { CredentialRow } from '../../api/types';
import { CredentialStatusBadge } from './CredentialStatusBadge';
import { deriveCredentialStatus } from './schema';

/**
 * 右栏每条凭证：复用基座 `CredentialMetaCard`（元数据 + 永不显 secret_hash/明文）
 * 外加本页派生状态徽章与行操作菜单（⋮ → 吊销 / 删除）。
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
  const [menuOpen, setMenuOpen] = useState(false);
  const status = deriveCredentialStatus(cred, now);
  const revoked = status === 'revoked';

  return (
    <div className="relative">
      <div className="flex items-start justify-between gap-2">
        <div className="flex-1">
          <CredentialMetaCard cred={cred} />
        </div>
        <div className="flex flex-col items-end gap-2 pt-1">
          <CredentialStatusBadge status={status} />
          {/* 终态：已吊销不再提供行操作（不可再吊销、不可反向）。 */}
          {!revoked && (
            <div className="relative">
              <button
                type="button"
                aria-label="凭证操作"
                aria-haspopup="menu"
                aria-expanded={menuOpen}
                onClick={() => setMenuOpen((o) => !o)}
                className="rounded-card border border-border p-1 text-text-muted hover:bg-surface-2 hover:text-text"
              >
                <MoreVertical size={16} />
              </button>
              {menuOpen && (
                <div
                  role="menu"
                  className="absolute right-0 z-10 mt-1 w-32 overflow-hidden rounded-card border border-border bg-surface text-sm shadow-lg"
                >
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      setMenuOpen(false);
                      onRevoke(cred);
                    }}
                    className="block w-full px-3 py-2 text-left text-deny hover:bg-surface-2"
                  >
                    吊销凭证
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      setMenuOpen(false);
                      onDelete(cred);
                    }}
                    className="block w-full px-3 py-2 text-left text-text hover:bg-surface-2"
                  >
                    删除凭证
                  </button>
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
