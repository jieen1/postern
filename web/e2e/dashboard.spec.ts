import { test, expect } from '@playwright/test';

/**
 * 总览 Dashboard (01-dashboard) — real-browser e2e against the shared Vite+MSW
 * dev server on :5173. MSW serves /v1/* from static fixtures (src/mocks), so we
 * exercise only the deterministic happy path + cross-page drill-downs + the
 * DOM-level contract invariants visible in a real browser. Error/empty/paging
 * edge states are covered by the vitest unit tests (per-test server.use), which
 * the browser MSW worker cannot replicate, so they are NOT re-created here.
 *
 * Fixtures this page renders (src/mocks/fixtures.ts):
 *   health      = { status:'up', audit_writable:true, audit_watermark:0.12,
 *                   policy_rev:'4187', uptime_ms:9_432_000 }   ⇒ UP/WRITABLE/12%
 *   modeState   = [ global normal, db-main maintain (expires 2026-06-14T06:00Z) ]
 *   denials     = [ agent-order-bot/db-main/mutate/rbac/42,
 *                   agent-report-bot/api-billing/execute/classify/11 ]
 */

test.describe('总览 Dashboard', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Wait for live data (not skeleton) before any assertion: HealthCard's "UP"
    // only appears after /v1/health resolves through the MSW worker.
    await expect(page.getByRole('heading', { name: '总览 Dashboard' })).toBeVisible();
    await expect(page.getByText('UP', { exact: true })).toBeVisible();
  });

  // ── Journey 1: real fixtures render across health + mode + denials ──────────
  test('boots and renders real fixture data in the health / mode / denials cards', async ({
    page,
  }) => {
    // All five card titles compose the observation panel. Scope to the content
    // <main> — the sidebar nav also links "红队自检 Verify" etc., so the page-wide
    // text would collide; the card titles are headings inside the panel.
    const panel = page.locator('main');
    await expect(panel.getByText('系统健康 Health')).toBeVisible();
    await expect(panel.getByText('当前模式姿态 Mode')).toBeVisible();
    await expect(panel.getByRole('heading', { name: '最近高频拒绝 Denials' })).toBeVisible();
    await expect(panel.getByText('临时授权将到期')).toBeVisible();
    await expect(panel.getByRole('heading', { name: '红队自检 Verify' })).toBeVisible();

    // HealthCard — real values from the health fixture (NOT skeleton/empty).
    await expect(page.getByText('UP', { exact: true })).toBeVisible();
    await expect(page.getByText('WRITABLE', { exact: true })).toBeVisible();
    // policy_rev 4187 is rendered as a STRING (SnowflakeId) in the card, never
    // parsed. Scope to <main>: the top-bar HealthLight title also mentions "rev
    // 4187", so a page-wide match would collide.
    await expect(panel.getByText('4187', { exact: true })).toBeVisible();
    // audit_watermark 0.12 ⇒ 12%, a healthy tone (no "逼近上限" warning).
    await expect(page.getByText('12%')).toBeVisible();
    await expect(page.getByText(/逼近上限/)).toHaveCount(0);
    // Capacity meter exposes the watermark as an accessible value.
    await expect(page.getByRole('meter', { name: 'audit store 容量水位' })).toHaveAttribute(
      'aria-valuenow',
      '12',
    );

    // ModePanel — global normal + the single db-main maintain override. Scope
    // to <main>: the top-bar GlobalEmergencyBar also renders the global "normal"
    // ModeBadge, so a page-wide match would collide.
    await expect(panel.getByText('normal', { exact: true })).toBeVisible();
    await expect(panel.getByText('资源覆盖 (1)')).toBeVisible();
    await expect(panel.getByText('db-main').first()).toBeVisible();
    await expect(panel.getByText('maintain', { exact: true })).toBeVisible();

    // DenialsTopTable — the cross-principal aggregation rows, real counts.
    await expect(page.getByText('agent-order-bot')).toBeVisible();
    await expect(page.getByText('mutate', { exact: true })).toBeVisible();
    await expect(page.getByText('rbac', { exact: true })).toBeVisible();
    await expect(page.getByText('42', { exact: true })).toBeVisible();
    // Second fixture row (different principal/resource/stage) also present.
    await expect(page.getByText('agent-report-bot')).toBeVisible();
    await expect(page.getByText('11', { exact: true })).toBeVisible();

    // VerifyCard — the client cache is empty on a fresh load, so it shows the
    // truthful "尚未运行红队自检" guidance, NOT a fabricated PASS.
    await expect(page.getByText('尚未运行红队自检')).toBeVisible();
  });

  // ── Journey 2: deny-board main interaction (window switch) ──────────────────
  test('switches the deny window and keeps rendering the aggregation board', async ({ page }) => {
    await expect(page.getByText('agent-order-bot')).toBeVisible();

    // The window selector is the deny board's primary in-page interaction.
    const windowSelect = page.getByLabel('拒绝窗口');
    await expect(windowSelect).toHaveValue('7d');
    await windowSelect.selectOption('24h');
    await expect(windowSelect).toHaveValue('24h');

    // After re-fetching the new window the board still renders real rows (the
    // static MSW fixture is window-agnostic, so the same Top-N surfaces).
    await expect(page.getByText('agent-order-bot')).toBeVisible();
    await expect(page.getByText('42', { exact: true })).toBeVisible();
  });

  // ── Journey 3a: deny row drills into Audit, prefiltered to this principal ────
  test('a deny row → audit drills into the Audit page carrying the deny prefilter', async ({
    page,
  }) => {
    await expect(page.getByText('agent-order-bot')).toBeVisible();

    // The deny board's per-row action jumps to the Audit deny stream.
    const row = page.locator('tr', { hasText: 'agent-order-bot' });
    await row.getByRole('button', { name: /audit/i }).click();

    // Lands on the Audit page with the prefilter facets in the URL (principal +
    // decision=deny). No reason detail / grant cell is carried into the URL —
    // deny detail is read field-by-field on the Audit page, not leaked here.
    await expect(page).toHaveURL(/\/audit\?/);
    await expect(page).toHaveURL(/decision=deny/);
    await expect(page).toHaveURL(/principal=agent-order-bot/);
    await expect(page).not.toHaveURL(/grant/);
    await expect(page.getByRole('heading', { name: '审计 Audit' })).toBeVisible();
  });

  // ── Journey 3b: right-rail jump cards reach Grants and Verify ───────────────
  test('the ExpiringGrants and VerifyCard jump links reach Grants and Verify', async ({ page }) => {
    // ExpiringGrants is a jump-only card (per-principal GET /v1/grants cannot
    // enumerate cross-principal near-expiry), so it inlines NO rows — only guidance.
    await page.getByRole('link', { name: '前往 Grants →' }).click();
    await expect(page).toHaveURL(/\/grants$/);
    await expect(page.getByRole('heading', { name: '授权矩阵 Grants' })).toBeVisible();

    await page.goBack();
    await expect(page.getByRole('heading', { name: '总览 Dashboard' })).toBeVisible();

    // VerifyCard never triggers verify; it only guides to the Verify page.
    await page.getByRole('link', { name: '前往 Verify →' }).click();
    await expect(page).toHaveURL(/\/verify$/);
    await expect(page.getByRole('heading', { name: '红队自检 Verify' })).toBeVisible();
  });

  // ── Journey 3c: ModePanel manage-mode link reaches the Mode page ────────────
  test('the ModePanel manage link reaches the Mode page (mode change is not inlined here)', async ({
    page,
  }) => {
    await page.getByRole('link', { name: '管理模式 → Mode' }).click();
    await expect(page).toHaveURL(/\/mode$/);
    await expect(page.getByRole('heading', { name: '模式 Mode' })).toBeVisible();
  });

  // ── DOM-level contract invariants (real, browser-visible) ───────────────────
  test('upholds the DOM contract: no real address / secret, no second freeze, snowflake intact', async ({
    page,
  }) => {
    const body = page.locator('body');

    // No connection string / real address surfaces anywhere on the panel
    // (resources show only the codename db-main; adapter "postgres" + the
    // vault:// secret_ref from the resource fixture are never rendered here).
    await expect(body).not.toContainText('postgres://');
    await expect(body).not.toContainText('vault://');
    // No raw IPv4 address leaks (e.g. host:port connection targets).
    const ipv4 = await body.evaluate((el) =>
      /\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/.test(el.textContent ?? ''),
    );
    expect(ipv4).toBe(false);

    // No secret_hash / plaintext credential leaks on the observation panel.
    await expect(body).not.toContainText('secret_hash');

    // Deny board exposes ONLY the Scope-in aggregation facets — no reason text
    // and no operator_note (those live field-by-field on the Denials/Audit page).
    await expect(body).not.toContainText('no grant cell');
    await expect(body).not.toContainText('DBA 值班');

    // The Dashboard's only write (global freeze) lives in the top-bar
    // GlobalEmergencyBar — the panel body must NOT carry a second freeze/解冻
    // control (no double-entry that could drift the freeze semantics).
    await expect(page.locator('main').getByRole('button', { name: /freeze/i })).toHaveCount(0);
    await expect(page.locator('main').getByRole('button', { name: /冻结|解冻/ })).toHaveCount(0);

    // Snowflake id discipline: policy_rev is the full string 4187, exposed in
    // the SnowflakeId title attribute (one-click copy carries the exact value,
    // never a Number-coerced approximation).
    await expect(page.locator('main [title="4187"]')).toBeVisible();
  });
});
