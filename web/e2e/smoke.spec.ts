import { test, expect } from '@playwright/test';

/**
 * Smoke: open the app, then navigate to every page via the sidebar and assert
 * its title renders. MSW serves /v1/* so this runs with no daemon.
 */
const PAGES: Array<{ link: RegExp; title: RegExp }> = [
  { link: /审计 Audit/, title: /审计 Audit/ },
  { link: /拒绝分析 Denials/, title: /拒绝分析 Denials/ },
  { link: /红队自检 Verify/, title: /红队自检 Verify/ },
  { link: /授权矩阵 Grants/, title: /授权矩阵 Grants/ },
  { link: /角色 Roles/, title: /角色 Roles/ },
  { link: /绑定 Bindings/, title: /绑定 Bindings/ },
  { link: /细则与条件 Constraints/, title: /细则与条件 Constraints/ },
  { link: /资源 Resources/, title: /资源 Resources/ },
  { link: /主体与凭证 Principals/, title: /主体与凭证 Principals/ },
  { link: /模式 Mode/, title: /模式 Mode/ },
  { link: /系统 System/, title: /系统 System/ },
];

test('app boots and the dashboard renders', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: /总览 Dashboard/ })).toBeVisible();
  // Brand + health light present in the top bar.
  await expect(page.getByText('postern')).toBeVisible();
});

test('every page is navigable and shows its title', async ({ page }) => {
  await page.goto('/');
  for (const p of PAGES) {
    await page.getByRole('link', { name: p.link }).click();
    await expect(page.getByRole('heading', { name: p.title })).toBeVisible();
  }
});
