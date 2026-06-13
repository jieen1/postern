import { describe, expect, it } from 'vitest';
import type { Role, RoleCapability } from '../../../api/types';
import {
  isAdminName,
  isNarrowRole,
  previewEffective,
  roleFormSchema,
  SELECTABLE_CAPABILITIES,
} from '../lib';

describe('isAdminName (§六-A convenience guard, real refusal is daemon)', () => {
  it.each(['admin', 'Admin', 'ADMIN', '  admin ', '\tadmin\n'])(
    'rejects the admin variant %j',
    (v) => expect(isAdminName(v)).toBe(true),
  );
  it.each(['administrator', 'admins', 'operator', 'admin-readonly'])(
    'allows the non-admin name %j',
    (v) => expect(isAdminName(v)).toBe(false),
  );
});

describe('SELECTABLE_CAPABILITIES — destroy/admin excluded', () => {
  it('offers exactly the five role-eligible verbs, never destroy', () => {
    expect(SELECTABLE_CAPABILITIES).toEqual(['observe', 'query', 'mutate', 'execute', 'manage']);
    expect(SELECTABLE_CAPABILITIES).not.toContain('destroy');
  });
});

describe('roleFormSchema (RHF+Zod)', () => {
  it('requires at least one capability', () => {
    const r = roleFormSchema.safeParse({ name: 'x', capabilities: [], inherits_from: [] });
    expect(r.success).toBe(false);
    if (!r.success) {
      expect(r.error.issues.some((i) => /至少勾选一个动词/.test(i.message))).toBe(true);
    }
  });
  it('rejects empty name and admin name', () => {
    expect(roleFormSchema.safeParse({ name: '', capabilities: [{ capability: 'observe', action: 'allow' }], inherits_from: [] }).success).toBe(false);
    expect(roleFormSchema.safeParse({ name: 'admin', capabilities: [{ capability: 'observe', action: 'allow' }], inherits_from: [] }).success).toBe(false);
  });
  it('accepts a valid role', () => {
    const r = roleFormSchema.safeParse({
      name: 'analyst',
      capabilities: [{ capability: 'observe', action: 'allow' }],
      inherits_from: ['observer'],
    });
    expect(r.success).toBe(true);
  });
});

describe('previewEffective (input aid only — daemon authoritative)', () => {
  const observer: Role = {
    id: '1', name: 'observer',
    effective: [
      { capability: 'observe', action: 'allow' },
      { capability: 'query', action: 'allow' },
    ],
    direct: [], inherits_from: [], version: 0, updated_at: null, updated_by: null,
  };
  const byName = new Map([['observer', observer]]);

  it('merges self ⊕ parent effective, capability-ordered, dedup', () => {
    const own: RoleCapability[] = [{ capability: 'mutate', action: 'allow' }];
    const out = previewEffective(own, ['observer'], byName);
    expect(out.map((c) => c.capability)).toEqual(['observe', 'query', 'mutate']);
  });

  it('own allow wins over inherited escalate for the same verb (more permissive)', () => {
    const parent: Role = { ...observer, name: 'p', effective: [{ capability: 'mutate', action: 'escalate' }] };
    const m = new Map([['p', parent]]);
    const out = previewEffective([{ capability: 'mutate', action: 'allow' }], ['p'], m);
    expect(out).toEqual([{ capability: 'mutate', action: 'allow' }]);
  });

  it('ignores unknown parent names (no throw)', () => {
    const out = previewEffective([{ capability: 'observe', action: 'allow' }], ['ghost'], byName);
    expect(out).toEqual([{ capability: 'observe', action: 'allow' }]);
  });
});

describe('isNarrowRole', () => {
  const base: Role = { id: '1', name: 'r', effective: [], direct: [], inherits_from: [], version: 0, updated_at: null, updated_by: null };
  it('narrow when no inheritance', () => expect(isNarrowRole(base)).toBe(true));
  it('not narrow when it inherits', () => expect(isNarrowRole({ ...base, inherits_from: ['x'] })).toBe(false));
});
