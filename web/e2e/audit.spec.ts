import { test, expect, type Locator, type Page } from '@playwright/test';

/**
 * Audit (02-audit) — REAL-browser e2e against the shared Vite+MSW dev server
 * (:5173). MSW serves static fixtures from src/mocks/fixtures.ts, so this spec
 * only exercises the happy-path + cross-page DOM contracts that the default
 * fixtures render reliably (error/empty/paging-boundary states are covered by
 * vitest with per-test server.use()).
 *
 * Default audit fixtures (fixtures.ts `auditEvents`, total=3):
 *   - req-aa01: an `allow` request whose intent + outcome PAIR into ONE row
 *               (outcome carries response_digest + duration_ms=23).
 *   - req-aa02: a `deny` request (stage=rbac) — its own row.
 *   ⇒ 2 list items, filter-bar count "匹配 3 条", footer "共 3 条 · 第 1/1 页".
 *
 * Row expansion is the FULL row (a single <button> toggle) — clicking anywhere
 * on it, INCLUDING the decision-badge region, toggles the row. The badge is an
 * inert element inside the row (no nested interactive), so a centered click on
 * the badge no longer gets swallowed. The deny row's 落点 stage is surfaced in
 * the row's expanded panel (it is no longer carried by an expandable badge).
 */

const ID_DENY = '7300000000000004003'; // fixtures.ts ID.auditDeny — snowflake > 2^53
const ID_OUTCOME = '7300000000000004002'; // fixtures.ts ID.auditOutcome (paired row's head)
const DENY_REASON =
  'denied at rbac: no grant cell (db-main, mutate) for binding observer';

/** Navigate to the audit page from the app root and wait for REAL rows. */
async function gotoAudit(page: Page) {
  await page.goto('/');
  await page.getByRole('link', { name: /审计 Audit/ }).click();
  await expect(page.getByRole('heading', { name: '审计 Audit' })).toBeVisible();
  // Wait for REAL data (not skeleton/empty): the two list items must appear.
  await expect(page.getByRole('listitem')).toHaveCount(2);
}

/**
 * Expand a row by clicking its DECISION BADGE — the horizontal-center region
 * that used to be a nested button and swallow the toggle. Clicking it now
 * toggles the whole row (the badge is inert). Waits for the expanded grid.
 * `decision` is the badge label ('allow' / 'deny') the row carries.
 */
async function expandRow(row: Locator, decision: 'allow' | 'deny') {
  await row.getByText(decision, { exact: true }).click();
  await expect(row.locator('.grid')).toHaveCount(1);
}

test('event stream renders REAL fixture rows (not skeleton/empty), with intent/outcome paired', async ({
  page,
}) => {
  await gotoAudit(page);

  // Real data from fixtures, reverse-chron, intent+outcome collapsed to 1 row.
  const rows = page.getByRole('listitem');
  await expect(rows).toHaveCount(2);

  // The filter-bar live count and the forced-pagination footer both reflect
  // total=3 (2 rows because req-aa01's intent+outcome pair into one).
  await expect(page.getByText(/匹配 3 条/)).toBeVisible();
  await expect(page.getByText(/共 3 条 · 第 1\/1 页/)).toBeVisible();

  // Real envelope values are visible on the collapsed rows.
  await expect(page.getByText('agent-order-bot').first()).toBeVisible();
  // DecisionBadges from fixtures: an allow row and a deny row.
  await expect(rows.nth(0).getByText('allow')).toBeVisible();
  await expect(rows.nth(1).getByText('deny')).toBeVisible();
  // Resource code badge — codename only (never an address; see DOM contract).
  await expect(rows.nth(0).getByText('db-main').first()).toBeVisible();
});

test('applying a filter narrows the list and keeps the forced-pagination footer', async ({
  page,
}) => {
  await gotoAudit(page);

  // Filter principal=agent3 + decision=deny + apply. Browser MSW is static and
  // ignores these params, but the REAL user flow must still: reset to page 1,
  // re-query, and converge to a rendered result (here the same fixtures).
  await page.getByLabel(/principal/).fill('agent3');
  await page.getByText('deny', { exact: true }).first().click(); // segmented radio label
  await page.getByRole('button', { name: /应用/ }).click();

  // After applying, the list re-renders REAL rows (not error/blank) and the
  // footer stays at page 1 — the forced-pagination contract is never dropped.
  await expect(page.getByRole('listitem')).toHaveCount(2);
  await expect(page.getByText(/共 3 条 · 第 1\/1 页/)).toBeVisible();

  // The decision segment reflects the chosen value (deny radio is checked).
  await expect(page.getByRole('radio', { name: 'deny' })).toBeChecked();
  await expect(page.getByLabel(/principal/)).toHaveValue('agent3');

  // 清空 returns the draft to defaults and re-queries to page 1.
  await page.getByRole('button', { name: /清空/ }).click();
  await expect(page.getByLabel(/principal/)).toHaveValue('');
  await expect(page.getByRole('radio', { name: '全部' })).toBeChecked();
  await expect(page.getByText(/共 3 条 · 第 1\/1 页/)).toBeVisible();
});

test('pagination controls are present and the page-size select re-queries (server-driven paging)', async ({
  page,
}) => {
  await gotoAudit(page);

  // The forced-pagination footer is ALWAYS present (contract §二): page-size
  // select + prev/next buttons. At total=3 on one page, both nav buttons are
  // disabled — the page never client-slices a fabricated "next" window.
  const sizeSelect = page.getByLabel('每页条数');
  await expect(sizeSelect).toBeVisible();
  await expect(page.getByRole('button', { name: '上一页' })).toBeDisabled();
  await expect(page.getByRole('button', { name: '下一页' })).toBeDisabled();

  // Default page size is 20 (PAGE_DEFAULT_SIZE).
  await expect(sizeSelect).toHaveValue('20');

  // Changing page size triggers a server re-query; rows re-render from
  // fixtures and the footer reflects the new size (still 1 page at total=3).
  await sizeSelect.selectOption('50');
  await expect(sizeSelect).toHaveValue('50');
  await expect(page.getByRole('listitem')).toHaveCount(2);
  await expect(page.getByText(/共 3 条 · 第 1\/1 页/)).toBeVisible();
});

test('expanding a paired allow row reveals outcome-only fields (response_digest + duration_ms)', async ({
  page,
}) => {
  await gotoAudit(page);
  const allowRow = page.getByRole('listitem').nth(0);

  // Outcome-only fields are hidden until expanded (A-3 two-phase pairing).
  await expect(page.getByText('response_digest')).toHaveCount(0);
  await expandRow(allowRow, 'allow');

  // After expand: outcome phase fields are visible (intent has no response_digest).
  await expect(allowRow.getByText('response_digest')).toBeVisible();
  await expect(allowRow.getByText('sha256:88be…41')).toBeVisible();
  await expect(allowRow.getByText('duration_ms')).toBeVisible();
  await expect(allowRow.getByText('23', { exact: true })).toBeVisible();
});

test('clicking the deny DecisionBadge region expands the row and reveals stage + verbatim reason (codename only)', async ({
  page,
}) => {
  await gotoAudit(page);
  const denyRow = page.getByRole('listitem').nth(1);

  // Collapsed: no expanded grid panel yet.
  await expect(denyRow.locator('.grid')).toHaveCount(0);

  // Click the DECISION BADGE itself (the row's horizontal center). With the
  // bug this hit a nested badge <button> and the row did NOT toggle; the fix
  // makes the badge inert so the click reaches the row toggle and expands it.
  await denyRow.getByText('deny', { exact: true }).click();
  await expect(denyRow.locator('.grid')).toHaveCount(1);

  // The deny 落点 stage is now surfaced in the ROW's expanded panel (it is no
  // longer carried by an expandable badge): a `stage` KV + the StageChip value.
  await expect(denyRow.getByText('stage', { exact: true })).toBeVisible();
  await expect(denyRow.getByText('rbac', { exact: true })).toBeVisible();

  // The verbatim policy reason is shown in the expanded panel (a <span> with the
  // FULL text — the collapsed toggle only carries a truncated copy).
  await expect(
    denyRow.locator('span').filter({ hasText: new RegExp(`^${escapeRe(DENY_REASON)}$`) }),
  ).toBeVisible();
  // Resource is the anonymized codename — never a host/address/connection string.
  await expect(denyRow.getByText('db-main').first()).toBeVisible();
});

// ── DOM-level contract invariants (browser-visible) ──

test('snowflake id renders as a truncated STRING with the FULL value preserved (no precision loss)', async ({
  page,
}) => {
  await gotoAudit(page);
  const allowRow = page.getByRole('listitem').nth(0);
  await expandRow(allowRow, 'allow');

  // The full id would lose precision if parsed as a Number — proves it must
  // stay a string end-to-end (设计系统 §3.4).
  expect(String(Number(ID_OUTCOME))).not.toBe(ID_OUTCOME);

  // The full id is preserved in the title attribute (hover), while the visible
  // text is middle-truncated (not the full value).
  const idEl = allowRow.getByTitle(ID_OUTCOME, { exact: true });
  await expect(idEl).toBeVisible();
  await expect(idEl).toHaveText('7300…4002');

  // A copy control exists for the full id.
  await expect(allowRow.getByRole('button', { name: '复制完整 id' })).toBeVisible();
});

test('no real address / connection string / secret_hash leaks anywhere on the audit page', async ({
  page,
}) => {
  await gotoAudit(page);
  // Expand BOTH rows so every envelope field is in the DOM.
  await expandRow(page.getByRole('listitem').nth(0), 'allow');
  await expandRow(page.getByRole('listitem').nth(1), 'deny');
  await expect(page.getByText('response_digest')).toBeVisible(); // expanded state ready

  // Structural redaction (§七 / E4): the audit body must never surface a real
  // address/connection string (postgres://, ://, raw IP) nor a secret_hash /
  // plaintext key. Digests are sha256 prefixes only.
  const body = await page.locator('body').innerText();
  expect(body).not.toMatch(/postgres:\/\//);
  expect(body).not.toMatch(/:\/\//); // no URL/connection-string form
  expect(body).not.toMatch(/\b\d{1,3}(\.\d{1,3}){3}\b/); // no raw IPv4
  expect(body.toLowerCase()).not.toContain('secret_hash');

  // No write/elevate affordance on this read-only observe surface (§4.4). The
  // page exposes only export (导出 JSONL), apply/clear filters, paging, the row
  // toggle, the DecisionBadge, and copy — never a 放行/扩权/elevate control.
  await expect(page.getByRole('button', { name: /放行|扩权|elevate|授权放行/i })).toHaveCount(0);
});

test('deny does not leak out-of-scope existence (only stage/reason/codename, no allow affordance)', async ({
  page,
}) => {
  await gotoAudit(page);
  const denyRow = page.getByRole('listitem').nth(1);
  await expandRow(denyRow, 'deny');

  // The deny carries the fixture snowflake id as a string, fully preserved.
  await expect(denyRow.getByTitle(ID_DENY, { exact: true })).toBeVisible();

  // Within the deny row there is NO actionable allow/grant/elevate affordance —
  // audit is observe-only. (The buttons present are the toggle, the inert
  // DecisionBadge, and id-copy — none of which let an operator override a deny.)
  await expect(denyRow.getByRole('button', { name: /放行|扩权|elevate|授权放行/i })).toHaveCount(0);
  // And the row exposes no address/connection-string form.
  await expect(denyRow.getByText(/:\/\//)).toHaveCount(0);
});

/** Escape a literal string for safe use inside a RegExp. */
function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
