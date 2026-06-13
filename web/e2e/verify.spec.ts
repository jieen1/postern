import { test, expect, type Page } from '@playwright/test';

/**
 * 红队自检 Verify — real-browser e2e (04-verify.md).
 *
 * Runs against the shared Vite+MSW dev server on :5173. POST /v1/verify returns
 * the all-pass `verifyReport` fixture (nine PASS probes) by default, OR the
 * one-FAIL `verifyReportOneFail` fixture when the e2e sets the
 * `__e2e_verify_fail__` localStorage flag (see src/mocks/handlers.ts) — letting
 * the in-browser suite exercise the security-load-bearing FAIL render (verdict
 * flip + auto-expanded red row + verbatim existence-leak gap_note) in a REAL
 * browser, not just via server.use() in the vitest unit suite. GET /v1/health
 * carries policy_rev '4187'. Error / 403 / incomplete-report / inconsistent-
 * report fail-closed paths remain vitest server.use() cases, not re-created here.
 */

/** Probed codename that the FAIL gap_note legitimately quotes verbatim. */
const PROBED_CODENAME = 'nonexistent-probe-target';
/** The exact verbatim gap_note the one-FAIL fixture carries (原样转述, §3.1). */
const FAIL_GAP_NOTE = `拒绝响应泄露了资源 '${PROBED_CODENAME}' 的存在性(your_grants 含被探测代号)`;

/**
 * Arm the e2e FAIL toggle so the NEXT POST /v1/verify returns the one-FAIL
 * report. Must run before the page loads (the flag is read inside the MSW
 * handler at request time). Call before gotoVerify.
 */
async function armVerifyFail(page: Page) {
  await page.addInitScript(() => {
    window.localStorage.setItem('__e2e_verify_fail__', '1');
  });
}

/** Open the page via the sidebar and wait for the idle (never-run) empty state. */
async function gotoVerify(page: Page) {
  await page.goto('/');
  await page.getByRole('link', { name: /红队自检 Verify/ }).click();
  await expect(page.getByRole('heading', { name: /红队自检 Verify/ })).toBeVisible();
  await expect(page.getByText('尚未运行红队自检')).toBeVisible();
}

/** Open the danger dialog, tick the ack checkbox, click 运行. */
async function runVerifyViaDialog(page: Page) {
  await page.getByRole('button', { name: '运行自检' }).first().click();
  const dialog = page.getByRole('dialog', { name: '运行红队自检？' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: '运行' }).click();
}

test('idle: never-run empty state + idle verdict, no fake nine, no fake green', async ({
  page,
}) => {
  await gotoVerify(page);

  // The mono subheading proves the page (not a skeleton) rendered.
  await expect(
    page.getByText('POST /v1/verify · 对当前策略快照逐条发起应被拒探针'),
  ).toBeVisible();

  // Overall verdict is the idle "尚未运行" banner — NOT green, NOT a fabricated FAIL.
  await expect(page.getByRole('status', { name: /整体判定：尚未运行/ })).toBeVisible();
  await expect(page.getByText('ALL PASS')).toHaveCount(0);
  await expect(page.getByText('VERIFY FAILED')).toHaveCount(0);
  // No probe rows render before a run — the nine are never faked.
  await expect(page.getByText('scope_out_mutate')).toHaveCount(0);

  // Snapshot policy_rev from the health fixture is shown as a STRING.
  await expect(page.getByText('policy_rev')).toBeVisible();
});

test('confirm dialog: 运行 gated on the ack checkbox; summary previews the action (no policy diff) + current policy_rev', async ({
  page,
}) => {
  await gotoVerify(page);

  await page.getByRole('button', { name: '运行自检' }).first().click();
  const dialog = page.getByRole('dialog', { name: '运行红队自检？' });
  await expect(dialog).toBeVisible();

  // Action summary (not a policy diff): probes + "不改任何策略".
  await expect(dialog.getByText(/自发 9 条应被拒探针/)).toBeVisible();
  await expect(dialog.getByText('不改任何策略（无 policy_rev 前进）')).toBeVisible();
  // The current snapshot policy_rev '4187' (from the health fixture) is previewed.
  await expect(dialog.getByText('4187')).toBeVisible();

  // 运行 stays disabled until the explicit acknowledgment is ticked.
  const runBtn = dialog.getByRole('button', { name: '运行' });
  await expect(runBtn).toBeDisabled();
  await dialog.getByRole('checkbox').check();
  await expect(runBtn).toBeEnabled();
});

test('cancelling the dialog does not trigger a run (stays in never-run state)', async ({
  page,
}) => {
  await gotoVerify(page);

  await page.getByRole('button', { name: '运行自检' }).first().click();
  const dialog = page.getByRole('dialog', { name: '运行红队自检？' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: '取消' }).click();
  await expect(dialog).toBeHidden();

  // No verdict change, no probe rows leaked.
  await expect(page.getByText('尚未运行红队自检')).toBeVisible();
  await expect(page.getByText('ALL PASS')).toHaveCount(0);
  await expect(page.getByText('scope_out_mutate')).toHaveCount(0);
});

test('ALL PASS happy path: complete all-pass report → green verdict (9/9), nine PASS rows in catalog order', async ({
  page,
}) => {
  await gotoVerify(page);
  await runVerifyViaDialog(page);

  // Verdict flips to the green ALL PASS banner with the real (9/9) count.
  await expect(page.getByRole('status', { name: /整体判定：ALL PASS/ })).toBeVisible();
  await expect(page.getByText('ALL PASS')).toBeVisible();
  await expect(page.getByText('(9/9)')).toBeVisible();

  // Nine real PASS rows render (the fixture's nine probes), in catalog order.
  await expect(page.getByText('PASS', { exact: true })).toHaveCount(9);
  await expect(page.getByText('FAIL', { exact: true })).toHaveCount(0);
  // First and last fixed probe names render verbatim (deterministic items order).
  await expect(page.getByText('scope_out_mutate')).toBeVisible();
  await expect(page.getByText('redaction_probe')).toBeVisible();

  // Audit deep-link affordance becomes available after a complete result.
  await expect(
    page.getByRole('button', { name: /查看本次探针在审计中的留痕/ }),
  ).toBeVisible();
});

test('expand a PASS row → static catalog (intent / defense stage / PASS criterion) revealed; collapsed by default', async ({
  page,
}) => {
  await gotoVerify(page);
  await runVerifyViaDialog(page);
  await expect(page.getByText('ALL PASS')).toBeVisible();

  // Probe ① row toggle — PASS rows are collapsed by default (aria-expanded=false),
  // so its intent prose is hidden until clicked.
  const toggle = page
    .getByRole('button')
    .filter({ hasText: 'scope_out_mutate' });
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByText(/对其 Scope 外资源发起 mutate/)).toHaveCount(0);

  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  // Static catalog now visible: intent + PASS criterion (descriptive, never the verdict).
  await expect(page.getByText(/对其 Scope 外资源发起 mutate/)).toBeVisible();
  await expect(page.getByText(/应在 rbac 阶因授权矩阵缺格被拒/)).toBeVisible();
});

test('cross-page flow: 查看留痕 deep-links to audit with verify principal + since window (no policy_rev write)', async ({
  page,
}) => {
  await gotoVerify(page);
  await runVerifyViaDialog(page);
  await expect(page.getByText('ALL PASS')).toBeVisible();

  await page.getByRole('button', { name: /查看本次探针在审计中的留痕/ }).click();

  // Landed on the audit page, carrying the temp verify principal + the run's since.
  await expect(page.getByRole('heading', { name: /审计 Audit/ })).toBeVisible();
  await expect(page).toHaveURL(/\/audit\?/);
  const url = new URL(page.url());
  expect(url.searchParams.get('principal')).toBe('verify-probe');
  expect(url.searchParams.get('since')).toBeTruthy();
  // The action changes no policy: no policy_rev / version param is carried over.
  expect(url.searchParams.get('policy_rev')).toBeNull();
  expect(url.searchParams.get('version')).toBeNull();
});

test('DOM contract: snowflake policy_rev kept as a precise STRING (full value in title, no Number coercion)', async ({
  page,
}) => {
  await gotoVerify(page);

  // policy_rev '4187' renders inside a span whose title carries the EXACT id —
  // SnowflakeId never round-trips through Number, so precision is preserved.
  const idCell = page.locator('span[title="4187"]');
  await expect(idCell).toBeVisible();
  await expect(idCell).toHaveAttribute('title', '4187');
});

test('all-pass report renders nine PASS rows and NO gap_note panel (no leak on a real green result)', async ({
  page,
}) => {
  await gotoVerify(page);
  await runVerifyViaDialog(page);
  await expect(page.getByText('ALL PASS')).toBeVisible();

  // Anchor the "no leak" claim to an actually-rendered green result (nine PASS
  // rows present), NOT to an empty DOM. Every probe PASS ⇒ every gap_note=null,
  // so the red gap_note panel (.text-deny with bg-deny/10) is rendered nowhere.
  await expect(page.getByText('PASS', { exact: true })).toHaveCount(9);
  await expect(page.locator('.bg-deny\\/10')).toHaveCount(0);

  // Expand every probe row so any leaked material in collapsed detail is in the DOM.
  for (const toggle of await page.getByRole('button', { name: /scope_out_mutate|disguised_write|session_tamper|multi_statement|default_deny_unknown_resource|credential_zero_touch|origin_not_trusted|untrusted_origin_auth_stage|redaction_probe/ }).all()) {
    if ((await toggle.getAttribute('aria-expanded')) === 'false') await toggle.click();
  }

  const body = page.locator('body');
  // On an all-pass report the FAIL existence-leak phrase never renders.
  await expect(body).not.toContainText('泄露了资源');
  // No real connection string / plaintext secret material echoed anywhere.
  await expect(body).not.toContainText('postgres://');
  // No raw IPv4-shaped host leaked in the rendered text.
  await expect(body).not.toHaveText(/\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/);
});

test('FAIL existence-leak path: verdict flips to VERIFY FAILED (8/9), FAIL row auto-expands, gap_note renders VERBATIM and is the ONLY place the probed codename appears', async ({
  page,
}) => {
  // Arm the one-FAIL report BEFORE the page loads (the MSW handler reads the
  // flag at request time). Probe ⑤ default_deny_unknown_resource fails with a
  // verbatim existence-leak gap_note (04-verify.md §4.1 / §6.2-E6).
  await armVerifyFail(page);
  await gotoVerify(page);
  await runVerifyViaDialog(page);

  // Verdict flips to the red VERIFY FAILED banner with the REAL (8/9) count —
  // never a fabricated green, never a faked nine-PASS.
  await expect(page.getByRole('status', { name: /整体判定：VERIFY FAILED/ })).toBeVisible();
  await expect(page.getByText('VERIFY FAILED')).toBeVisible();
  await expect(page.getByText('(8/9)')).toBeVisible();
  await expect(page.getByText('ALL PASS')).toHaveCount(0);

  // Exactly one FAIL row, eight PASS rows.
  await expect(page.getByText('FAIL', { exact: true })).toHaveCount(1);
  await expect(page.getByText('PASS', { exact: true })).toHaveCount(8);

  // The FAIL row auto-expands (PASS rows stay collapsed) so the缺口 is visible
  // without a click — its toggle reports aria-expanded=true.
  const failToggle = page
    .getByRole('button')
    .filter({ hasText: 'default_deny_unknown_resource' });
  await expect(failToggle).toHaveAttribute('aria-expanded', 'true');
  // A collapsed PASS sibling proves the auto-expand is FAIL-specific, not global.
  await expect(
    page.getByRole('button').filter({ hasText: 'scope_out_mutate' }),
  ).toHaveAttribute('aria-expanded', 'false');

  // The gap_note renders VERBATIM (原样转述 — not reworded / summarized) inside
  // the red deny panel.
  const gapPanel = page.locator('.bg-deny\\/10', { hasText: FAIL_GAP_NOTE });
  await expect(gapPanel).toBeVisible();
  await expect(gapPanel).toHaveText(FAIL_GAP_NOTE);

  // Existence-leak redaction invariant: the probed codename appears in the page
  // ONLY inside that verbatim gap_note — it must not leak into any other rendered
  // text (banner, row header, static catalog prose, audit affordance, etc.). The
  // static catalog deliberately phrases its PASS criterion as "your_grants 不含被
  // 探测代号" WITHOUT echoing the codename, so the only legitimate occurrence is
  // the backend's verbatim gap_note.
  const codenameHits = page.getByText(PROBED_CODENAME);
  await expect(codenameHits).toHaveCount(1);
  await expect(codenameHits).toHaveText(FAIL_GAP_NOTE);

  // No real connection string / plaintext secret / raw IPv4 host leaks elsewhere
  // on the FAIL render either.
  const body = page.locator('body');
  await expect(body).not.toContainText('postgres://');
  await expect(body).not.toHaveText(/\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/);
});
