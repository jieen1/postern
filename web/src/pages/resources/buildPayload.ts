/**
 * Build the `POST /v1/resources` request body from validated form values.
 *
 * Contract discipline:
 *  - The real address is NEVER sent as a usable address field. When the operator
 *    entered one, we send `address_set: true` so the daemon mints the
 *    `vault://{code}/target` reference; the plaintext itself is not forwarded as
 *    a stored coordinate. When blank, the existing reference is left untouched.
 *  - Edits/disable carry the read `version` for optimistic locking (409 on stale).
 *  - tiers carry only verb sets here; secret plaintext is entered on page 10.
 */

import type { ResourceFormValues } from './resourceForm';
import type { ResourceRow } from '../../api/types';

export interface ResourceWriteBody {
  code: string;
  adapter: string;
  transport: string;
  engine_enforced: boolean;
  labels: { key: string; value: string }[];
  tiers: { tier: string; capabilities: string[] }[];
  enable_flag: boolean;
  /** Present only when the operator entered a new address (daemon mints vault ref). */
  address_set?: boolean;
  /** Optimistic-lock baseline; omitted for a brand-new resource. */
  version?: number;
}

export function buildResourcePayload(
  values: ResourceFormValues,
  editing: ResourceRow | null,
  enableFlag = editing?.enable_flag ?? true,
): ResourceWriteBody {
  const body: ResourceWriteBody = {
    code: values.code,
    adapter: values.adapter,
    transport: values.transport,
    engine_enforced: values.engine_enforced,
    labels: values.labels,
    tiers: values.tiers.map((t) => ({ tier: t.tier, capabilities: t.capabilities })),
    enable_flag: enableFlag,
  };
  if (values.address.trim().length > 0) {
    body.address_set = true;
  }
  if (editing) {
    body.version = editing.version;
  }
  return body;
}
