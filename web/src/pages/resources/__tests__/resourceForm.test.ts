import { describe, expect, it } from 'vitest';
import {
  declaresHighRisk,
  resourceFormSchema,
  resourceResolver,
  type ResourceFormValues,
} from '../resourceForm';
import { buildResourcePayload } from '../buildPayload';
import type { ResourceRow } from '../../../api/types';

function base(overrides: Partial<ResourceFormValues> = {}): ResourceFormValues {
  return {
    code: 'db-main',
    adapter: 'postgres',
    transport: 'ssm',
    engine_enforced: true,
    address: '',
    labels: [],
    tiers: [{ tier: 'ro', capabilities: ['observe', 'query'] }],
    ...overrides,
  };
}

describe('resourceFormSchema — front-end invariants', () => {
  it('accepts a valid resource with a read-only tier', () => {
    expect(resourceFormSchema.safeParse(base()).success).toBe(true);
  });

  it('rejects a non-slug code', () => {
    const res = resourceFormSchema.safeParse(base({ code: 'DB Main' }));
    expect(res.success).toBe(false);
  });

  it('requires at least one read-only tier (设计 §6 步骤4)', () => {
    // Only a write tier — no tier with verbs ⊆ {observe, query}.
    const res = resourceFormSchema.safeParse(
      base({ tiers: [{ tier: 'rw', capabilities: ['mutate'] }] }),
    );
    expect(res.success).toBe(false);
    if (!res.success) {
      expect(res.error.issues.some((i) => i.path[0] === 'tiers')).toBe(true);
    }
  });

  it('requires at least one tier', () => {
    expect(resourceFormSchema.safeParse(base({ tiers: [] })).success).toBe(false);
  });
});

describe('resourceResolver — maps Zod issues to RHF field errors', () => {
  const opts = { shouldUseNativeValidation: false, fields: {} } as Parameters<
    typeof resourceResolver
  >[2];

  it('returns no errors for valid values', async () => {
    const out = await resourceResolver(base(), undefined, opts);
    expect(out.errors).toEqual({});
  });

  it('surfaces a code error on invalid slug', async () => {
    const out = await resourceResolver(base({ code: 'Bad Code' }), undefined, opts);
    expect(out.errors.code?.message).toMatch(/代号/);
  });
});

describe('declaresHighRisk', () => {
  it('flags mutate/manage/destroy across tiers', () => {
    const v = base({
      tiers: [
        { tier: 'ro', capabilities: ['observe', 'query'] },
        { tier: 'rw', capabilities: ['mutate', 'manage'] },
      ],
    });
    expect(declaresHighRisk(v).sort()).toEqual(['manage', 'mutate']);
  });

  it('returns empty for a read-only resource', () => {
    expect(declaresHighRisk(base())).toEqual([]);
  });
});

describe('buildResourcePayload — contract discipline', () => {
  const editing: ResourceRow = {
    id: '7300000000000002001',
    code: 'db-main',
    adapter: 'postgres',
    transport: 'ssm',
    tiers: [{ tier: 'ro', capabilities: ['observe'], secret_ref: 'vault://db-main/ro' }],
    labels: [],
    enable_flag: true,
    version: 5,
  };

  it('omits version for a brand-new resource', () => {
    const body = buildResourcePayload(base(), null);
    expect(body.version).toBeUndefined();
    expect(body.enable_flag).toBe(true);
  });

  it('carries the read version for an edit (optimistic lock baseline)', () => {
    const body = buildResourcePayload(base(), editing);
    expect(body.version).toBe(5);
  });

  it('flags address_set only when an address was entered (never sends plaintext)', () => {
    const withAddr = buildResourcePayload(base({ address: '10.0.3.7:5432' }), null);
    expect(withAddr.address_set).toBe(true);
    // The body shape carries no plaintext address field at all.
    expect(JSON.stringify(withAddr)).not.toContain('10.0.3.7');

    const noAddr = buildResourcePayload(base({ address: '   ' }), null);
    expect(noAddr.address_set).toBeUndefined();
  });

  it('applies an explicit enable flag (disable path)', () => {
    const body = buildResourcePayload(base(), editing, false);
    expect(body.enable_flag).toBe(false);
    expect(body.version).toBe(5);
  });
});
