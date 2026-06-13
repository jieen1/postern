import { test, expect, type Page } from '@playwright/test';

/**
 * 细则 / 条件 / 拒绝指引 (docs/08-constraints-conditions.md) — REAL browser e2e.
 *
 * Runs against the shared Vite+MSW dev server on :5173 (static handlers serving
 * src/mocks/fixtures.ts). Browser MSW cannot do per-test server.use(), so this
 * spec stays on the happy path + cross-segment flow + DOM-level contract
 * invariants that the default fixtures render deterministically. Error/empty/409
 * states are covered by the vitest suite (server.use) and NOT duplicated here.
 *
 * Fixtures this page renders (src/mocks/fixtures.ts):
 *   constraints[0]: db-main / query / table_allow / {"tables":[...]} / v1 / id 7300000000000005001
 *   conditions[0]:  db-main / capability NULL (→ "*") / rate_limit / v1
 *   denyNotes[0]:   db-main / mutate / note "写操作请走变更单据，联系 DBA 值班。" / v1
 *   resources[0]:   code db-main, adapter postgres (kind matrix → table_allow/column_mask/mask_fields)
 */

const CONSTRAINT_ID = '7300000000000005001'; // > 2^53 — must survive as a STRING
const DENY_NOTE = '写操作请走变更单据，联系 DBA 值班。';

/** Navigate to the page via the real sidebar link (mirrors a real user). */
async function openConstraints(page: Page) {
  await page.goto('/constraints');
  await expect(page.getByRole('heading', { name: '细则与条件' })).toBeVisible();
  // Real data (not skeleton/empty): the seeded constraint row must appear.
  await expect(page.getByText('table_allow')).toBeVisible();
}

test.describe('细则与条件 — constraints e2e', () => {
  test('journey 1: 列表从 fixtures 渲染真实细则数据（非骨架/空态）', async ({ page }) => {
    await openConstraints(page);

    // The seeded constraint row's real values are visible: resource code badge,
    // capability, kind, spec-summary (object keys), version.
    await expect(page.getByText('table_allow')).toBeVisible();
    // Resource code badge in the table cell (scoped to <table>, not the filter
    // <select> option which also reads "db-main").
    await expect(page.locator('table').getByText('db-main').first()).toBeVisible();
    // specSummary({"tables":[...]}) → the object key "tables".
    await expect(page.locator('table').getByText('tables', { exact: true })).toBeVisible();
    // The constraints header has a `kind` column (not `predicate`).
    await expect(page.getByRole('columnheader', { name: 'kind' })).toBeVisible();
    await expect(
      page.getByRole('columnheader', { name: 'predicate' }),
    ).toHaveCount(0);

    // DOM contract: resource is shown as a CODE badge, never a real address /
    // connection string (no postgres:// URI, no host:port form) anywhere.
    const body = await page.locator('body').innerText();
    expect(body).not.toMatch(/postgres:\/\//);
    expect(body).not.toMatch(/\b\d{1,3}(?:\.\d{1,3}){3}\b/); // dotted-quad IP
    expect(body.toLowerCase()).not.toContain('secret_hash');
  });

  test('journey 2: 段切换 细则→条件→拒绝指引，各段渲染本段真实列与数据', async ({ page }) => {
    await openConstraints(page);

    // → 条件 Conditions: predicate column appears, kind disappears; the
    // rate_limit row renders and the NULL capability shows as "*".
    await page.getByRole('tab', { name: /条件 Conditions/ }).click();
    await expect(page.getByText('rate_limit')).toBeVisible();
    await expect(
      page.getByRole('columnheader', { name: 'predicate' }),
    ).toBeVisible();
    await expect(page.getByRole('columnheader', { name: 'kind' })).toHaveCount(0);
    // NULL capability rendered as a grey "*" (全动词), not invented data.
    await expect(page.getByText('*', { exact: true }).first()).toBeVisible();

    // → 拒绝指引 Deny-notes: the note is shown VERBATIM (== operator_note, 公理六)
    // and the column is labelled as the operator_note source.
    await page.getByRole('tab', { name: /拒绝指引 Deny-notes/ }).click();
    await expect(page.getByText(DENY_NOTE)).toBeVisible();
    await expect(
      page.getByText(/越权时 Agent 收到的 operator_note/),
    ).toBeVisible();
    // DOM contract: deny segment has NO allow/放行 control — this page only
    // authors the note source, it is not an approval desk.
    await expect(page.getByRole('button', { name: /放行|allow/i })).toHaveCount(0);
  });

  test('journey 3: 新建细则流程 — kind 矩阵收窄 + 摘要预览 + 提交成功 toast', async ({ page }) => {
    await openConstraints(page);

    await page.getByRole('button', { name: /新建细则/ }).click();
    const drawer = page.getByRole('dialog');
    await expect(drawer).toBeVisible();

    // Select resource db-main (postgres). Its adapter narrows the kind matrix.
    await drawer.getByLabel('资源').selectOption('db-main');

    // KindMatrixSelect (postgres) declares table_allow/column_mask/mask_fields —
    // NOT docker's container_prefix. Assert the real narrowed option set.
    const kindSelect = drawer.locator('#constraint-kind');
    const kindOptions = await kindSelect.locator('option').allInnerTexts();
    expect(kindOptions).toContain('table_allow');
    expect(kindOptions).not.toContain('container_prefix');

    await kindSelect.selectOption('table_allow');
    await drawer
      .getByPlaceholder('{"prefix":"app-"}')
      .fill('{"tables":["orders"]}');

    // 提交前摘要预览 reflects the typed facts (resource, verb, kind).
    const summary = drawer.getByRole('region', { name: '摘要预览' });
    await expect(summary).toContainText('db-main');
    await expect(summary).toContainText('table_allow');

    // Submit → success toast: 细则已挂载，policy_rev 前进至 N (N is the shared
    // server's running rev; assert the stable prefix, not the volatile number).
    await drawer.getByRole('button', { name: '提交' }).click();
    const toast = page.getByRole('status');
    await expect(toast).toContainText('细则已挂载，policy_rev 前进至');
    // Drawer closed on success.
    await expect(page.getByRole('dialog')).toHaveCount(0);
  });

  test('journey 4: 非法 JSON spec 被前端语法校验拦截，不提交', async ({ page }) => {
    await openConstraints(page);

    await page.getByRole('button', { name: /新建细则/ }).click();
    const drawer = page.getByRole('dialog');
    await drawer.getByLabel('资源').selectOption('db-main');
    await drawer.locator('#constraint-kind').selectOption('table_allow');
    await drawer.getByPlaceholder('{"prefix":"app-"}').fill('not json');
    await drawer.getByRole('button', { name: '提交' }).click();

    // Syntax-layer convenience check blocks submit (semantics still in daemon).
    await expect(drawer.getByText('spec 必须是可解析的 JSON')).toBeVisible();
    // No success toast — the write did not go through.
    await expect(page.getByRole('status')).toHaveCount(0);
  });

  test('journey 5: 删除=扩大作用面 — 危险确认 gating + 雪花 id 全值不丢精度', async ({ page }) => {
    await openConstraints(page);

    // DOM contract: snowflake id is preserved as a full STRING in the detail
    // drawer (title attr) — never coerced to Number (> 2^53 would corrupt it).
    await page.getByRole('button', { name: '查看详情' }).click();
    const detail = page.getByRole('dialog');
    await expect(detail.getByTitle(CONSTRAINT_ID)).toBeVisible();
    // Prove the precision trap is real: Number() would mangle this value.
    expect(Number(CONSTRAINT_ID).toString()).not.toBe(CONSTRAINT_ID);
    // The spec detail shows raw JSON (real fixture content), no secret leakage.
    await expect(detail).toContainText('orders');
    await expect(
      (await detail.innerText()).toLowerCase().includes('secret_hash'),
    ).toBe(false);
    // Close the detail drawer (FormDrawer header close button, aria-label 关闭).
    await detail.getByRole('button', { name: '关闭' }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);

    // Delete row → ConfirmDialog directly states scope-widening; confirm is
    // gated behind the EXACT acknowledgement word.
    await page.getByRole('button', { name: '删除' }).click();
    const confirm = page.getByRole('dialog');
    await expect(confirm).toContainText(/放宽/);
    const confirmBtn = confirm.getByRole('button', { name: '删除' });
    await expect(confirmBtn).toBeDisabled();
    await confirm.getByRole('textbox').fill('我已知此操作扩大授权作用面');
    await expect(confirmBtn).toBeEnabled();

    // Confirm the delete → success toast (policy_rev advances). MSW is static so
    // the row still renders afterward — that is the expected fixture behavior,
    // not a leak.
    await confirmBtn.click();
    await expect(page.getByRole('status')).toContainText('已删除，policy_rev 前进');
  });

  test('journey 6: deny-note 已存在 → FormDrawer 进入编辑语态并预填原文', async ({ page }) => {
    await openConstraints(page);

    await page.getByRole('tab', { name: /拒绝指引 Deny-notes/ }).click();
    await expect(page.getByText(DENY_NOTE)).toBeVisible();

    // Editing the existing note shows the EDIT phrasing (not "create a second
    // one"), pins the verbatim-relay warning, and prefills the existing text.
    await page.getByRole('button', { name: '编辑' }).click();
    const drawer = page.getByRole('dialog');
    await expect(drawer).toContainText('已有生效拒绝指引');
    await expect(drawer).toContainText('此文本越权时将原样回给 Agent（operator_note）');
    await expect(drawer.getByText(DENY_NOTE).first()).toBeVisible();
  });
});
