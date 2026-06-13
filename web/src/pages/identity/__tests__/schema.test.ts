import { describe, expect, it } from 'vitest';
import type { CredentialRow } from '../../../api/types';
import {
  NEAR_EXPIRY_MS,
  credentialSchema,
  deriveCredentialStatus,
  principalSchema,
  tallyCredentials,
  zodResolver,
} from '../schema';

const NOW = Date.parse('2026-06-14T00:00:00Z');

function cred(over: Partial<CredentialRow>): CredentialRow {
  return {
    id: '7300000000000000789',
    principal: 'agent-order-bot',
    principal_id: '7300000000000000123',
    kind: 'api_key',
    trust_domain: 'mcp-local',
    expires_at: null,
    revoked_at: null,
    version: 1,
    ...over,
  };
}

describe('deriveCredentialStatus (§3.2 派生状态机)', () => {
  it('revoked is terminal and outranks expiry', () => {
    // Revoked AND past-expiry → still "revoked" (吊销终态优先).
    expect(
      deriveCredentialStatus(
        cred({ revoked_at: '2026-06-13T00:00:00Z', expires_at: '2020-01-01T00:00:00Z' }),
        NOW,
      ),
    ).toBe('revoked');
  });

  it('null expiry is active (长期有效)', () => {
    expect(deriveCredentialStatus(cred({ expires_at: null }), NOW)).toBe('active');
  });

  it('past expiry is expired (fail-closed, not active)', () => {
    expect(
      deriveCredentialStatus(cred({ expires_at: '2026-06-13T00:00:00Z' }), NOW),
    ).toBe('expired');
  });

  it('within near-expiry window turns to near_expiry (warn)', () => {
    const soon = new Date(NOW + NEAR_EXPIRY_MS - 60_000).toISOString();
    expect(deriveCredentialStatus(cred({ expires_at: soon }), NOW)).toBe('near_expiry');
  });

  it('far future expiry is active', () => {
    const far = new Date(NOW + NEAR_EXPIRY_MS * 10).toISOString();
    expect(deriveCredentialStatus(cred({ expires_at: far }), NOW)).toBe('active');
  });
});

describe('tallyCredentials (§3.1 凭证数：生效计数)', () => {
  it('counts active/revoked/expired and treats near_expiry as still active', () => {
    const soon = new Date(NOW + NEAR_EXPIRY_MS - 60_000).toISOString();
    const t = tallyCredentials(
      [
        cred({ expires_at: null }), // active
        cred({ revoked_at: '2026-06-13T00:00:00Z' }), // revoked
        cred({ expires_at: '2026-06-13T00:00:00Z' }), // expired
        cred({ expires_at: soon }), // near_expiry (still active)
      ],
      NOW,
    );
    expect(t.active).toBe(2); // active + near_expiry
    expect(t.revoked).toBe(1);
    expect(t.expired).toBe(1);
    expect(t.near_expiry).toBe(1);
  });
});

describe('principalSchema 归一化校验', () => {
  it('rejects empty / symbol-leading names', () => {
    expect(principalSchema.safeParse({ name: '', kind: 'agent' }).success).toBe(false);
    expect(principalSchema.safeParse({ name: '-bad', kind: 'agent' }).success).toBe(false);
  });
  it('accepts a valid identifier name + kind', () => {
    const r = principalSchema.safeParse({ name: 'agent1', kind: 'agent' });
    expect(r.success).toBe(true);
  });
});

describe('credentialSchema kind-自适应', () => {
  it('requires a token value only for kind=token', () => {
    expect(
      credentialSchema.safeParse({
        kind: 'token',
        trust_domain: 'local',
        ttl_days: '',
        secret: '',
      }).success,
    ).toBe(false);
    expect(
      credentialSchema.safeParse({
        kind: 'api_key',
        trust_domain: 'local',
        ttl_days: '',
        secret: '',
      }).success,
    ).toBe(true);
  });
  it('clamps ttl_days to 1..3650, empty = 长期有效', () => {
    expect(
      credentialSchema.safeParse({ kind: 'api_key', trust_domain: 'l', ttl_days: '0' }).success,
    ).toBe(false);
    expect(
      credentialSchema.safeParse({ kind: 'api_key', trust_domain: 'l', ttl_days: '4000' }).success,
    ).toBe(false);
    expect(
      credentialSchema.safeParse({ kind: 'api_key', trust_domain: 'l', ttl_days: '' }).success,
    ).toBe(true);
  });
});

describe('zodResolver (本地 RHF 适配)', () => {
  it('returns values on success, field errors on failure', async () => {
    const resolver = zodResolver(principalSchema);
    // await tolerates RHF's sync-or-Promise Resolver return type.
    const ok = await resolver({ name: 'agent1', kind: 'agent' }, undefined, {} as never);
    expect(ok.values).toEqual({ name: 'agent1', kind: 'agent' });
    expect(ok.errors).toEqual({});

    const bad = await resolver({ name: '', kind: 'agent' }, undefined, {} as never);
    expect(Object.keys(bad.errors)).toContain('name');
  });
});
