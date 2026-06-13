/**
 * Roles page — local helpers (06-roles.md §三/§七).
 *
 * SPA holds ZERO security logic: admin/verb/cycle legality is daemon-owned.
 * What lives here is (a) the convenience-layer admin-name guard (Zod), (b) the
 * closed 6-verb vocabulary with `destroy` rendered un-pickable, and (c) a LOCAL
 * effective-set preview (self ⊕ inherited) used only as an input aid — the list
 * column always shows the daemon-reported `effective`, never this preview.
 */

import { z } from 'zod';
import type { Capability, GrantAction, Role, RoleCapability } from '../../api/types';

/** The narrowed verb union a role may carry — the five role-eligible verbs,
 * excluding `destroy` (06-roles.md §五) and `admin` (no variant exists). */
export type RoleVerb = 'observe' | 'query' | 'mutate' | 'execute' | 'manage';

/** The five verbs a role may actually carry. `destroy` is excluded by design
 * (06-roles.md §五: destroy never enters any role) and `admin` has no variant. */
export const SELECTABLE_CAPABILITIES: readonly RoleVerb[] = [
  'observe',
  'query',
  'mutate',
  'execute',
  'manage',
] as const;

/** `destroy` is shown in the picker but disabled (struct-level un-pickable). */
export const DISABLED_CAPABILITY: Capability = 'destroy';

export const GRANT_ACTIONS: readonly GrantAction[] = ['allow', 'escalate'] as const;

/**
 * Convenience admin-name guard (§六-A). Trims + lowercases and rejects any
 * case/whitespace variant of `admin`. This is a UI nicety ONLY; the real hard
 * refusal is `SEC_ADMIN_NOT_GRANTABLE` in the daemon.
 */
export function isAdminName(name: string): boolean {
  return name.trim().toLowerCase() === 'admin';
}

// ── Form schema (RHF + Zod) ───────────────────────────────────────────────────

/** One picked verb row in the form: the verb + its allow/escalate routing. */
export const capabilityPickSchema = z.object({
  capability: z.enum(['observe', 'query', 'mutate', 'execute', 'manage']),
  action: z.enum(['allow', 'escalate']),
});

export const roleFormSchema = z.object({
  name: z
    .string()
    .trim()
    .min(1, '名称不能为空')
    .refine((v) => !isAdminName(v), 'admin 不可作为可授予角色'),
  description: z.string().optional(),
  /** At least one verb (06-roles.md §二: 动词集至少勾 1). */
  capabilities: z.array(capabilityPickSchema).min(1, '至少勾选一个动词'),
  /** Parent role names (existing roles only); cycle legality is daemon-owned. */
  inherits_from: z.array(z.string()),
});

export type RoleFormValues = z.infer<typeof roleFormSchema>;

/** One picked verb in the form — like RoleCapability but `capability` is
 * narrowed to the five role-eligible verbs (never `destroy`/`admin`). */
export type FormCapability = z.infer<typeof capabilityPickSchema>;

// ── Local effective-set preview (input aid only — daemon is authoritative) ─────

/**
 * Merge this role's own picks with the effective sets of its named parents to
 * preview "effective = self ⊕ inherited". On conflict, the more permissive
 * action (`allow`) wins for display purposes only; the daemon recomputes the
 * real set on write. Returns a stable, capability-ordered list.
 */
export function previewEffective(
  own: RoleCapability[],
  parentNames: string[],
  rolesByName: Map<string, Role>,
): RoleCapability[] {
  const merged = new Map<Capability, GrantAction>();
  const put = (cap: Capability, action: GrantAction) => {
    const existing = merged.get(cap);
    // `allow` is more permissive than `escalate`; keep the looser one.
    if (existing === 'allow') return;
    merged.set(cap, action);
  };
  for (const rc of own) put(rc.capability, rc.action);
  for (const parentName of parentNames) {
    const parent = rolesByName.get(parentName);
    if (!parent) continue;
    for (const rc of parent.effective) put(rc.capability, rc.action);
  }
  const order: readonly Capability[] = [
    'observe',
    'query',
    'mutate',
    'execute',
    'manage',
    'destroy',
  ];
  return order
    .filter((cap) => merged.has(cap))
    .map((cap) => ({ capability: cap, action: merged.get(cap) as GrantAction }));
}

/** Whether a role is a "narrow" role (no inheritance) vs a ladder rung. */
export function isNarrowRole(role: Role): boolean {
  return role.inherits_from.length === 0;
}
