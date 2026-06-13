import { useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { useQueryClient } from '@tanstack/react-query';
import { formatTime } from '../../lib/format';
import { HealthCard } from './HealthCard';
import { ModePanel } from './ModePanel';
import { DenialsTopTable } from './DenialsTopTable';
import { ExpiringGrants } from './ExpiringGrants';
import { VerifyCard } from './VerifyCard';

/**
 * 总览 Dashboard (01-dashboard): observe + jump landing page. A card grid (NOT
 * a list-page skeleton) compressing daemon health, current mode posture, the
 * cross-principal deny board, near-expiry temp-grant guidance, and the last
 * red-team summary into one screen. Each card fetches and fails independently
 * and fail-closed; the Dashboard's only write (global freeze) lives in the
 * top-bar GlobalEmergencyBar — there is deliberately no second freeze control.
 */
export function DashboardPage() {
  const qc = useQueryClient();
  const [updatedAt, setUpdatedAt] = useState(() => Date.now());

  // [刷新 ⟳] invalidates the read sources (health/mode/denials) and refetches.
  // ExpiringGrants issues no request and is not part of the refresh.
  function refresh() {
    void qc.invalidateQueries({ queryKey: ['health'] });
    void qc.invalidateQueries({ queryKey: ['mode-state'] });
    void qc.invalidateQueries({ queryKey: ['denials'] });
    setUpdatedAt(Date.now());
  }

  return (
    <section className="flex flex-col gap-4">
      <header className="flex items-center gap-4">
        <h1 className="text-2xl font-medium">总览 Dashboard</h1>
        <div className="ml-auto flex items-center gap-3 text-xs text-text-muted">
          <span>
            最后更新 <time>{formatTime(updatedAt)}</time>
          </span>
          <button
            type="button"
            onClick={refresh}
            className="inline-flex items-center gap-1 rounded-card border border-border px-3 py-1.5 text-sm text-text hover:bg-surface-2"
          >
            <RefreshCw size={14} aria-hidden />
            刷新
          </button>
        </div>
      </header>

      {/* Row 1 — system posture: health + mode. */}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <HealthCard />
        <ModePanel />
      </div>

      {/* Row 2 — deny board (primary, wide) + right rail (grants + verify). */}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <div className="lg:col-span-2">
          <DenialsTopTable />
        </div>
        <div className="flex flex-col gap-4">
          <ExpiringGrants />
          <VerifyCard />
        </div>
      </div>
    </section>
  );
}

export default DashboardPage;
