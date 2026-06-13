/**
 * Elevate (临时提权) form schema. RHF+Zod, contract-aligned:
 *  - resource + capability required (Scope-bounded select),
 *  - TTL is REQUIRED and > 0 (场景 06 异常 13「提权必须带 TTL」); the daemon
 *    hard-rejects a missing TTL — this is convenience validation only.
 *
 * The form collects a number + a unit; we convert to absolute ttl_ms for the
 * wire (`ElevateRequest.ttl_ms`).
 */

import { z } from 'zod';
import { CAPABILITIES, type Capability } from '../../api/types';

export const TTL_UNITS = {
  minute: 60_000,
  hour: 3_600_000,
  day: 86_400_000,
} as const;

export type TtlUnit = keyof typeof TTL_UNITS;

export const elevateSchema = z.object({
  resource: z.string().min(1, '请选择资源'),
  capability: z.enum(CAPABILITIES as unknown as [Capability, ...Capability[]], {
    errorMap: () => ({ message: '请选择能力' }),
  }),
  // coerce so the native number input's string value validates as a number.
  ttlValue: z.coerce
    .number({ invalid_type_error: '提权必须带 TTL' })
    .int('TTL 必须为整数')
    .positive('提权必须带 TTL（>0）'),
  ttlUnit: z.enum(['minute', 'hour', 'day']),
});

export type ElevateForm = z.infer<typeof elevateSchema>;

/** Absolute ttl_ms from the form's value + unit. */
export function ttlToMs(value: number, unit: TtlUnit): number {
  return value * TTL_UNITS[unit];
}
