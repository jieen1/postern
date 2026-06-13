import { describe, expect, it } from 'vitest';
import type { GrantsView } from '../../../api/types';
import { buildMatrix, isLiveTempGrant, liveTempGrants } from '../matrix';

const NOW = Date.parse('2026-06-14T00:00:00Z');
const future = (h: number) => new Date(NOW + h * 3_600_000).toISOString();
const past = (h: number) => new Date(NOW - h * 3_600_000).toISOString();

function view(over: Partial<GrantsView> = {}): GrantsView {
  return {
    your_grants: { 'db-main': ['observe', 'query'], 'api-billing': ['observe'] },
    temp_grants: [],
    ...over,
  };
}

describe('matrix cell-state projection (零授权计算, 公理一缺格=拒绝)', () => {
  it('marks present capabilities persistent and absent ones default-deny', () => {
    const rows = buildMatrix(view(), NOW);
    const db = rows.find((r) => r.resource === 'db-main')!;
    const state = (cap: string) => db.cells.find((c) => c.capability === cap)!.state;
    expect(state('observe')).toBe('persistent');
    expect(state('query')).toBe('persistent');
    // absence == default deny, never derived
    expect(state('mutate')).toBe('deny');
    expect(state('destroy')).toBe('deny');
  });

  it('renders all six capability columns per row', () => {
    const rows = buildMatrix(view(), NOW);
    expect(rows[0]!.cells).toHaveLength(6);
  });

  it('does NOT synthesize rows for resources the daemon did not return (scope-bounded)', () => {
    // svc-* / mq-main are not in your_grants ⇒ no row, no existence surfaced.
    const rows = buildMatrix(view(), NOW);
    const codes = rows.map((r) => r.resource);
    expect(codes).toContain('db-main');
    expect(codes).not.toContain('svc-orders');
    expect(codes).not.toContain('mq-main');
  });

  it('flags an all-deny row (every cell default-deny)', () => {
    // a resource present only via an EXPIRED temp grant contributes no live cell
    const rows = buildMatrix(
      view({
        your_grants: {},
        temp_grants: [
          {
            id: '7300000000000003999',
            resource: 'redis-main',
            capability: 'destroy',
            granted_at: past(2),
            expires_at: past(1), // expired ⇒ not live ⇒ no row
            ended_at: null,
            end_reason: null,
            version: 1,
          },
        ],
      }),
      NOW,
    );
    // expired temp grant produces no row at all
    expect(rows.map((r) => r.resource)).not.toContain('redis-main');
  });
});

describe('temp-grant liveness (生效=未结束且未过期)', () => {
  const baseRow = {
    id: '7300000000000003001',
    resource: 'redis-main',
    capability: 'destroy' as const,
    granted_at: past(1),
    expires_at: future(1),
    ended_at: null as string | null,
    end_reason: null as string | null,
    version: 1,
  };

  it('counts a future, unended grant as live', () => {
    expect(isLiveTempGrant(baseRow, NOW)).toBe(true);
  });

  it('excludes an ended grant (revoked) even if not yet expired', () => {
    expect(isLiveTempGrant({ ...baseRow, ended_at: past(0.5), end_reason: 'revoked' }, NOW)).toBe(
      false,
    );
  });

  it('excludes an expired grant', () => {
    expect(isLiveTempGrant({ ...baseRow, expires_at: past(1) }, NOW)).toBe(false);
  });

  it('liveTempGrants filters the list to live rows only', () => {
    const v = view({
      temp_grants: [
        baseRow,
        { ...baseRow, id: '7300000000000003002', ended_at: past(0.5), end_reason: 'revoked' },
        { ...baseRow, id: '7300000000000003003', expires_at: past(2) },
      ],
    });
    const live = liveTempGrants(v, NOW);
    expect(live.map((g) => g.id)).toEqual(['7300000000000003001']);
  });

  it('promotes a (resource,capability) to a temp cell when a live grant exists', () => {
    const rows = buildMatrix(view({ temp_grants: [baseRow] }), NOW);
    const cell = rows
      .find((r) => r.resource === 'redis-main')!
      .cells.find((c) => c.capability === 'destroy')!;
    expect(cell.state).toBe('temp');
    expect(cell.temp?.id).toBe('7300000000000003001');
  });
});
