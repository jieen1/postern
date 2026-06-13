import { Badge } from './Badge';
import { SnowflakeId } from './SnowflakeId';
import { TtlBadge } from './TtlBadge';
import type { CredentialRow } from '../api/types';
import { formatTime } from '../lib/format';

/**
 * Credential metadata card (设计系统 §4 / §6): shows kind / trust domain /
 * expiry / revocation only. The `secret_hash` is intentionally absent from the
 * `CredentialRow` type, so it is structurally impossible to render here — and
 * plaintext is never present at all.
 */
export function CredentialMetaCard({ cred }: { cred: CredentialRow }) {
  const revoked = cred.revoked_at !== null;
  return (
    <div className="flex flex-col gap-2 rounded-card border border-border bg-surface p-3 text-sm">
      <div className="flex items-center gap-2">
        <Badge className="border-border text-text">{cred.kind}</Badge>
        {revoked ? (
          <Badge className="border-deny/50 text-deny">revoked</Badge>
        ) : (
          <Badge className="border-allow/50 text-allow">active</Badge>
        )}
        {cred.trust_domain && (
          <span className="text-xs text-text-muted">域: {cred.trust_domain}</span>
        )}
      </div>
      <div className="flex items-center gap-2 text-xs text-text-muted">
        <span>id</span>
        <SnowflakeId id={cred.id} />
      </div>
      <div className="flex items-center gap-4 text-xs">
        <span className="text-text-muted">
          principal: <span className="text-text">{cred.principal}</span>
        </span>
        {!revoked && <TtlBadge expiresAt={cred.expires_at} />}
      </div>
      {revoked && (
        <div className="text-xs text-deny">吊销于 {formatTime(cred.revoked_at)}</div>
      )}
    </div>
  );
}
