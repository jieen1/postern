import { Clock } from 'lucide-react';
import { Link } from 'react-router-dom';
import { Card, CardHeader } from './Card';

/**
 * ExpiringGrants (01-dashboard §2 / §3.1 / §7): a JUMP-ONLY card. The control
 * plane's GET /v1/grants is per-principal / scope-bounded and CANNOT enumerate
 * "cross-principal near-expiry temp grants" (and no cross-principal temp-grant
 * aggregate read route exists in CONTROL_ROUTES). So the Dashboard does NOT
 * inline any subjects/resources/expiry rows here — it only guides the operator
 * to the Grants page, where near-expiry is handled per principal. This card
 * issues NO request, hence has no independent error/empty state.
 */
export function ExpiringGrants() {
  return (
    <Card>
      <CardHeader
        icon={<Clock size={16} className="text-warn" />}
        title="临时授权将到期"
      />
      <p className="text-sm text-text-muted">
        跨主体临时授权的临近到期需在 Grants 页按<strong className="text-text">主体维度</strong>
        查看与处理（续期 / 吊销）。
      </p>
      <Link
        to="/grants"
        className="self-start rounded-card border border-border px-3 py-1.5 text-sm text-info hover:bg-surface-2"
      >
        前往 Grants →
      </Link>
    </Card>
  );
}
