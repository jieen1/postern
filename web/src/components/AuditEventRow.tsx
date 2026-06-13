import { useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { CapabilityBadge } from './CapabilityBadge';
import { DecisionBadge } from './DecisionBadge';
import { ResourceCodeBadge } from './ResourceCodeBadge';
import { SnowflakeId } from './SnowflakeId';
import type { AuditEvent } from '../api/types';
import { formatTime } from '../lib/format';

/**
 * Audit event row (设计系统 §4 / §8): renders the full envelope; ids/digests in
 * mono. Two-phase request events (intent → outcome) pair by `request_id`; an
 * intent with no paired outcome is an orphan (deny-before-exec or an outcome
 * write failure). Pass a `pair` (intent + optional outcome) to render the
 * collapsed two-phase form.
 */
export interface AuditPair {
  intent: AuditEvent;
  outcome?: AuditEvent;
}

/** Group a reverse-chron list of `request` events into intent/outcome pairs. */
export function pairAuditEvents(events: AuditEvent[]): AuditPair[] {
  const byReq = new Map<string, AuditPair>();
  const standalone: AuditPair[] = [];
  for (const ev of events) {
    if (ev.kind !== 'request' || !ev.request_id) {
      standalone.push({ intent: ev });
      continue;
    }
    const existing = byReq.get(ev.request_id);
    // The OUTCOME phase carries a response_digest; the intent does not.
    const isOutcome = ev.response_digest !== undefined;
    if (!existing) {
      byReq.set(ev.request_id, isOutcome ? { intent: ev, outcome: ev } : { intent: ev });
    } else if (isOutcome) {
      existing.outcome = ev;
    } else {
      existing.intent = ev;
    }
  }
  return [...byReq.values(), ...standalone];
}

export function AuditEventRow({ pair }: { pair: AuditPair }) {
  const [open, setOpen] = useState(false);
  const head = pair.outcome ?? pair.intent;
  const orphan = pair.intent.kind === 'request' && !pair.outcome;

  return (
    <div className="rounded-card border border-border">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-surface-2"
      >
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span className="font-mono text-xs text-text-muted">{formatTime(head.ts)}</span>
        <span className="text-sm">{head.principal ?? '—'}</span>
        <ResourceCodeBadge code={head.resource} />
        {head.capability && <CapabilityBadge capability={head.capability} />}
        <DecisionBadge decision={head.decision} stage={head.stage} reason={head.reason} />
        {orphan && (
          <span className="rounded-badge border border-warn/40 px-2 py-0.5 text-xs text-warn">
            intent only
          </span>
        )}
        <span className="ml-auto font-mono text-xs text-text-muted">
          rev {head.policy_rev}
        </span>
      </button>

      {open && (
        <div className="grid grid-cols-2 gap-x-6 gap-y-1 border-t border-border px-3 py-2 text-xs">
          <KV k="kind" v={head.kind} />
          <KV k="entry" v={head.entry} />
          <KV k="origin" v={head.origin} />
          {head.id && (
            <div className="flex items-center gap-2">
              <span className="font-mono text-text-muted">id</span>
              <SnowflakeId id={head.id} />
            </div>
          )}
          {head.intent_digest && <KV k="intent_digest" v={head.intent_digest} mono />}
          {pair.outcome?.response_digest && (
            <KV k="response_digest" v={pair.outcome.response_digest} mono />
          )}
          {head.tier && <KV k="tier" v={head.tier} />}
          {pair.outcome?.duration_ms !== undefined && (
            <KV k="duration_ms" v={String(pair.outcome.duration_ms)} />
          )}
          {head.objects.length > 0 && <KV k="objects" v={head.objects.join(', ')} mono />}
          {head.reason && <KV k="reason" v={head.reason} mono />}
        </div>
      )}
    </div>
  );
}

function KV({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div className="flex items-center gap-2">
      <span className="font-mono text-text-muted">{k}</span>
      <span className={mono ? 'font-mono text-text' : 'text-text'}>{v}</span>
    </div>
  );
}
