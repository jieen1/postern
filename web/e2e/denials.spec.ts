import { test, expect, type Page } from '@playwright/test';

/**
 * E2E · 拒绝分析 Denials — real Chromium + browser MSW (static handlers serving
 * src/mocks/fixtures.ts). Focus: happy-path browser integration, the page's main
 * read-only flows (aggregation ranking, alert band locate, row expand, jump to
 * Grants) and the DOM-level contract invariants visible in a real browser. We do
 * NOT re-test error/empty/threshold-edge states here — those are covered by the
 * vitest suite via per-test server.use() (impossible with the browser's static
 * worker).
 *
 * Default-fixture facts the browser will render (src/mocks/fixtures.ts §denialsSummary):
 *   row A  agent-order-bot × db-main     × rbac     × mutate  = 42  (principal_id 7300000000000000123)
 *   row B  agent-report-bot × api-billing × classify × execute = 11 (principal_id 7300000000000000999)
 * Alert threshold is 30 (lib.ts), so ONLY row A is over threshold → AlertBand (1).
 */

// The over-threshold group's stable four-tuple key (lib.ts groupKey =
// `${principal_id}|${resource}|${stage}|${capability}`).
const ROW_A_KEY = '7300000000000000123|db-main|rbac|mutate';
const ROW_B_KEY = '7300000000000000999|api-billing|classify|execute';
const BIG_ID = '7300000000000000123'; // > 2^53; Number() would corrupt it.

/** Go to /denials and wait until the ranking has rendered real fixture rows. */
async function openDenials(page: Page) {
  await page.goto('/denials');
  await expect(
    page.getByRole('heading', { name: '拒绝分析 Denials' }),
  ).toBeVisible();
  // Wait for real data (not skeleton): both fixture groups present as rows.
  await expect(page.locator(`[data-group-key="${ROW_A_KEY}"]`)).toBeVisible();
  await expect(page.locator(`[data-group-key="${ROW_B_KEY}"]`)).toBeVisible();
}

test.describe('Denials · 真实浏览器集成', () => {
  test('聚合榜从 fixtures 渲染真实数据并按 count 倒序（42 在 11 之上）', async ({
    page,
  }) => {
    await openDenials(page);

    // Real principal names from fixtures appear in the ranking (not
    // skeleton/empty). `agent-order-bot` also appears in the alert band, so we
    // scope to its ranking row to keep the locator unambiguous.
    await expect(
      page.locator(`[data-group-key="${ROW_A_KEY}"]`).getByText('agent-order-bot'),
    ).toBeVisible();
    await expect(
      page.locator(`[data-group-key="${ROW_B_KEY}"]`).getByText('agent-report-bot'),
    ).toBeVisible();

    // Summary bar echoes the default window label + group count = total (2).
    const summary = page.getByLabel('窗口汇总');
    await expect(summary).toContainText('近 7 天');
    await expect(summary.getByTestId('group-count')).toHaveText('2');

    // Reverse count order: collect the count cells from the two data rows in
    // DOM order; must be 42 then 11.
    const rowACount = page
      .locator(`[data-group-key="${ROW_A_KEY}"]`)
      .getByText('42', { exact: true });
    const rowBCount = page
      .locator(`[data-group-key="${ROW_B_KEY}"]`)
      .getByText('11', { exact: true });
    await expect(rowACount).toBeVisible();
    await expect(rowBCount).toBeVisible();

    // Row A (42) is physically above row B (11) in the rendered table.
    const aBox = await page.locator(`[data-group-key="${ROW_A_KEY}"]`).boundingBox();
    const bBox = await page.locator(`[data-group-key="${ROW_B_KEY}"]`).boundingBox();
    expect(aBox).not.toBeNull();
    expect(bBox).not.toBeNull();
    expect(aBox!.y).toBeLessThan(bBox!.y);
  });

  test('告警带带[定位]：超阈值组(42)在告警带高亮，点定位滚动到对应行并打高亮', async ({
    page,
  }) => {
    await openDenials(page);

    // Alert band renders, scoped to the single over-threshold group.
    const band = page.getByRole('region', { name: '告警 Alerts' });
    await expect(band).toBeVisible();
    await expect(band).toContainText('告警 Alerts (1)');
    await expect(band).toContainText('42 次');
    await expect(band).toContainText('≥阈值 30');
    // The non-alerting group (count 11) must NOT appear in the alert band.
    await expect(band).not.toContainText('agent-report-bot');

    // Click [定位] → scrolls to and highlights the matching ranking row.
    await band.getByRole('button', { name: /定位/ }).click();
    const targetRow = page.locator(`[data-group-key="${ROW_A_KEY}"]`);
    await expect(targetRow).toBeVisible();
    // Amber highlight class applied after locate.
    await expect(targetRow).toHaveClass(/bg-warn/);
  });

  test('展开行细节：显落点 stage / intent_digest 全展 / policy_rev / principal_id 雪花原样不丢精度', async ({
    page,
  }) => {
    await openDenials(page);

    // Expand row A via its expand toggle (scoped to row A to avoid the 2nd row).
    await page
      .locator(`[data-group-key="${ROW_A_KEY}"]`)
      .getByRole('button', { name: '展开细节' })
      .click();

    const panel = page.getByRole('region', { name: '聚合组细节' });
    await expect(panel).toBeVisible();

    // Deny landing stage shown (rbac), full intent_digest, policy_rev value.
    await expect(panel).toContainText('在 rbac 阶段被拒');
    await expect(panel.getByText('sha256:77cd…02', { exact: true })).toBeVisible();
    await expect(panel).toContainText('policy_rev');
    await expect(panel.getByText('4187', { exact: true })).toBeVisible();

    // principal_id rendered as a STRING snowflake: full value lives in title,
    // truncated text shown — never a coerced number.
    const idEl = panel.getByTitle(BIG_ID);
    await expect(idEl).toBeVisible();
    await expect(idEl).toHaveText('7300…0123');
    // Prove the trap is real in-browser: Number() would have corrupted the tail.
    expect(String(Number(BIG_ID))).not.toBe(BIG_ID);

    // The mechanical elevate template (copy item, NOT an allow button) is shown
    // with a placeholder TTL.
    await expect(
      panel.getByText('postern elevate agent-order-bot --cap db-main:mutate --ttl <填>'),
    ).toBeVisible();
  });

  test('行操作跳 Grants：菜单项路由到 /grants 并带 principal+resource 参', async ({
    page,
  }) => {
    await openDenials(page);

    // Open row A's action menu and choose "跳 Grants 这一格".
    await page
      .locator(`[data-group-key="${ROW_A_KEY}"]`)
      .getByRole('button', { name: '行操作' })
      .click();
    const menu = page.getByRole('menu');
    await expect(menu).toBeVisible();
    await menu.getByRole('menuitem', { name: '跳 Grants 这一格' }).click();

    // Real browser URL change → /grants with the preselected格 params.
    await expect(page).toHaveURL(/\/grants\?/);
    const url = new URL(page.url());
    expect(url.pathname).toBe('/grants');
    expect(url.searchParams.get('principal')).toBe('agent-order-bot');
    expect(url.searchParams.get('resource')).toBe('db-main');
    // The destination page actually rendered (navigation completed, not a stub).
    await expect(
      page.getByRole('heading', { name: '授权矩阵 Grants' }),
    ).toBeVisible();
  });
});

test.describe('Denials · DOM 级契约不变量', () => {
  test('页面无放行/allow 按钮（E7：聚合榜是信号，绝不一键放行）', async ({
    page,
  }) => {
    await openDenials(page);

    // Expand a row to surface the detail panel + open the row action menu so
    // every interactive control is in the DOM when we assert their absence.
    await page
      .locator(`[data-group-key="${ROW_A_KEY}"]`)
      .getByRole('button', { name: '展开细节' })
      .click();
    await expect(page.getByRole('region', { name: '聚合组细节' })).toBeVisible();

    await page
      .locator(`[data-group-key="${ROW_A_KEY}"]`)
      .getByRole('button', { name: '行操作' })
      .click();
    await expect(page.getByRole('menu')).toBeVisible();

    // No allow/grant/放行 control anywhere — only mechanical templates + jumps.
    await expect(page.getByRole('button', { name: /放行/ })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: /放行/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /allow/i })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: /allow/i })).toHaveCount(0);
  });

  test('无真实地址/连接串/IP/secret_hash 泄露；资源只以代号呈现', async ({
    page,
  }) => {
    await openDenials(page);
    // Expand the row so the detail panel (intent_digest, policy_rev, jumps) is in
    // the DOM and included in the leakage scan.
    await page
      .locator(`[data-group-key="${ROW_A_KEY}"]`)
      .getByRole('button', { name: '展开细节' })
      .click();
    await expect(page.getByRole('region', { name: '聚合组细节' })).toBeVisible();

    // Resource shown by codename only.
    await expect(page.getByText('db-main', { exact: true }).first()).toBeVisible();
    await expect(page.getByText('api-billing', { exact: true }).first()).toBeVisible();

    const body = (await page.locator('body').textContent()) ?? '';
    expect(body).not.toMatch(/postgres:\/\//); // no connection string
    expect(body).not.toMatch(/\d+\.\d+\.\d+\.\d+/); // no dotted-quad IP
    expect(body).not.toContain('secret_hash');
    expect(body).not.toMatch(/secret_hash/i);
    // The snowflake full id is present only in a title attr (truncated in text),
    // so the literal 19-digit id must NOT leak into the visible body text.
    expect(body).not.toContain(BIG_ID);
  });
});
