import { test, expect, type Page } from '@playwright/test';

/**
 * Roles 角色 — real-browser e2e against the shared Vite+MSW dev server (:5173).
 *
 * Browser MSW serves STATIC handlers (src/mocks/handlers.ts + fixtures.ts): the
 * roles collection is a SINGLE role `observer` (narrow, no inheritance, effective
 * {observe,query}, version 4, snowflake id 7300000000000001001). POST /v1/roles
 * always returns a fresh policy_rev (WriteAck). We therefore cover real-browser
 * happy paths + DOM-level contract invariants reliably observable from those
 * static fixtures; error/edge/409 cases are owned by the vitest unit tests
 * (which can per-test server.use()), so we do NOT re-create them here.
 *
 * Selectors are borrowed from the page source + the verified vitest selectors
 * (getByRole/getByLabelText/getByText), kept semantically identical.
 */

const ROLE_NAME = 'observer';
const ROLE_ID_FULL = '7300000000000001001';
const ROLE_ID_TRUNC = '7300…1001';

async function gotoRoles(page: Page) {
  await page.goto('/roles');
  await expect(page.getByRole('heading', { name: '角色 Roles' })).toBeVisible();
}

/** The DataTable <table>. Role names also appear in the LadderGraph, so
 * table-scoped queries disambiguate (same discipline as the vitest harness). */
function table(page: Page) {
  return page.getByRole('table');
}

test.describe('Roles 角色 — real-browser e2e', () => {
  test('renders REAL fixture data (not skeleton/empty): observer row with its daemon effective set, id, version', async ({
    page,
  }) => {
    await gotoRoles(page);

    // Header + primary action present.
    await expect(page.getByRole('button', { name: /新建角色/ })).toBeVisible();

    // The observer row is the real fixture row, in the TABLE (not the ladder).
    const row = table(page).getByRole('row').filter({ hasText: ROLE_NAME });
    await expect(row).toBeVisible();

    // Daemon-reported effective verb set: observe + query (and NOT mutate/manage).
    await expect(row.getByText('observe', { exact: true })).toBeVisible();
    await expect(row.getByText('query', { exact: true })).toBeVisible();
    await expect(row.getByText('mutate', { exact: true })).toHaveCount(0);
    await expect(row.getByText('manage', { exact: true })).toHaveCount(0);

    // Narrow role: 继承自 column shows the em-dash placeholder (no parent).
    await expect(row.getByText('—', { exact: true })).toBeVisible();

    // ver column reflects the fixture version (4), not a skeleton/empty.
    await expect(row.getByText('4', { exact: true })).toBeVisible();
  });

  test('LadderGraph renders from real data: observer is a floating narrow role + the destroy footnote', async ({
    page,
  }) => {
    await gotoRoles(page);

    const ladder = page.getByRole('region', { name: '继承阶梯' });
    await expect(ladder).toBeVisible();

    // Single narrow role → no rungs; observer is listed under 游离窄角色.
    await expect(ladder.getByText('尚无阶梯角色')).toBeVisible();
    await expect(ladder.getByText(ROLE_NAME, { exact: true })).toBeVisible();

    // The fixed fact footnote is present (read-only, daemon-fact).
    await expect(ladder.getByText(/destroy 不进任何角色/)).toBeVisible();
  });

  test('snowflake id discipline: middle-truncated in the cell, FULL string in the title (no precision loss)', async ({
    page,
  }) => {
    await gotoRoles(page);

    // The full id is preserved verbatim in the title attr; the cell is truncated.
    const idCell = page.locator(`[title="${ROLE_ID_FULL}"]`);
    await expect(idCell).toBeVisible();
    await expect(idCell).toHaveText(ROLE_ID_TRUNC);

    // Contract: the id is a STRING — coercing to Number WOULD round it, so the
    // full title value proves precision is preserved.
    expect(Number(ROLE_ID_FULL)).toBeGreaterThan(Number.MAX_SAFE_INTEGER);
    expect(String(Number(ROLE_ID_FULL))).not.toBe(ROLE_ID_FULL);
  });

  test('name filter narrows the current page (renders the matching row, hides non-matches)', async ({
    page,
  }) => {
    await gotoRoles(page);
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toBeVisible();

    // A non-matching query empties the table; the matching one brings it back.
    await page.getByLabel('按名称筛选').fill('zzz-no-match');
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toHaveCount(0);

    await page.getByLabel('按名称筛选').fill('obs');
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toBeVisible();
  });

  test('verb filter uses the daemon effective set: manage hides observer, query shows it', async ({
    page,
  }) => {
    await gotoRoles(page);
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toBeVisible();

    // observer's effective set has no `manage` → filtered out of the table.
    await page.getByLabel('按动词筛选').selectOption('manage');
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toHaveCount(0);

    // It does carry `query` → comes back.
    await page.getByLabel('按动词筛选').selectOption('query');
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toBeVisible();
  });

  test('create drawer: empty-form Zod blocks (名称必填 + ≥1 动词), and admin variants are hard-disabled with NO admin control', async ({
    page,
  }) => {
    await gotoRoles(page);

    await page.getByRole('button', { name: /新建角色/ }).click();
    const drawer = page.getByRole('dialog', { name: '新建角色' });
    await expect(drawer).toBeVisible();

    // Empty form → submit-to-summary surfaces Zod errors, stays on the form
    // (no 写入摘要 summary view appears).
    await drawer.getByRole('button', { name: /预览摘要/ }).click();
    await expect(drawer.getByRole('alert').filter({ hasText: '名称不能为空' })).toBeVisible();
    await expect(drawer.getByRole('alert').filter({ hasText: '至少勾选一个动词' })).toBeVisible();
    await expect(drawer.getByLabel('写入摘要')).toHaveCount(0);

    // admin name variants → convenience hard-block: notice shown + submit disabled.
    for (const variant of ['admin', 'Admin', '  admin  ']) {
      await drawer.getByLabel('名称').fill(variant);
      await expect(drawer.getByText(/admin 不可作为可授予角色/)).toBeVisible();
      await expect(drawer.getByRole('button', { name: /预览摘要/ })).toBeDisabled();
    }

    // Structural absence: there is NO admin control in the picker; destroy exists
    // but is disabled (un-pickable by design).
    await expect(drawer.getByLabel('admin')).toHaveCount(0);
    await expect(drawer.getByLabel('destroy')).toBeDisabled();
  });

  test('create happy path: fill → live preview → 预览摘要 summary → 提交 → success banner (policy_rev 前进) + drawer closes', async ({
    page,
  }) => {
    await gotoRoles(page);
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toBeVisible();

    await page.getByRole('button', { name: /新建角色/ }).click();
    const drawer = page.getByRole('dialog', { name: '新建角色' });
    await expect(drawer).toBeVisible();

    await drawer.getByLabel('名称').fill('analyst');
    await drawer.getByLabel('observe', { exact: true }).check();
    await drawer.getByLabel('query', { exact: true }).check();

    // Local effective preview reflects the picked verbs and is labelled daemon-final.
    const preview = drawer.getByLabel('有效动词集预览');
    await expect(preview.getByText('observe', { exact: true })).toBeVisible();
    await expect(preview.getByText('query', { exact: true })).toBeVisible();
    await expect(drawer.getByText(/最终以 daemon 为准/)).toBeVisible();

    // → summary view shows exactly what will be written.
    await drawer.getByRole('button', { name: /预览摘要/ }).click();
    const summary = drawer.getByLabel('写入摘要');
    await expect(summary).toBeVisible();
    await expect(summary.getByText('analyst', { exact: true })).toBeVisible();
    // Create carries no optimistic-lock version row.
    await expect(summary.getByText('乐观锁 version（编辑携带）')).toHaveCount(0);

    // 提交 → optimistic success banner (policy_rev advances), drawer closes.
    await drawer.getByRole('button', { name: '提交' }).click();
    const banner = page.getByRole('status').filter({ hasText: /policy_rev 前进至 \d+/ });
    await expect(banner).toBeVisible();
    await expect(drawer).toHaveCount(0);
  });

  test('edit happy path: row menu → 编辑 prefills name + carries version 4 in summary → 提交 → success banner', async ({
    page,
  }) => {
    await gotoRoles(page);
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toBeVisible();

    // Open the row action menu, then edit.
    await page.getByRole('button', { name: /角色 observer 操作/ }).click();
    await page.getByRole('menuitem', { name: /编辑动词集/ }).click();

    const drawer = page.getByRole('dialog', { name: '编辑角色' });
    await expect(drawer).toBeVisible();
    // Prefilled with the daemon name.
    await expect(drawer.getByLabel('名称')).toHaveValue('observer');

    // Summary carries the optimistic-lock version read at load (4).
    await drawer.getByRole('button', { name: /预览摘要/ }).click();
    const summary = drawer.getByLabel('写入摘要');
    await expect(summary).toBeVisible();
    await expect(summary.getByText('乐观锁 version（编辑携带）')).toBeVisible();
    await expect(summary.getByText('4', { exact: true })).toBeVisible();

    await drawer.getByRole('button', { name: '提交' }).click();
    await expect(
      page.getByRole('status').filter({ hasText: /policy_rev 前进至 \d+/ }),
    ).toBeVisible();
    await expect(drawer).toHaveCount(0);
  });

  test('delete danger flow: ConfirmDialog requires explicit ack before delete fires; ack → 删除 → success banner', async ({
    page,
  }) => {
    await gotoRoles(page);
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toBeVisible();

    await page.getByRole('button', { name: /角色 observer 操作/ }).click();
    await page.getByRole('menuitem', { name: /删除/ }).click();

    const dialog = page.getByRole('dialog', { name: /删除角色/ });
    await expect(dialog).toBeVisible();
    // Danger copy: states the cascade impact (fact, not a judgment).
    await expect(dialog.getByText(/会影响引用它的绑定的授权展开/)).toBeVisible();
    // Summary lists the target name + version (the optimistic-lock token).
    await expect(dialog.getByText('observer', { exact: true })).toBeVisible();
    await expect(dialog.getByText('4', { exact: true })).toBeVisible();

    // Confirm WITHOUT acking → nothing fires, dialog stays open, no success banner.
    await dialog.getByRole('button', { name: '删除' }).click();
    await expect(dialog).toBeVisible();
    await expect(page.getByRole('status').filter({ hasText: /policy_rev 前进/ })).toHaveCount(0);

    // Ack → confirm → optimistic success banner; dialog closes.
    await dialog.getByLabel('我已知晓影响').check();
    await dialog.getByRole('button', { name: '删除' }).click();
    await expect(
      page.getByRole('status').filter({ hasText: /policy_rev 前进至 \d+/ }),
    ).toBeVisible();
    await expect(dialog).toHaveCount(0);
  });

  test('DOM contract invariants: no real addresses/secrets/plaintext keys; admin never appears anywhere on the page', async ({
    page,
  }) => {
    await gotoRoles(page);
    await expect(table(page).getByText(ROLE_NAME, { exact: true })).toBeVisible();

    const bodyText = (await page.locator('body').innerText()).toLowerCase();

    // No connection strings / driver URIs / raw IP addresses (roles are
    // resource-agnostic verb sets — must never leak a real address).
    expect(bodyText).not.toMatch(/postgres:\/\//);
    expect(bodyText).not.toMatch(/\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/);

    // No secret material ever surfaces on this page.
    expect(bodyText).not.toContain('secret_hash');
    expect(bodyText).not.toContain('secret_ref');
    expect(bodyText).not.toContain('vault://');

    // admin is a structural absence: no admin role row, no admin control. (It is
    // allowed inside the page's own copy as "admin 不可声明" — which is why we
    // assert there's no admin ROW in the table rather than scanning prose.)
    await expect(table(page).getByText('admin', { exact: true })).toHaveCount(0);

    // Re-open the create drawer to confirm the picker exposes no admin control.
    await page.getByRole('button', { name: /新建角色/ }).click();
    const drawer = page.getByRole('dialog', { name: '新建角色' });
    await expect(drawer.getByLabel('admin')).toHaveCount(0);
  });
});
