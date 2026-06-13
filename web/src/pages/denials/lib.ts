/**
 * Local derivations for the Denials page.
 *
 * The control-plane endpoint `GET /v1/denials/summary` returns ONLY a paged
 * envelope of `DenialSummaryRow` (the wire type — see api/types.ts). It does
 * NOT carry a resolved window range, a window-wide deny-event total, nor an
 * alert-threshold/alert-event list. The design (03-denials.md) calls for an
 * alert band and a window summary; until the backend DTO grows those fields,
 * the page derives what it CAN from the rows and degrades fail-closed for what
 * it cannot — it never fabricates a value (空缺即占位 "—", 不臆造).
 */

import type { DenialSummaryRow, DenialWindow } from '../../api/types';

/**
 * Alert threshold (matches the doc's example `≥阈值30`). A group whose `count`
 * reaches this is flagged. NOTE: derived client-side over the CURRENT page only
 * — not a server alert-event feed (that DTO does not exist yet). Documented as
 * a deviation in the page notes.
 */
export const ALERT_THRESHOLD = 30;

/** Human window label for the summary bar (echoes the selected window). */
export const WINDOW_LABEL: Record<DenialWindow, string> = {
  '24h': '近 24 小时',
  '7d': '近 7 天',
  '30d': '近 30 天',
};

export const WINDOW_OPTIONS: readonly DenialWindow[] = ['24h', '7d', '30d'] as const;

/** A stable key for one aggregation group (its four-tuple identity). */
export function groupKey(row: DenialSummaryRow): string {
  return `${row.principal_id ?? row.principal ?? '∅'}|${row.resource}|${row.stage}|${row.capability}`;
}

/** Is this group at/over the alert threshold? */
export function isAlerting(row: DenialSummaryRow): boolean {
  return row.count >= ALERT_THRESHOLD;
}

/** The over-threshold groups within the given (already-paged) rows. */
export function alertingRows(rows: DenialSummaryRow[]): DenialSummaryRow[] {
  return rows.filter(isAlerting);
}

/**
 * Mechanical `postern elevate …` template for a group. This is a fact-derived
 * COMMAND TEMPLATE (operator must fill TTL), never an executable allow button:
 * granting is always an explicit human write in Grants (E7). TTL is left as a
 * placeholder on purpose.
 */
export function elevateTemplate(row: DenialSummaryRow): string {
  const principal = row.principal ?? row.principal_id ?? '<principal>';
  return `postern elevate ${principal} --cap ${row.resource}:${row.capability} --ttl <填>`;
}

/** "—" placeholder for absent fields — never an invented value. */
export const DASH = '—';
