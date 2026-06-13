import { test, expect } from '@playwright/test';

/**
 * Navigation backbone smoke: open the app, then walk every sidebar link and
 * assert it lands on the right route and that page's *real* heading renders.
 * Headings are taken from the page sources / vitest specs (not guessed): note
 * constraints renders "细则与条件" (no "Constraints" suffix) and principals
 * (the identity page, /principals) renders "主体与凭证 Principals / Credentials".
 * MSW serves /v1/* so this runs with no daemon.
 */
const PAGES: Array<{ link: RegExp; path: string; title: RegExp }> = [
  { link: /审计 Audit/, path: '/audit', title: /^审计 Audit$/ },
  { link: /拒绝分析 Denials/, path: '/denials', title: /^拒绝分析 Denials$/ },
  { link: /红队自检 Verify/, path: '/verify', title: /^红队自检 Verify$/ },
  { link: /授权矩阵 Grants/, path: '/grants', title: /^授权矩阵 Grants$/ },
  { link: /角色 Roles/, path: '/roles', title: /^角色 Roles$/ },
  { link: /绑定 Bindings/, path: '/bindings', title: /^绑定 Bindings$/ },
  { link: /细则与条件 Constraints/, path: '/constraints', title: /^细则与条件$/ },
  { link: /资源 Resources/, path: '/resources', title: /^资源 Resources$/ },
  { link: /主体与凭证 Principals/, path: '/principals', title: /^主体与凭证 Principals \/ Credentials$/ },
  { link: /模式 Mode/, path: '/mode', title: /^模式 Mode$/ },
  { link: /系统 System/, path: '/system', title: /^系统 System$/ },
];

test('app boots and the dashboard renders', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: /^总览 Dashboard$/ })).toBeVisible();
  // Brand + health light present in the top bar. The health light is a titled
  // span whose label is one of the real states (健康/降级/连接中/不可达).
  await expect(page.getByText('postern')).toBeVisible();
  await expect(
    page.locator('header [title]').filter({ hasText: /健康 · rev|降级|连接中|不可达/ }),
  ).toBeVisible();
});

test('every nav link routes to its page and shows the real heading', async ({ page }) => {
  await page.goto('/');
  for (const p of PAGES) {
    await page.getByRole('link', { name: p.link }).click();
    await expect(page).toHaveURL(new RegExp(`${p.path}$`));
    await expect(page.getByRole('heading', { name: p.title })).toBeVisible();
  }
});

test('an unknown path fails closed to the dashboard', async ({ page }) => {
  await page.goto('/this-route-does-not-exist');
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole('heading', { name: /^总览 Dashboard$/ })).toBeVisible();
});
