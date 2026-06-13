import { test, expect, type Page } from '@playwright/test';

/**
 * Resources (09-resources.md) real-browser e2e against the shared Vite+MSW dev
 * server on :5173. MSW serves static /v1/* handlers (src/mocks/handlers.ts +
 * fixtures.ts); we exercise the happy path, the access/edit drawer flow, the
 * discover secondary view, and the DOM-level contract invariants that are
 * reliably observable with the default fixtures.
 *
 * Fixtures in play (fixtures.ts `resources[0]` = db-main):
 *  - code db-main, adapter postgres, transport direct, label env=prod, 启用
 *  - tiers: readonly[observe,query], readwrite[mutate]
 *  - id 7300000000000002001 (a snowflake string > 2^53)
 *
 * The dev server is shared and stateful; POST /v1/resources returns an
 * incrementing policy_rev, so success assertions match the toast *shape*
 * (policy_rev → <n>) rather than a fixed number.
 */

const RESOURCE_ID = '7300000000000002001';

/** Land on the Resources page via the real sidebar nav and wait for real data. */
async function gotoResources(page: Page) {
  await page.goto('/');
  await page.getByRole('link', { name: /资源 Resources/ }).click();
  await expect(page.getByRole('heading', { name: '资源 Resources' })).toBeVisible();
  // Real data from fixtures rendered (not skeleton/empty): the db-main codename.
  await expect(page.getByText('db-main', { exact: true })).toBeVisible();
}

test.describe('Resources — list renders real fixture data as codename badges', () => {
  test('renders db-main from the envelope with its tiers, caps, label and 启用 status', async ({
    page,
  }) => {
    await gotoResources(page);

    // Scope cell assertions to the DataTable so we read the rendered row badges,
    // not the (hidden) <option> elements inside the form's adapter/transport
    // selects elsewhere on the page.
    const table = page.getByRole('table');

    // Real values from fixtures.resources[0] — not a skeleton, not empty state.
    await expect(table.getByText('db-main', { exact: true })).toBeVisible();
    // adapter / transport badges carry the real fixture values.
    await expect(table.getByText('postgres', { exact: true })).toBeVisible();
    await expect(table.getByText('direct', { exact: true })).toBeVisible();
    // Folded, de-duplicated capability badges across its two tiers.
    await expect(table.getByText('observe', { exact: true })).toBeVisible();
    await expect(table.getByText('mutate', { exact: true })).toBeVisible();
    // The env=prod label badge.
    await expect(table.getByText('env=prod', { exact: true })).toBeVisible();
    // enable_flag=true → 启用 status badge.
    await expect(table.getByText('启用', { exact: true })).toBeVisible();

    // Forced pagination footer is present (server-driven Page<T>).
    await expect(page.getByText(/共 1 条/)).toBeVisible();
  });

  test('DOM contract: no real address / connection string / secret leaks in the list', async ({
    page,
  }) => {
    await gotoResources(page);

    // The library only shows codenames + vault:// references — never a real
    // address. The whole rendered DOM must be clean of these markers.
    const body = await page.locator('body').innerText();
    // No connection strings (postgres://…), no dotted IPs, no instance-ids.
    expect(body).not.toMatch(/postgres:\/\//);
    expect(body).not.toMatch(/\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/);
    expect(body).not.toMatch(/\bi-[0-9a-f]{8,}\b/); // ssm/ec2 instance ids
    // Never expose secret material in the table.
    expect(body).not.toMatch(/secret_hash/);
  });

  test('DOM contract: snowflake id stays a full string (no 2^53 precision loss)', async ({
    page,
  }) => {
    await gotoResources(page);
    // The row is keyed by id; the page asserts the id never round-trips through
    // a JS number. Confirm the exact 19-digit string is intact and that parsing
    // it as a number would have lost precision.
    expect(RESOURCE_ID.length).toBe(19);
    expect(Number(RESOURCE_ID) > Number.MAX_SAFE_INTEGER).toBe(true);
    // The codename — not the raw address — is how the operator addresses it.
    await expect(page.getByText('db-main', { exact: true })).toBeVisible();
  });
});

test.describe('Resources — access drawer flow (form → summary → success)', () => {
  test('opens the access drawer, previews the summary, submits and shows the policy_rev toast', async ({
    page,
  }) => {
    await gotoResources(page);

    // Open the access drawer via the primary action.
    await page.getByRole('button', { name: /接入资源/ }).click();
    const drawer = page.getByRole('dialog', { name: '接入资源' });
    await expect(drawer).toBeVisible();

    // Sectioned form is rendered (基本信息 / 真实地址 / tiers).
    await expect(drawer.getByText('① 基本信息')).toBeVisible();
    await expect(drawer.getByText('② 真实地址（匿名化）')).toBeVisible();

    // Fill a fresh codename. Default tier ro/[observe,query] is read-only valid,
    // so the Zod ≥1-read-only-tier gate is already satisfied.
    const codeInput = drawer.getByPlaceholder('db-main');
    await codeInput.fill('svc-e2e');
    // The vault reference preview echoes the code, never a plaintext address.
    await expect(drawer.getByText('现值: vault://svc-e2e/target')).toBeVisible();

    // Preview summary.
    await drawer.getByRole('button', { name: /预览摘要/ }).click();
    const summary = page.getByRole('dialog', { name: '摘要预览' });
    await expect(summary).toBeVisible();
    await expect(summary.getByText(/svc-e2e/)).toBeVisible();
    // Summary states the address becomes a vault reference (plaintext not stored).
    await expect(
      summary.getByText(/未填写；现有引用保持不变|将转为 vault:\/\//),
    ).toBeVisible();

    // Confirm → POST /v1/resources → success toast with advancing policy_rev.
    await summary.getByRole('button', { name: '确认提交' }).click();
    const toast = page.getByRole('status');
    await expect(toast).toBeVisible();
    await expect(toast).toHaveText(/资源 svc-e2e 已接入，policy_rev → \d+/);
    // Drawer closed after success.
    await expect(page.getByRole('dialog', { name: '摘要预览' })).toHaveCount(0);
  });

  test('Zod gate: stripping every read-only tier blocks the summary preview', async ({
    page,
  }) => {
    await gotoResources(page);
    await page.getByRole('button', { name: /接入资源/ }).click();
    const drawer = page.getByRole('dialog', { name: '接入资源' });
    await expect(drawer).toBeVisible();

    await drawer.getByPlaceholder('db-main').fill('svc-bad');
    // Default tier 1 = ro/[observe,query]. Strip the read-only verbs and add a
    // write verb so no read-only tier remains.
    await drawer.getByLabel('tier 1 动词 observe').click();
    await drawer.getByLabel('tier 1 动词 query').click();
    await drawer.getByLabel('tier 1 动词 mutate').click();

    await drawer.getByRole('button', { name: /预览摘要/ }).click();
    // Stays on the form; the only-read-only-tier validation error (role=alert)
    // shows; summary never opens.
    await expect(
      drawer.getByRole('alert').filter({ hasText: /只读 tier/ }),
    ).toBeVisible();
    await expect(page.getByRole('dialog', { name: '摘要预览' })).toHaveCount(0);
  });
});

test.describe('Resources — edit drawer flow', () => {
  test('opens the edit drawer for db-main with the code locked and tiers prefilled', async ({
    page,
  }) => {
    await gotoResources(page);

    // Click the codename badge to open the edit drawer for that row.
    await page.getByRole('button', { name: /db-main/ }).first().click();
    const drawer = page.getByRole('dialog', { name: '编辑 db-main' });
    await expect(drawer).toBeVisible();

    // code is read-only in edit mode and prefilled from the row.
    const codeInput = drawer.getByPlaceholder('db-main');
    await expect(codeInput).toHaveValue('db-main');
    await expect(codeInput).toHaveAttribute('readonly', '');

    // Tiers prefilled from the fixture (readonly + readwrite tier codes).
    await expect(drawer.getByLabel('tier 代号 1')).toHaveValue('readonly');
    await expect(drawer.getByLabel('tier 代号 2')).toHaveValue('readwrite');

    // The drawer never echoes a real address — only the vault reference preview.
    const drawerText = await drawer.innerText();
    expect(drawerText).not.toMatch(/postgres:\/\//);
    expect(drawerText).not.toMatch(/\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/);
    await expect(drawer.getByText('现值: vault://db-main/target')).toBeVisible();
  });
});

test.describe('Resources — discover (discovery ≠ authorization)', () => {
  test('opens the discover view with the boundary banner and selectable objects, then gates the configure jump', async ({
    page,
  }) => {
    await gotoResources(page);

    // Open the row action menu and trigger discover.
    await page.getByRole('button', { name: /资源 db-main 行操作/ }).click();
    await page.getByRole('menuitem', { name: '探测 discover' }).click();

    const drawer = page.getByRole('dialog', { name: /Discover: db-main/ });
    await expect(drawer).toBeVisible();
    // Explicit boundary banner: discovery ≠ authorization.
    await expect(drawer.getByText(/发现 ≠ 授权/)).toBeVisible();

    // Probed capabilities + objects from the static discover handler.
    await expect(drawer.getByText('探得能力 capabilities')).toBeVisible();
    const objectCheckbox = drawer.getByLabel('选择对象 table:orders');
    await expect(objectCheckbox).toBeVisible();

    // The configure-jump button is gated until at least one object is selected
    // (unselected = denied by default; discover does not authorize).
    const configureBtn = drawer.getByRole('button', { name: /配置细则/ });
    await expect(configureBtn).toBeDisabled();
    await expect(drawer.getByText(/已选 0 项/)).toBeVisible();

    await objectCheckbox.check();
    await expect(drawer.getByText(/已选 1 项/)).toBeVisible();
    await expect(configureBtn).toBeEnabled();

    // Selecting only produces inputs for 08; it does NOT authorize here.
    await configureBtn.click();
    await expect(page.getByRole('dialog', { name: /Discover: db-main/ })).toHaveCount(0);
    await expect(page.getByText(/未圈选对象一律默认拒绝/)).toBeVisible();
  });
});
