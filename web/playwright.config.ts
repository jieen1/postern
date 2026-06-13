import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright config (设计系统 §1 统一 e2e). baseURL points at the Vite dev
 * server, which Playwright starts itself (MSW serves /v1/* so no daemon is
 * needed). The smoke spec opens the app and navigates each page.
 */
export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: 'list',
  use: {
    baseURL: 'http://localhost:5173',
    trace: 'on-first-retry',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'pnpm dev',
    url: 'http://localhost:5173',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
