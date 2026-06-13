/**
 * MSW handlers for /v1/*. Returns real-shaped fixtures so the SPA runs and
 * tests pass with no daemon. Collection GETs honor page_no/page_size and
 * return the paged envelope; writes return a fresh policy_rev (WriteAck).
 */

import { http, HttpResponse } from 'msw';
import * as fx from './fixtures';
import type { Page } from '../api/types';

const BASE = '/v1';

function paged<T>(url: URL, items: T[]): Page<T> {
  const page_no = Math.max(1, Number(url.searchParams.get('page_no') ?? '1'));
  const page_size = Math.min(
    200,
    Math.max(1, Number(url.searchParams.get('page_size') ?? '20')),
  );
  const start = (page_no - 1) * page_size;
  return {
    items: items.slice(start, start + page_size),
    page_no,
    page_size,
    total: items.length,
  };
}

let rev = 4187;
const nextRev = () => ({ policy_rev: String(++rev) });

/**
 * E2E-only escape hatch: when the browser worker runs (real DOM), the e2e suite
 * sets `localStorage['__e2e_verify_fail__']` to make the next POST /v1/verify
 * return the one-FAIL report so the security-load-bearing FAIL render is
 * observable in a real browser. Guarded so it is a no-op under the Node test
 * server (vitest), which has no `localStorage` and keeps using server.use().
 */
function e2eVerifyFail(): boolean {
  try {
    return (
      typeof localStorage !== 'undefined' &&
      localStorage.getItem('__e2e_verify_fail__') === '1'
    );
  } catch {
    return false;
  }
}

export const handlers = [
  // ── 健康 / 模式 ──
  http.get(`${BASE}/health`, () => HttpResponse.json(fx.health)),
  http.post(`${BASE}/mode`, async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as { op?: string };
    if (body.op === 'set') {
      return HttpResponse.json({ rows: fx.modeState, ...nextRev() });
    }
    return HttpResponse.json(fx.modeState);
  }),

  // ── 审计 / 拒绝摘要 / 校验 ──
  http.get(`${BASE}/audit`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.auditEvents)),
  ),
  http.get(`${BASE}/denials/summary`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.denialsSummary)),
  ),
  http.post(`${BASE}/verify`, () =>
    HttpResponse.json(e2eVerifyFail() ? fx.verifyReportOneFail : fx.verifyReport),
  ),

  // ── 授权 ──
  http.get(`${BASE}/grants`, () => HttpResponse.json(fx.grantsView)),
  http.post(`${BASE}/grants/temp/elevate`, () => HttpResponse.json(nextRev())),
  http.post(`${BASE}/grants/temp/revoke`, () => HttpResponse.json(nextRev())),

  // ── 主体 / 凭据 / 角色 / 绑定 ──
  http.get(`${BASE}/principals`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.principals)),
  ),
  http.post(`${BASE}/principals`, () => HttpResponse.json(nextRev())),
  http.get(`${BASE}/credentials`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.credentials)),
  ),
  http.post(`${BASE}/credentials`, () => HttpResponse.json(nextRev())),
  http.get(`${BASE}/roles`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.roles)),
  ),
  http.post(`${BASE}/roles`, () => HttpResponse.json(nextRev())),
  http.get(`${BASE}/bindings`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.bindings)),
  ),
  http.post(`${BASE}/bindings`, () => HttpResponse.json(nextRev())),

  // ── 资源 ──
  http.get(`${BASE}/resources`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.resources)),
  ),
  http.post(`${BASE}/resources`, () => HttpResponse.json(nextRev())),
  http.post(`${BASE}/resources/:code/discover`, () =>
    HttpResponse.json({ capabilities: ['observe', 'query'], objects: ['table:orders'] }),
  ),

  // ── 细则 / 条件 / 拒绝备注 ──
  http.get(`${BASE}/constraints`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.constraints)),
  ),
  http.post(`${BASE}/constraints`, () => HttpResponse.json(nextRev())),
  http.get(`${BASE}/conditions`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.conditions)),
  ),
  http.post(`${BASE}/conditions`, () => HttpResponse.json(nextRev())),
  http.get(`${BASE}/deny-notes`, ({ request }) =>
    HttpResponse.json(paged(new URL(request.url), fx.denyNotes)),
  ),
  http.post(`${BASE}/deny-notes`, () => HttpResponse.json(nextRev())),

  // ── 设置 / 审批 / 导入导出 / 关停 ──
  http.get(`${BASE}/settings`, () => HttpResponse.json(fx.settings)),
  http.post(`${BASE}/settings`, () => HttpResponse.json(nextRev())),
  http.post(`${BASE}/approvals`, ({ request }) => {
    return request.url.includes('adjudicate')
      ? HttpResponse.json(nextRev())
      : HttpResponse.json(paged(new URL(request.url), fx.approvals));
  }),
  http.post(`${BASE}/export`, () =>
    HttpResponse.json({ toml: '# postern policy export\npolicy_rev = 4187\n' }),
  ),
  http.post(`${BASE}/import`, () =>
    HttpResponse.json({ added: 2, changed: 1, deleted: 0, applied: false }),
  ),
  http.post(`${BASE}/shutdown`, () => HttpResponse.json(nextRev())),
];
