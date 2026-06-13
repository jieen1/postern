/**
 * Scope-spec construction helpers (07-bindings.md §2.2, §七 契约).
 *
 * Two Scope kinds map to the backend `Scope` enum:
 *  - selector → `Scope::Selector(String)`: the SPA submits the RAW JSON text
 *    `{"all":[{"key":..,"value":..}]}` — what-you-see-is-what-you-send. Keys are
 *    limited to the three semantic prefixes host/env/kind (controlled dropdown).
 *  - resource → `Scope::Resources(Vec<ResourceCode>)`: a multi-select of codes,
 *    submitted as a comma-joined code list.
 *
 * The SPA holds ZERO expansion logic: it only BUILDS the spec text it will send
 * and renders the daemon's reported expansion. It never derives a resource set.
 */

import type { ScopeKind } from '@/api/types';

/** The three allowed selector key prefixes (controlled). */
export const SELECTOR_KEYS = ['host', 'env', 'kind'] as const;
export type SelectorKey = (typeof SELECTOR_KEYS)[number];

/** One selector match row: a `key:value` pair. */
export interface SelectorRow {
  key: SelectorKey;
  value: string;
}

/**
 * Build the raw selector JSON spec text the SPA will submit: an `{all:[...]}`
 * (all-must-match) object. Empty-value rows are dropped (they would be a no-op
 * label). The output is stable-keyed so the JsonViewer preview is deterministic.
 */
export function buildSelectorSpec(rows: SelectorRow[]): string {
  const all = rows
    .filter((r) => r.value.trim() !== '')
    .map((r) => ({ key: r.key, value: r.value.trim() }));
  return JSON.stringify({ all });
}

/** Build the resource-kind spec: comma-joined resource codes. */
export function buildResourceSpec(codes: string[]): string {
  return codes.filter((c) => c.trim() !== '').join(',');
}

/**
 * Whether the current draft has enough to attempt a preview/submit:
 *  - selector: at least one non-empty row,
 *  - resource: at least one code.
 */
export function hasScopeContent(
  kind: ScopeKind,
  rows: SelectorRow[],
  codes: string[],
): boolean {
  if (kind === 'selector') {
    return rows.some((r) => r.value.trim() !== '');
  }
  return codes.some((c) => c.trim() !== '');
}

/** Parse a list-row `scope_spec` (resource kind) back into codes for display. */
export function parseResourceSpec(spec: string): string[] {
  return spec
    .split(',')
    .map((c) => c.trim())
    .filter((c) => c !== '');
}
