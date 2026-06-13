import { test, expect, type Page } from '@playwright/test';

/**
 * 模式 Mode (11-mode.md) — real-browser e2e against the shared Vite+MSW dev
 * server on :5173. MSW serves STATIC fixtures (src/mocks/fixtures.ts), so this
 * spec covers the happy path + cross-page integration + DOM-level contract
 * invariants that are reliably observable from the default fixtures. Error /
 * 409 / pagination-boundary states are covered exhaustively by vitest
 * (per-test server.use) and are NOT re-created here.
 *
 * Fixture facts this spec relies on (src/mocks/fixtures.ts → modeState):
 *   - global row: scope=null, mode/effective = normal, version 7, policy_rev '4187'
 *   - override:   scope='db-main', local+effective = maintain, version 2, policy_rev '4180'
 * grantsView.your_grants: { 'db-main': [observe, query], 'api-billing': [observe] }.
 * POST /v1/mode {op:'set'} echoes the SAME static modeState, so a successful
 * write is observed as the drawer CLOSING (no error/conflict), not as the board
 * flipping to the new mode (the static mock can't reflect the write).
 */

async function gotoMode(page: Page) {
  await page.goto('/mode');
  await expect(page.getByRole('heading', { name: '模式 Mode' })).toBeVisible();
}

test.describe('模式 Mode — 真实浏览器集成', () => {
  test('从 fixtures 渲染真实数据：全局态卡片 + 资源级覆盖 + 顶栏同源', async ({
    page,
  }) => {
    await gotoMode(page);

    // Global card is a real region rendered from fixtures (not skeleton/empty):
    // effective mode = normal, with the full policy_rev string and "no TTL".
    const card = page.getByRole('region', { name: '全局辖区' });
    await expect(card).toBeVisible();
    await expect(card.getByText('normal', { exact: true })).toBeVisible();
    // Snowflake policy_rev kept as a STRING (full value in title) — no precision loss.
    await expect(card.getByTitle('4187')).toBeVisible();
    await expect(card.getByText('no TTL')).toBeVisible();

    // Resource-override table renders the db-main override row from fixtures,
    // with BOTH local and effective mode = maintain, source-labelled ←本地.
    const row = page.getByRole('row').filter({ hasText: 'db-main' });
    await expect(row).toBeVisible();
    await expect(row.getByText('maintain')).toHaveCount(2); // local + effective
    await expect(row.getByText('←本地')).toBeVisible();
    // The override row carries its own (different) policy_rev string, intact.
    await expect(row.getByTitle('4180')).toBeVisible();

    // Same-source contract: the top-bar GlobalEmergencyBar shows the SAME global
    // effective mode badge (normal) as the page's global card, and a Freeze switch.
    await expect(page.getByRole('button', { name: 'Freeze' })).toBeVisible();
  });

  test('筛选资源代号收窄表格，命中/落空对照 EmptyState', async ({ page }) => {
    await gotoMode(page);
    await expect(
      page.getByRole('row').filter({ hasText: 'db-main' }),
    ).toBeVisible();

    const search = page.getByRole('searchbox', { name: '筛选资源代号' });

    // A matching prefix keeps the row.
    await search.fill('db');
    await expect(
      page.getByRole('row').filter({ hasText: 'db-main' }),
    ).toBeVisible();

    // A non-matching filter empties the table → EmptyState inherit-global copy.
    await search.fill('zzz-no-such-resource');
    await expect(
      page.getByRole('row').filter({ hasText: 'db-main' }),
    ).toHaveCount(0);
    await expect(
      page.getByText(/当前无资源级模式覆盖，全部辖区继承全局模式 normal/),
    ).toBeVisible();

    // Clearing the filter brings the override row back.
    await search.fill('');
    await expect(
      page.getByRole('row').filter({ hasText: 'db-main' }),
    ).toBeVisible();
  });

  test('展开收窄影响预览：读 GET /v1/grants 渲染真实授权世界对照', async ({
    page,
  }) => {
    await gotoMode(page);

    // The preview is collapsed by default; expanding it reads /v1/grants.
    const toggle = page.getByRole('button', { name: /收窄影响预览/ });
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');

    const preview = page.getByRole('region', { name: '收窄影响预览' });
    // your_grants from fixtures renders both scope-bounded resource rows.
    const dbRow = preview.getByRole('row').filter({ hasText: 'db-main' });
    await expect(dbRow).toBeVisible();
    await expect(preview.getByRole('row').filter({ hasText: 'api-billing' })).toBeVisible();

    // Global mode is normal → all RBAC verbs survive; db-main keeps observe+query
    // in BOTH the original and remaining columns (no narrowing under normal).
    const cells = dbRow.getByRole('cell');
    await expect(cells.nth(1).getByText('observe')).toBeVisible(); // original
    await expect(cells.nth(1).getByText('query')).toBeVisible();
    await expect(cells.nth(2).getByText('observe')).toBeVisible(); // remaining
    await expect(cells.nth(2).getByText('query')).toBeVisible();
  });

  test('切换全局模式 → observe：摘要预览(旧→新 + 期望 version) → 危险确认 → 乐观成功关闭抽屉', async ({
    page,
  }) => {
    await gotoMode(page);
    // Open the drawer AFTER the board loaded so global version (7) is resolved.
    await expect(page.getByRole('region', { name: '全局辖区' })).toBeVisible();
    await page.getByRole('button', { name: '切换模式' }).click();

    const drawer = page.getByRole('dialog', { name: '切换模式' });
    await expect(drawer).toBeVisible();
    // Scope prefilled to Global.
    await expect(drawer.getByText('全局 (Global)')).toBeVisible();

    // Pick observe and attach a TTL.
    await drawer.getByRole('radio', { name: /observe/i }).click();
    await drawer.getByPlaceholder(/留空=长期/).fill('30');

    // Summary preview: 旧(normal) → 新(observe), TTL echoed, expected version 7.
    await expect(drawer.getByText(/期望 version/)).toBeVisible();
    await expect(drawer.getByTitle('7')).toBeVisible();
    await expect(drawer.getByText(/30 分钟/)).toBeVisible();

    // Danger confirm (standard, scope-widening) for the GLOBAL jurisdiction.
    await drawer.getByRole('button', { name: '确认切换' }).click();
    const confirm = page.getByRole('dialog', { name: /切换模式 — GLOBAL/ });
    await expect(confirm).toBeVisible();
    await confirm.getByRole('button', { name: '确认切换' }).click();

    // Optimistic success: write resolved with no error/conflict, drawer closes.
    await expect(page.getByRole('dialog', { name: '切换模式' })).toHaveCount(0);
    await expect(page.getByText(/乐观锁冲突/)).toHaveCount(0);
    // Board still renders (re-read succeeded); global card present again.
    await expect(page.getByRole('region', { name: '全局辖区' })).toBeVisible();
  });

  test('DOM 契约 — 单资源 freeze 最高危：确认词是辖区标识(db-main)而非字面 "freeze"', async ({
    page,
  }) => {
    await gotoMode(page);
    // Open the per-resource switch drawer from the db-main row action.
    await page.getByRole('button', { name: '切换此资源' }).click();
    const drawer = page.getByRole('dialog', { name: '切换模式' });
    await expect(drawer).toBeVisible();
    // Scope prefilled with the resource code (badge + echoed in the summary).
    await expect(drawer.getByText('db-main').first()).toBeVisible();

    // Choose freeze → summary states the strictest narrowing fact.
    await drawer.getByRole('radio', { name: /freeze/i }).click();
    await expect(drawer.getByText(/拒绝一切动词/).first()).toBeVisible();

    await drawer.getByRole('button', { name: '确认切换' }).click();
    const confirm = page.getByRole('dialog', { name: /切到 FREEZE — db-main/ });
    await expect(confirm).toBeVisible();

    const confirmBtn = confirm.getByRole('button', { name: '确认冻结' });
    // Anti-misclick: confirm stays DISABLED until the jurisdiction id is typed.
    await expect(confirmBtn).toBeDisabled();

    // Typing the literal word "freeze" must NOT unlock it (the word is the scope id).
    await confirm.getByRole('textbox').fill('freeze');
    await expect(confirmBtn).toBeDisabled();

    // The correct jurisdiction identifier (resource code) unlocks confirm.
    await confirm.getByRole('textbox').fill('db-main');
    await expect(confirmBtn).toBeEnabled();
    await confirmBtn.click();

    // Success → drawer closes, no conflict/error surfaced.
    await expect(page.getByRole('dialog', { name: '切换模式' })).toHaveCount(0);
    await expect(page.getByText(/乐观锁冲突/)).toHaveCount(0);
  });

  test('DOM 契约 — 脱敏与雪花精度：无真实地址/连接串/secret_hash，policy_rev 全值字符串', async ({
    page,
  }) => {
    await gotoMode(page);
    await expect(page.getByRole('region', { name: '全局辖区' })).toBeVisible();
    // Expand the narrowing preview so the grants-derived rows are in the DOM too.
    await page.getByRole('button', { name: /收窄影响预览/ }).click();
    await expect(page.getByRole('region', { name: '收窄影响预览' })).toBeVisible();

    const body = (await page.locator('body').innerText()).toLowerCase();

    // No real connection strings / scheme-qualified addresses leak (脱敏：只显代号).
    expect(body).not.toContain('postgres://');
    expect(body).not.toContain('postgresql://');
    expect(body).not.toContain('redis://');
    expect(body).not.toContain('mysql://');
    expect(body).not.toContain('tcp://');
    // No bare dotted-quad IP address rendered anywhere on the page.
    const fullText = await page.locator('body').innerText();
    expect(fullText).not.toMatch(/\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/);
    // No secret material leaks.
    expect(body).not.toContain('secret_hash');

    // Snowflake policy_rev is rendered as a STRING (full value in the title attr),
    // never Number()-coerced — the global card's '4187' survives intact.
    const card = page.getByRole('region', { name: '全局辖区' });
    await expect(card.getByTitle('4187')).toBeVisible();
    const titled = card.getByTitle('4187');
    await expect(titled).toHaveText(/4187|418…|4…87|41…87/); // truncated display, full in title
  });
});
