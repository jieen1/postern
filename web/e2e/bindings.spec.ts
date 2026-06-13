import { test, expect, type Page } from '@playwright/test';

/**
 * 绑定 Bindings — real-browser e2e (07-bindings.md).
 *
 * Runs against the shared Vite+MSW dev server on :5173. The browser MSW worker
 * serves the STATIC handlers/fixtures (src/mocks/*): exactly one binding —
 * agent-order-bot · observer · resource db-main (id 7300000000000001234,
 * expanded_resources ['db-main'], version 2). `POST /v1/bindings` is handled
 * (returns a fresh policy_rev); the expansion-preview probe and the delete
 * endpoint are NOT handled (onUnhandledRequest:'bypass'), so in-browser they
 * fail-closed — this spec asserts that REAL observable behavior, not the
 * vitest mock-server reality.
 *
 * Scope: real-browser happy path + cross-page flow + DOM-level contract
 * invariants. Per-test error/boundary cases are covered by vitest (server.use).
 */

/**
 * The principal cell in a data row is a clickable button (the same name also
 * appears as a filter <option>, so scope to the row button to stay unambiguous).
 */
function principalCell(page: Page) {
  return page.getByRole('button', { name: 'agent-order-bot' });
}

/** Navigate to the bindings page via the sidebar and wait for the real row. */
async function gotoBindings(page: Page) {
  await page.goto('/');
  await page.getByRole('link', { name: /绑定 Bindings/ }).click();
  await expect(page.getByRole('heading', { name: /绑定 Bindings/ })).toBeVisible();
  // The single fixture binding renders (real data, not skeleton/empty state).
  await expect(principalCell(page)).toBeVisible();
}

test.describe('绑定 Bindings — list rendering from fixtures', () => {
  test('renders the one real binding row from fixtures (not a skeleton/empty)', async ({
    page,
  }) => {
    await gotoBindings(page);

    // Real fixture values surface: principal / role / resource scope / expansion.
    await expect(principalCell(page)).toBeVisible();
    await expect(page.getByRole('button', { name: 'observer' })).toBeVisible();
    // resource-kind scope renders db-main as a ResourceCodeBadge.
    await expect(page.getByText('db-main').first()).toBeVisible();
    // expanded_resources:['db-main'] ⇒ "1 资源" count badge (not 0/无匹配).
    await expect(page.getByText('1 资源')).toBeVisible();
    await expect(page.getByText(/无匹配/)).toHaveCount(0);

    // The empty/CTA state is NOT shown — real data took its place.
    await expect(page.getByText('还没有绑定')).toHaveCount(0);
  });

  test('snowflake id is middle-truncated yet keeps its full value verbatim (no precision loss)', async ({
    page,
  }) => {
    await gotoBindings(page);

    // Full id is preserved in the title attribute — string discipline, never Number.
    const idEl = page.getByTitle('7300000000000001234');
    await expect(idEl).toBeVisible();
    // The id really is past the safe-integer boundary (precision trap).
    expect(Number('7300000000000001234') > Number.MAX_SAFE_INTEGER).toBe(true);
    // Displayed head…tail, not the rounded number.
    await expect(idEl).toHaveText('7300…1234');
  });

  test('DOM contract: no real address / connection string / secret material is ever rendered', async ({
    page,
  }) => {
    await gotoBindings(page);

    const body = (await page.locator('body').textContent()) ?? '';
    // Resources are shown as codes only — never a real address or connstring.
    expect(body).not.toMatch(/postgres:\/\//);
    expect(body).not.toMatch(/redis:\/\//);
    expect(body).not.toMatch(/\b\d{1,3}(?:\.\d{1,3}){3}\b/); // dotted-quad IP
    // Bindings never touch credentials — secret material must not leak.
    expect(body.toLowerCase()).not.toContain('secret_hash');
    expect(body).not.toContain('secret_ref');
    expect(body).not.toContain('vault://');
  });
});

test.describe('绑定 Bindings — filters (no hidden-row count leak)', () => {
  test('Scope 类型 = resource keeps the resource binding; an unmatched Role hides it', async ({
    page,
  }) => {
    await gotoBindings(page);

    // Filter by Scope 类型 = selector — the one binding is resource-kind, hidden.
    await page.getByLabel('按 Scope 类型筛选').selectOption('selector');
    await expect(principalCell(page)).toHaveCount(0);
    // Switch back to resource — the row returns.
    await page.getByLabel('按 Scope 类型筛选').selectOption('resource');
    await expect(principalCell(page)).toBeVisible();

    // Filtering it out must NOT announce how many rows were hidden (存在性不泄露).
    await page.getByLabel('按 Scope 类型筛选').selectOption('selector');
    await expect(principalCell(page)).toHaveCount(0);
    await expect(page.getByText(/被隐藏/)).toHaveCount(0);
    await expect(page.getByText(/还有 \d+ 行/)).toHaveCount(0);
  });
});

test.describe('绑定 Bindings — 查看展开 detail drawer', () => {
  test('row menu → 查看展开 opens a read-only drawer showing the daemon-reported expansion', async ({
    page,
  }) => {
    await gotoBindings(page);

    await page.getByRole('button', { name: '行操作' }).click();
    await page.getByRole('menuitem', { name: '查看展开' }).click();

    const drawer = page.getByRole('dialog', { name: '绑定展开详情' });
    await expect(drawer).toBeVisible();
    // Metadata + expansion result (db-main) from fixtures, shown as a code badge.
    await expect(drawer.getByText('agent-order-bot')).toBeVisible();
    await expect(drawer.getByText('observer')).toBeVisible();
    await expect(drawer.getByText('db-main').first()).toBeVisible();
    // No precision loss in the drawer either: full id in title.
    await expect(drawer.getByTitle('7300000000000001234')).toBeVisible();
    // It is NOT an error/empty state — there is a real expansion.
    await expect(drawer.getByText('展开为 0 个资源（无匹配标签）')).toHaveCount(0);
  });
});

test.describe('绑定 Bindings — create flow (drawer → ScopeEditor → summary → confirm)', () => {
  test('end-to-end create: fill → 预览摘要 → 确认创建 → success toast (policy_rev↑), drawer closes', async ({
    page,
  }) => {
    await gotoBindings(page);

    // Open the create FormDrawer.
    await page.getByRole('button', { name: /新建绑定/ }).click();
    const drawer = page.getByRole('dialog', { name: '新建绑定' });
    await expect(drawer).toBeVisible();

    // The summary trigger is gated until principal/role/scope are valid.
    const previewBtn = drawer.getByRole('button', { name: '预览摘要并创建' });
    await expect(previewBtn).toBeDisabled();

    // Fill principal + role (the only non-admin role offered is observer).
    await drawer.getByLabel('Principal *').selectOption('agent-order-bot');
    await drawer.getByLabel('Role *').selectOption('observer');

    // ScopeEditor: selector mode is default; fill one host:value label row.
    // What-you-see-is-what-you-send: the spec preview reflects the typed value.
    await drawer.getByLabel('标签值 1').fill('A');
    // JsonViewer pretty-prints the exact spec that will be submitted.
    await expect(drawer.getByLabel('selector spec 预览')).toContainText('"value": "A"');

    // Now the summary trigger is enabled.
    await expect(previewBtn).toBeEnabled();
    await previewBtn.click();

    // Summary dialog echoes the intended principal / role / scope before commit.
    const summary = page.getByRole('dialog', { name: '确认创建绑定' });
    await expect(summary).toBeVisible();
    await expect(summary.getByText('agent-order-bot')).toBeVisible();
    await expect(summary.getByText('observer')).toBeVisible();
    await expect(summary.getByText(/selector/)).toBeVisible();

    // Commit → POST /v1/bindings (handled) → success toast advances policy_rev.
    await summary.getByRole('button', { name: '确认创建' }).click();

    // Optimistic success: toast announces policy_rev advancing; drawer closes.
    const toast = page.getByRole('status').filter({ hasText: /已创建，policy_rev 前进至/ });
    await expect(toast).toBeVisible();
    await expect(toast).toContainText(/policy_rev 前进至 \d+/);
    await expect(page.getByRole('dialog', { name: '新建绑定' })).toHaveCount(0);
    await expect(page.getByRole('dialog', { name: '确认创建绑定' })).toHaveCount(0);
  });

  test('expansion-preview probe is fail-closed in-browser: "按未授权对待", never an optimistic resource set', async ({
    page,
  }) => {
    await gotoBindings(page);

    await page.getByRole('button', { name: /新建绑定/ }).click();
    const drawer = page.getByRole('dialog', { name: '新建绑定' });
    await expect(drawer).toBeVisible();

    await drawer.getByLabel('Role *').selectOption('observer');
    // resource mode: switch via the radio, then pick a real code so the probe fires.
    await drawer.getByRole('radio', { name: 'resource' }).check();
    await drawer.getByLabel('添加资源代号').selectOption('db-main');

    // The preview endpoint is unhandled (bypassed) ⇒ probe fails ⇒ fail-closed
    // alert. It must NEVER fall back to "show all resources".
    const failClosed = drawer
      .getByRole('alert')
      .filter({ hasText: '无法计算展开——按未授权对待' });
    await expect(failClosed).toBeVisible();
  });
});

test.describe('绑定 Bindings — delete danger confirm (anti-misclick gate)', () => {
  test('row menu → 删除绑定 opens a ConfirmDialog naming the 缩权方向, gated on the DELETE word', async ({
    page,
  }) => {
    await gotoBindings(page);

    await page.getByRole('button', { name: '行操作' }).click();
    await page.getByRole('menuitem', { name: '删除绑定' }).click();

    const dialog = page.getByRole('dialog', { name: '删除绑定' });
    await expect(dialog).toBeVisible();
    // Summary names the affected resource and the 缩权方向 (shrinking) direction.
    await expect(dialog.getByText(/db-main/)).toBeVisible();
    await expect(dialog.getByText(/缩权方向/)).toBeVisible();
    // No allow / 放行 affordance on a danger dialog (deny-discipline parity).
    await expect(dialog.getByRole('button', { name: /放行|allow/i })).toHaveCount(0);

    // Anti-misclick: the confirm button is disabled until the word is typed.
    const confirmBtn = dialog.getByRole('button', { name: '确认删除' });
    await expect(confirmBtn).toBeDisabled();
    await dialog.getByRole('textbox').fill('DELETE');
    await expect(confirmBtn).toBeEnabled();
  });
});
