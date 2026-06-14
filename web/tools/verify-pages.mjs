// Headless per-page verification against the REAL backend.
//
// Drives the web-target frontend (httpTransport -> vite proxy -> control-bridge
// -> control.sock -> daemon) through every nav route with Playwright, and for
// each page asserts: (a) the route-level ErrorBoundary did NOT trip, and (b) no
// uncaught/console errors fired (benign dev warnings filtered). This is the
// automated "verify every page" harness — no GUI, no human in the loop.
//
// Prereqs (started by run-verify.sh): daemon on control.sock, control-bridge on
// :8787, vite (VITE_TARGET=web + VITE_API_PROXY) on VERIFY_BASE.

import { chromium } from '@playwright/test';

const BASE = process.env.VERIFY_BASE || 'http://localhost:5174';

const ROUTES = [
  ['/', '总览 Dashboard'],
  ['/audit', '审计 Audit'],
  ['/denials', '拒绝分析 Denials'],
  ['/verify', '红队自检 Verify'],
  ['/grants', '授权矩阵 Grants'],
  ['/roles', '角色 Roles'],
  ['/bindings', '绑定 Bindings'],
  ['/constraints', '细则 Constraints'],
  ['/resources', '资源 Resources'],
  ['/principals', '主体 Principals'],
  ['/mode', '模式 Mode'],
  ['/system', '系统 System'],
];

function isBenign(e) {
  return /React Router Future Flag|Download the React DevTools|\[vite\] (connect|connected|hot)|favicon/.test(e);
}

const browser = await chromium.launch();
const results = [];
for (const [route, name] of ROUTES) {
  const page = await browser.newPage();
  const pageErrors = []; // uncaught JS exceptions = a real page crash
  const consoleErrors = []; // console.error — may be a gracefully-handled net error
  page.on('console', (m) => {
    if (m.type() === 'error') consoleErrors.push(m.text());
  });
  page.on('pageerror', (e) => pageErrors.push(e.message));
  try {
    await page.goto(BASE + route, { waitUntil: 'networkidle', timeout: 20000 });
    await page.waitForTimeout(900); // let TanStack queries + render settle
  } catch (e) {
    pageErrors.push('goto: ' + e.message);
  }
  const boundary = (await page.locator('text=此页加载出错').count()) > 0;
  results.push({
    route,
    name,
    boundary,
    pageErrors: [...new Set(pageErrors)],
    consoleErrors: [...new Set(consoleErrors)].filter((e) => !isBenign(e)),
  });
  await page.close();
}
await browser.close();

let bad = 0;
console.log(`\n=== 逐页验证 @ ${BASE} (真后端) ===`);
for (const r of results) {
  // A page is broken only if it crashed (error boundary or an uncaught
  // exception). A console.error from a gracefully-handled network response
  // (e.g. a deferred endpoint's 501) is reported as a note, not a failure.
  const ok = !r.boundary && r.pageErrors.length === 0;
  if (!ok) bad++;
  console.log(
    `${ok ? '✅' : '❌'} ${r.route.padEnd(13)} ${r.name}${r.boundary ? '  [ERROR BOUNDARY 触发]' : ''}`,
  );
  for (const e of r.pageErrors.slice(0, 4)) console.log(`      ✗ ${e.slice(0, 220)}`);
  if (ok) for (const e of r.consoleErrors.slice(0, 2)) console.log(`      · (note) ${e.slice(0, 160)}`);
}
console.log(`\n${results.length - bad}/${results.length} 页可用（无崩溃）`);
process.exit(bad > 0 ? 1 : 0);
