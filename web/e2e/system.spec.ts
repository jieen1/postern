import { test, expect, type Page } from '@playwright/test';

/**
 * Real-browser e2e for the System page (设计文档 12-system.md): a Tab container
 * over Approvals / Settings / Import-Export / Shutdown.
 *
 * Runs against the shared Vite+MSW dev server on :5173. The browser MSW is the
 * STATIC handlers/fixtures — no per-test server.use(). So this focuses on the
 * real-browser happy paths + cross-tab flow + DOM-level contract invariants that
 * the default fixtures render reliably. Error/edge states (read fail, 409, 422
 * whole-reject, big-id adjudication) are covered exhaustively by the vitest unit
 * tests via server.use() and are intentionally NOT duplicated here.
 *
 * What the static browser fixtures render (src/mocks/fixtures.ts + handlers.ts):
 *   - settings: approval.enabled=false, approval.on_timeout=deny (locked),
 *     audit.fsync=always, audit.retention_days=90, audit.exporter.otel.enabled=false
 *   - approvals: [] (empty) → EmptyState, approval disabled → no 裁决 control
 *   - POST /settings → { policy_rev: "<incrementing>" }
 *   - POST /export → toml "# postern policy export\npolicy_rev = 4187\n"
 *   - POST /import (dry-run & apply) → { added:2, changed:1, deleted:0 }
 */

// Navigate to the System page via the real sidebar link, then assert the page
// heading is the active panel (not just the tab button).
async function gotoSystem(page: Page) {
  await page.goto('/');
  await page.getByRole('link', { name: /系统 System/ }).click();
  await expect(page.getByRole('heading', { name: '系统 System' })).toBeVisible();
}

test.describe('System page', () => {
  test('Journey 1 — four tabs switch the active panel (cross-tab flow)', async ({
    page,
  }) => {
    await gotoSystem(page);

    const tabs = page.getByRole('tab');
    await expect(tabs).toHaveCount(4);

    // Default tab is Approvals: its section heading (a heading, not the tab
    // button) is shown and the tab is aria-selected.
    await expect(
      page.getByRole('tab', { name: '审批队列 Approvals' }),
    ).toHaveAttribute('aria-selected', 'true');
    await expect(
      page.getByRole('heading', { name: '审批队列 Approvals' }),
    ).toBeVisible();

    // → Settings: its locked key heading-level content appears, and the
    // Approvals section heading is gone (panel swapped, not stacked).
    await page.getByRole('tab', { name: '设置 Settings' }).click();
    await expect(
      page.getByRole('heading', { name: '设置 Settings' }),
    ).toBeVisible();
    await expect(page.getByText('approval.on_timeout')).toBeVisible();
    await expect(
      page.getByRole('heading', { name: '审批队列 Approvals' }),
    ).toHaveCount(0);

    // → Import/Export: both export and import sub-cards render.
    await page.getByRole('tab', { name: '导入导出' }).click();
    await expect(
      page.getByRole('heading', { name: '导入导出 Import / Export' }),
    ).toBeVisible();
    await expect(page.getByText('导出 TOML')).toBeVisible();

    // → Shutdown: the danger control is present.
    await page.getByRole('tab', { name: '关停 Shutdown' }).click();
    await expect(
      page.getByRole('heading', { name: '关停 Shutdown' }),
    ).toBeVisible();
    await expect(
      page.getByRole('button', { name: '关停 daemon' }),
    ).toBeVisible();
  });

  test('Journey 2 — Approvals renders the real disabled EmptyState; no allow/裁决 control', async ({
    page,
  }) => {
    await gotoSystem(page);
    // Approvals is the default tab.

    // Real fixture data (not skeleton/fake rows): the EmptyState for the
    // default-disabled queue.
    await expect(page.getByText('审批未启用，无挂起项。')).toBeVisible();

    // Standing fact from the live settings read: approval.enabled = false,
    // rendered verbatim in the info banner.
    await expect(page.getByText('false', { exact: true })).toBeVisible();

    // DOM contract — deny-side default: with approval.enabled=false there is NO
    // adjudication (放行/allow) BUTTON anywhere on the tab. escalate folds to
    // deny; the UI offers no human override. (The EmptyState prose "无需人工裁决"
    // mentions the word, so we assert on the actionable control, not raw text.)
    await expect(page.getByRole('button', { name: '裁决' })).toHaveCount(0);
  });

  test('Journey 3 — Settings renders real values; locked on_timeout=deny; edit→summary→save toast', async ({
    page,
  }) => {
    await gotoSystem(page);
    await page.getByRole('tab', { name: '设置 Settings' }).click();

    // Real fixture data renders (not a skeleton): the fixed key set with its
    // actual values from fixtures.ts.
    await expect(page.getByText('approval.enabled')).toBeVisible();
    await expect(page.getByText('audit.retention_days')).toBeVisible();
    const retention = page.getByLabel('设置 audit.retention_days');
    await expect(retention).toHaveValue('90'); // fixture value

    // DOM contract — approval.on_timeout is LOCKED read-only deny: there is NO
    // editable control for it, and it states it is fixed at deny (the UI
    // embodiment of ESCALATE_FOLDS_TO_DENY; on_timeout can never be set allow).
    await expect(page.getByLabel('设置 approval.on_timeout')).toHaveCount(0);
    await expect(page.getByText(/不可配（恒为 deny）/)).toBeVisible();

    // Main write flow: change a normal key → it accumulates into 保存改动 (n)
    // with a summary preview of old → new.
    await expect(
      page.getByRole('button', { name: '保存改动 (0)' }),
    ).toBeDisabled();

    await page.getByLabel('设置 audit.fsync').selectOption('relaxed');

    const saveBtn = page.getByRole('button', { name: '保存改动 (1)' });
    await expect(saveBtn).toBeEnabled();
    const summary = page.getByLabel('改动摘要');
    await expect(summary).toContainText('audit.fsync: always → relaxed');

    // Submit → single POST /settings → optimistic success toast with the
    // advanced policy_rev (live MSW returns an incrementing rev).
    await saveBtn.click();
    await expect(page.getByRole('status')).toContainText('已保存，policy_rev →');
    // After a successful save the dirty count resets to 0 (button disabled).
    await expect(
      page.getByRole('button', { name: '保存改动 (0)' }),
    ).toBeDisabled();
  });

  test('Journey 4 — Settings clamps audit.retention_days to [1, 3650] on the client', async ({
    page,
  }) => {
    await gotoSystem(page);
    await page.getByRole('tab', { name: '设置 Settings' }).click();

    const retention = page.getByLabel('设置 audit.retention_days');
    await retention.fill('99999');
    // Client clamps the out-of-bound retention to the safe upper bound 3650
    // (bounded safe default) — fires on change, no server round-trip needed.
    await expect(retention).toHaveValue('3650');
    await expect(page.getByLabel('改动摘要')).toContainText(
      'audit.retention_days: 90 → 3650',
    );
  });

  test('Journey 5 — Export downloads declarative TOML (read action, no confirm dialog)', async ({
    page,
  }) => {
    await gotoSystem(page);
    await page.getByRole('tab', { name: '导入导出' }).click();

    // Export is a read action → triggers a browser download, no ConfirmDialog
    // interposed. Assert the real download fires and its TOML content is the
    // declarative policy snapshot (never credentials/addresses).
    const downloadPromise = page.waitForEvent('download');
    await page.getByRole('button', { name: '导出 TOML' }).click();
    const download = await downloadPromise;
    expect(download.suggestedFilename()).toBe('postern-policy.toml');

    // No confirm dialog was interposed for the read export.
    await expect(page.getByRole('dialog')).toHaveCount(0);

    // DOM contract — the exported TOML is declarative and carries NO real
    // address / connection string and NO secret material.
    const stream = await download.createReadStream();
    const chunks: Buffer[] = [];
    for await (const c of stream) chunks.push(c as Buffer);
    const text = Buffer.concat(chunks).toString('utf8');
    expect(text).toContain('postern policy export');
    expect(text).not.toMatch(/postgres:\/\//);
    expect(text).not.toMatch(/\b\d{1,3}(\.\d{1,3}){3}\b/); // no IP-shaped address
    expect(text).not.toMatch(/secret_hash/);
  });

  test('Journey 6 — Import validate (dry-run) → diff summary → merge apply (no confirm)', async ({
    page,
  }) => {
    await gotoSystem(page);
    await page.getByRole('tab', { name: '导入导出' }).click();

    // Apply is gated: disabled until a dry-run validates (no partial apply).
    await expect(page.getByRole('button', { name: '应用导入' })).toBeDisabled();

    // Paste TOML and validate → diff summary appears with the real fixture
    // counts (added 2 / changed 1 / deleted 0).
    await page.getByLabel('粘贴 TOML').fill('[role.observer]\n');
    await page.getByRole('button', { name: '校验' }).click();

    const diff = page.getByLabel('diff 摘要');
    await expect(diff).toBeVisible();
    await expect(diff).toContainText('新增 2');
    await expect(diff).toContainText('变更 1');
    await expect(diff).toContainText('删除 0');

    // Merge apply is non-danger → NO ConfirmDialog interposed; reports the
    // applied counts on success.
    const applyBtn = page.getByRole('button', { name: '应用导入' });
    await expect(applyBtn).toBeEnabled();
    await applyBtn.click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByRole('status')).toContainText('已应用 (+2 ~1 -0)');
  });

  test('Journey 7 — Shutdown requires the typed confirm word "shutdown" (danger gate)', async ({
    page,
  }) => {
    await gotoSystem(page);
    await page.getByRole('tab', { name: '关停 Shutdown' }).click();

    // Open the danger ConfirmDialog.
    await page.getByRole('button', { name: '关停 daemon' }).click();
    const dialog = page.getByRole('dialog', { name: '确认：关停 daemon' });
    await expect(dialog).toBeVisible();

    // DOM contract — danger gate: the confirm button is disabled until the EXACT
    // word `shutdown` is typed (anti-misclick on an irreversible action).
    const confirmBtn = dialog.getByRole('button', { name: '关停' });
    await expect(confirmBtn).toBeDisabled();

    await dialog.getByRole('textbox').fill('wrong');
    await expect(confirmBtn).toBeDisabled();

    await dialog.getByRole('textbox').fill('shutdown');
    await expect(confirmBtn).toBeEnabled();

    // Confirm → POST /shutdown → UI states the daemon is gracefully shutting
    // down (success status surfaced, never a silent assumption).
    await confirmBtn.click();
    await expect(page.getByRole('status')).toContainText('daemon 正在优雅关停');
  });
});
