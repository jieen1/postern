import { test, expect, type Page } from '@playwright/test';

/**
 * 授权矩阵 Grants — real-browser e2e (web/docs/05-grants.md).
 *
 * Runs against the shared Vite+MSW dev server on :5173; the browser MSW worker
 * serves the STATIC fixtures from src/mocks/handlers.ts + fixtures.ts, so these
 * tests assert the happy path, cross-page flow, and DOM-level contract
 * invariants that the default fixtures render reliably. Error/empty/pagination
 * edges are covered by the vitest suite (server.use per-test) and are NOT
 * re-created here.
 *
 * Fixture facts this spec relies on (src/mocks/fixtures.ts):
 *   - principals: agent-order-bot (default), alice
 *   - health.policy_rev: '4187'
 *   - grantsView.your_grants: { 'db-main': ['observe','query'], 'api-billing': ['observe'] }
 *   - grantsView.temp_grants[0]: id 7300000000000003001, db-main × mutate,
 *       granted 2026-06-14T01:00:00Z, expires 2026-06-14T05:00:00Z (live now).
 */

const GRANT_ID = '7300000000000003001';

/** Open the app and land on the Grants page via the sidebar, matrix rendered. */
async function gotoGrants(page: Page) {
  await page.goto('/');
  await page.getByRole('link', { name: /授权矩阵 Grants/ }).click();
  await expect(page.getByRole('heading', { name: /授权矩阵 Grants/ })).toBeVisible();
  // Wait for REAL data (not skeleton): the matrix table from fixtures.
  await expect(page.getByRole('table', { name: '生效授权矩阵' })).toBeVisible();
}

test('matrix renders real fixture data: principal default, policy_rev, rows × 6 capability columns, three-state cells', async ({
  page,
}) => {
  await gotoGrants(page);

  const matrix = page.getByRole('table', { name: '生效授权矩阵' });

  // Principal selector defaulted to the first principal; policy_rev reconciled.
  await expect(page.getByLabel('选择 Principal')).toHaveValue('agent-order-bot');
  await expect(page.getByText('policy_rev:')).toBeVisible();
  await expect(page.getByText('4187', { exact: true })).toBeVisible();

  // Resource rows come straight from your_grants + live temp_grants.
  await expect(matrix.getByText('db-main', { exact: true })).toBeVisible();
  await expect(matrix.getByText('api-billing', { exact: true })).toBeVisible();

  // The six fixed capability column headers.
  for (const cap of ['observe', 'query', 'mutate', 'execute', 'manage', 'destroy']) {
    await expect(matrix.getByText(cap, { exact: true }).first()).toBeVisible();
  }

  // Three-state DECISION cells, addressed by aria-label (text+icon, not color):
  // persistent (from your_grants), temp (from a live temp_grant), default-deny.
  await expect(page.getByLabel('db-main × observe：持久')).toBeVisible();
  await expect(page.getByLabel('db-main × mutate：临时')).toBeVisible();
  await expect(page.getByLabel('db-main × destroy：默认拒绝')).toBeVisible();
  await expect(page.getByLabel('api-billing × observe：持久')).toBeVisible();
});

test('temp-cell drill-down: opens provenance drawer with full snowflake id + revoke entry', async ({
  page,
}) => {
  await gotoGrants(page);

  await page.getByLabel('db-main × mutate：临时').click();
  const drawer = page.getByRole('dialog', { name: '格详情' });
  await expect(drawer).toBeVisible();

  // Provenance is the live temp_grant; an inline revoke entry is offered.
  await expect(drawer.getByText(/临时授权 \(allow\)/)).toBeVisible();
  await expect(drawer.getByRole('button', { name: /立即吊销 revoke/ })).toBeVisible();

  // Snowflake id rendered as a STRING with its full value in the title (no
  // Number round-trip / precision loss). 2^53 < this id, so a numeric coercion
  // would corrupt it.
  await expect(drawer.getByTitle(GRANT_ID)).toBeVisible();
  expect(Number(GRANT_ID).toString()).not.toBe(GRANT_ID);

  // The temp drawer shows a decision, never any role NAME/tier (wire has none).
  await expect(drawer.getByText(/角色/)).toHaveCount(0);
});

test('persistent-cell drill-down: read-only drawer links to Bindings (no edit/allow here)', async ({
  page,
}) => {
  await gotoGrants(page);

  await page.getByLabel('db-main × observe：持久').click();
  const drawer = page.getByRole('dialog', { name: '格详情' });
  await expect(drawer).toBeVisible();

  await expect(drawer.getByText(/持久授权 \(allow\)/)).toBeVisible();
  // Cross-page link to revise the persistent grant on the Bindings page.
  await expect(drawer.getByRole('link', { name: /去 Bindings 页修订/ })).toHaveAttribute(
    'href',
    '/bindings',
  );
});

test('Elevate write flow: form → summary preview → danger confirm (type resource code) → optimistic success closes drawer', async ({
  page,
}) => {
  await gotoGrants(page);

  await page.getByRole('button', { name: /Elevate 提权/ }).click();
  const elevate = page.getByRole('dialog', { name: '临时提权 Elevate' });
  await expect(elevate).toBeVisible();
  // Principal is locked to the selected one (no free choice = no scope leak).
  await expect(elevate.getByText('agent-order-bot（锁定）')).toBeVisible();

  await elevate.getByLabel('Resource *').selectOption('db-main');
  await elevate.getByLabel('Capability *').selectOption('destroy');

  // Summary preview reflects the chosen fields and flags the 扩权 nature.
  await expect(elevate.getByText(/将给/)).toBeVisible();
  await expect(elevate.getByText('扩大')).toBeVisible();

  await elevate.getByRole('button', { name: /提权…（危险确认）/ }).click();

  // Danger confirm: the confirm button stays disabled until the resource code
  // is typed verbatim (two-key扩权 gate).
  const confirm = page.getByRole('dialog', { name: '确认临时提权（扩权）' });
  await expect(confirm).toBeVisible();
  const confirmBtn = confirm.getByRole('button', { name: '确认提权' });
  await expect(confirmBtn).toBeDisabled();
  await confirm.getByRole('textbox').fill('db-main');
  await expect(confirmBtn).toBeEnabled();
  await confirmBtn.click();

  // Optimistic success: the elevate handler returns a fresh policy_rev, the
  // confirm + drawer both close (no error surfaced).
  await expect(confirm).toBeHidden();
  await expect(elevate).toBeHidden();
});

test('Revoke write flow: row 吊销 → confirm shows revoked reason + id → confirm closes dialog', async ({
  page,
}) => {
  await gotoGrants(page);

  // The live temp_grant appears in the temp_grants table with a revoke action.
  await expect(page.getByTitle(GRANT_ID).first()).toBeVisible();
  await page.getByRole('button', { name: '吊销' }).first().click();

  const confirm = page.getByRole('dialog', { name: '确认吊销临时授权（收权）' });
  await expect(confirm).toBeVisible();
  // Danger copy names the machine end_reason and the exact id being closed.
  await expect(confirm.getByText(/revoked/)).toBeVisible();
  await expect(confirm.getByText(GRANT_ID)).toBeVisible();

  await confirm.getByRole('button', { name: '确认吊销' }).click();
  // Success (handler returns a fresh policy_rev): the confirm dialog closes.
  await expect(confirm).toBeHidden();
});

test('matrix resource filter narrows rows to the typed code', async ({ page }) => {
  await gotoGrants(page);
  const matrix = page.getByRole('table', { name: '生效授权矩阵' });

  await expect(matrix.getByText('api-billing', { exact: true })).toBeVisible();
  await page.getByLabel('资源代号筛选').fill('db');

  // db-main stays; api-billing drops out of the matrix.
  await expect(matrix.getByText('db-main', { exact: true })).toBeVisible();
  await expect(matrix.getByText('api-billing', { exact: true })).toHaveCount(0);
});

test('cross-page deep link from Denials (/grants?principal=&resource=) ECHOES that cell: principal selected + resource filter prefilled', async ({
  page,
}) => {
  // This is the exact URL the Denials page builds (DenialDetailPanel JumpLink):
  //   /grants?principal=<p>&resource=<r>. Both values are REAL fixtures:
  //   principal 'alice' (the SECOND principal, not the default first) and the
  //   resource substring 'db-main'. Echoing must land on alice, not the default.
  await page.goto('/grants?principal=alice&resource=db-main');

  await expect(page.getByRole('heading', { name: /授权矩阵 Grants/ })).toBeVisible();
  await expect(page.getByRole('table', { name: '生效授权矩阵' })).toBeVisible();

  // The principal selector is set to the DEEP-LINKED principal (alice), proving
  // the page read ?principal — not silently defaulting to the first principal.
  await expect(page.getByLabel('选择 Principal')).toHaveValue('alice');

  // The resource filter input is PREFILLED from ?resource …
  await expect(page.getByLabel('资源代号筛选')).toHaveValue('db-main');

  // … and the matrix converges to that cell: db-main stays, api-billing is
  // filtered out (the matrix echoes the linked resource, not the full grid).
  const matrix = page.getByRole('table', { name: '生效授权矩阵' });
  await expect(matrix.getByText('db-main', { exact: true })).toBeVisible();
  await expect(matrix.getByText('api-billing', { exact: true })).toHaveCount(0);
});

test('DOM contract: never leaks a real address/connection string, secret_hash, or plaintext secret', async ({
  page,
}) => {
  await gotoGrants(page);
  // Render every state the page can reach from fixtures before scanning DOM:
  // open both drawers and a danger confirm, then read the full document text.
  await page.getByLabel('db-main × mutate：临时').click();
  await expect(page.getByRole('dialog', { name: '格详情' })).toBeVisible();

  const body = (await page.locator('body').innerText()) + ' ' +
    ((await page.content()) ?? '');

  // No real addresses / connection strings / dotted-quad IPs — only codenames.
  expect(body).not.toMatch(/postgres:\/\//);
  expect(body).not.toMatch(/\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/);
  expect(body).not.toMatch(/jdbc:|redis:\/\/|amqp:\/\/|mongodb:\/\//);

  // No secret material is ever printed on this page.
  expect(body).not.toMatch(/secret_hash/);
  expect(body).not.toMatch(/BEGIN [A-Z ]*PRIVATE KEY/);
  expect(body).not.toMatch(/sk-[A-Za-z0-9]{16,}/);
});

test('DOM contract: deny cells are inert — no allow/放行/批准 button anywhere on the matrix', async ({
  page,
}) => {
  await gotoGrants(page);

  // A default-deny cell is rendered (text+icon) but is non-interactive: it has
  // no enabled affordance to flip the decision in place.
  const denyCell = page.getByRole('button', { name: 'db-main × destroy：默认拒绝' });
  await expect(denyCell).toBeDisabled();

  // The page offers no one-click allow/grant/approve control —扩权 only flows
  // through the gated Elevate form, never a direct放行 on a deny.
  await expect(page.getByRole('button', { name: /^放行$|^允许$|^批准$|^allow$/i })).toHaveCount(0);
});
