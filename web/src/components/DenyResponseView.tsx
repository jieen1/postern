import { CapabilityBadge } from './CapabilityBadge';
import { ResourceCodeBadge } from './ResourceCodeBadge';
import type { DenyResponse } from '../api/types';

/**
 * Structured deny renderer (设计系统 §4 / §8): every field shown verbatim and
 * field-by-field. `operator_note` is relayed exactly as written — never
 * reworded. `your_grants` is the principal's OWN world only (scope-bounded);
 * this view shows only what the backend sent and asserts nothing about the
 * existence of the target resource.
 */
export function DenyResponseView({ deny }: { deny: DenyResponse }) {
  const grants = Object.entries(deny.your_grants);
  return (
    <div className="flex flex-col gap-3 rounded-card border border-deny/40 bg-deny/5 p-4 text-sm">
      <Field label="decision">
        <span className="font-mono text-deny">{deny.decision}</span>
      </Field>

      <Field label="denied">
        <span className="flex flex-wrap items-center gap-2">
          <ResourceCodeBadge code={deny.denied.resource} />
          <CapabilityBadge capability={deny.denied.capability} />
          {deny.denied.objects.map((o) => (
            <span key={o} className="font-mono text-xs text-text-muted">
              {o}
            </span>
          ))}
        </span>
      </Field>

      <Field label="reason">
        <span className="font-mono text-xs text-text">{deny.reason}</span>
      </Field>

      <Field label="your_grants">
        {grants.length === 0 ? (
          <span className="text-text-muted">（空）</span>
        ) : (
          <ul className="flex flex-col gap-1">
            {grants.map(([res, caps]) => (
              <li key={res} className="flex flex-wrap items-center gap-2">
                <ResourceCodeBadge code={res} />
                {caps.map((c) => (
                  <span key={c} className="font-mono text-xs text-text-muted">
                    {c}
                  </span>
                ))}
              </li>
            ))}
          </ul>
        )}
      </Field>

      <Field label="request_hint">
        {deny.request_hint ? (
          <code className="rounded-badge bg-surface-2 px-2 py-0.5 font-mono text-xs">
            {deny.request_hint}
          </code>
        ) : (
          <span className="text-text-muted">null</span>
        )}
      </Field>

      {/* operator_note: relayed verbatim; absent when the backend omits it. */}
      {deny.operator_note !== undefined && (
        <Field label="operator_note">
          <p className="whitespace-pre-wrap text-text">{deny.operator_note}</p>
        </Field>
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[120px_1fr] gap-2">
      <span className="font-mono text-xs text-text-muted">{label}</span>
      <div>{children}</div>
    </div>
  );
}
