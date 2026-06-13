/**
 * Resource access/edit form: Zod schema + a tiny manual RHF resolver.
 *
 * `@hookform/resolvers` is not a scaffold dependency, so we adapt Zod to RHF's
 * Resolver signature by hand (no new package). The schema encodes the page's
 * front-end invariants (back end stays authoritative):
 *  - code required, slug-shaped, unique-in-undeleted is a back-end concern;
 *  - ≥1 read-only tier (a tier whose verbs ⊆ {observe, query}) — 设计 §6 步骤4;
 *  - real address never leaves as plaintext: the form holds a transient
 *    `address` input that the summary states will be turned into a
 *    `vault://{code}/target` reference; we never echo it back as an address.
 */

import { z } from 'zod';
import type { FieldErrors, Resolver } from 'react-hook-form';
import type { Adapter, Capability } from '../../api/types';
import { CAPABILITIES } from '../../api/types';

export const ADAPTERS: readonly Adapter[] = ['postgres', 'http', 'docker'] as const;

/** Transport options (to-reach path); real coordinates stay vault-referenced. */
export const TRANSPORTS = ['ssh', 'ssm', 'direct'] as const;
export type Transport = (typeof TRANSPORTS)[number];

/** Read-only verb set: a tier is "read-only" iff its verbs ⊆ this set. */
const READ_ONLY_VERBS: readonly Capability[] = ['observe', 'query'] as const;

/** High-risk verbs that force a danger confirm when declared (设计 §4.7). */
export const HIGH_RISK_VERBS: readonly Capability[] = ['mutate', 'manage', 'destroy'] as const;

const labelSchema = z.object({
  key: z.string().min(1, '标签键不能为空'),
  value: z.string().min(1, '标签值不能为空'),
});

const tierSchema = z.object({
  tier: z.string().min(1, 'tier 代号不能为空'),
  capabilities: z.array(z.enum(CAPABILITIES as [Capability, ...Capability[]])).min(1, '至少声明一个动词'),
});

export const resourceFormSchema = z
  .object({
    code: z
      .string()
      .min(1, '代号 code 不能为空')
      .regex(/^[a-z0-9][a-z0-9-]*$/, '代号仅限小写字母/数字/连字符'),
    adapter: z.enum(ADAPTERS as [Adapter, ...Adapter[]]),
    transport: z.enum(TRANSPORTS),
    engine_enforced: z.boolean(),
    address: z.string().trim(),
    labels: z.array(labelSchema),
    tiers: z.array(tierSchema).min(1, '至少声明一个 tier'),
  })
  .superRefine((val, ctx) => {
    // 设计 §6 步骤4: every resource needs ≥1 read-only tier (verbs ⊆ observe/query).
    const hasReadOnly = val.tiers.some(
      (t) =>
        t.capabilities.length > 0 &&
        t.capabilities.every((c) => READ_ONLY_VERBS.includes(c)),
    );
    if (!hasReadOnly) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ['tiers'],
        message: '每个资源至少需要一个只读 tier（动词仅含 observe/query）',
      });
    }
  });

export type ResourceFormValues = z.infer<typeof resourceFormSchema>;

/** Does this form declare any high-risk verb across its tiers? */
export function declaresHighRisk(values: ResourceFormValues): Capability[] {
  const set = new Set<Capability>();
  for (const t of values.tiers) {
    for (const c of t.capabilities) {
      if (HIGH_RISK_VERBS.includes(c)) set.add(c);
    }
  }
  return [...set];
}

/**
 * Manual Zod→RHF resolver (no @hookform/resolvers dependency). Maps Zod issues
 * to RHF's `FieldErrors` by joined path key; top-level / array-root issues land
 * on their first path segment so the section shows the message.
 */
export const resourceResolver: Resolver<ResourceFormValues> = (values) => {
  const parsed = resourceFormSchema.safeParse(values);
  if (parsed.success) {
    return { values: parsed.data, errors: {} };
  }
  const errors: FieldErrors<ResourceFormValues> = {};
  for (const issue of parsed.error.issues) {
    const key = issue.path[0];
    if (key === undefined) continue;
    const field = String(key) as keyof ResourceFormValues;
    // Keep the FIRST issue per field (RHF convention).
    if (!(field in errors)) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (errors as any)[field] = { type: 'zod', message: issue.message };
    }
  }
  return { values: {}, errors };
};
