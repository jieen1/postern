import { describe, expect, it } from 'vitest';
import { pairAuditEvents } from './AuditEventRow';
import type { AuditEvent } from '../api/types';

function ev(over: Partial<AuditEvent>): AuditEvent {
  return {
    v: 1,
    kind: 'request',
    entry: 'mcp',
    origin: 'unix:uid=1000',
    principal: 'p',
    resource: 'db-main',
    capability: 'query',
    objects: [],
    decision: 'allow',
    stage: null,
    reason: '',
    policy_rev: '1',
    ...over,
  };
}

describe('pairAuditEvents (两阶段 intent/outcome 配对)', () => {
  it('pairs an intent with its outcome by request_id', () => {
    const intent = ev({ request_id: 'r1', id: 'i1' });
    const outcome = ev({ request_id: 'r1', id: 'o1', response_digest: 'sha:1', duration_ms: 9 });
    const pairs = pairAuditEvents([outcome, intent]);
    expect(pairs).toHaveLength(1);
    expect(pairs[0]!.intent.request_id).toBe('r1');
    expect(pairs[0]!.outcome?.response_digest).toBe('sha:1');
  });

  it('keeps an intent-only event as an orphan (deny before exec)', () => {
    const intent = ev({ request_id: 'r2', id: 'i2', decision: 'deny', stage: 'rbac' });
    const pairs = pairAuditEvents([intent]);
    expect(pairs).toHaveLength(1);
    expect(pairs[0]!.outcome).toBeUndefined();
  });

  it('treats non-request events as standalone', () => {
    const pc = ev({ kind: 'policy_change', request_id: undefined });
    const pairs = pairAuditEvents([pc]);
    expect(pairs).toHaveLength(1);
    expect(pairs[0]!.intent.kind).toBe('policy_change');
  });
});
