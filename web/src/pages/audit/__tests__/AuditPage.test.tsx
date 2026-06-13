import { describe, expect, it, beforeEach, vi } from 'vitest';
import { screen, fireEvent, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import * as fx from '../../../mocks/fixtures';
import type { AuditEvent } from '../../../api/types';
import { renderWithQuery } from './test-utils';
import { AuditPage } from '../index';

const BASE = '/v1';

/** Capture the request URL each call so we can assert the wire query params. */
function captureAudit(items: AuditEvent[], onUrl?: (url: URL) => void) {
  return http.get(`${BASE}/audit`, ({ request }) => {
    const url = new URL(request.url);
    onUrl?.(url);
    const page_no = Math.max(1, Number(url.searchParams.get('page_no') ?? '1'));
    const page_size = Math.min(200, Math.max(1, Number(url.searchParams.get('page_size') ?? '20')));
    const start = (page_no - 1) * page_size;
    return HttpResponse.json({
      items: items.slice(start, start + page_size),
      page_no,
      page_size,
      total: items.length,
    });
  });
}

describe('AuditPage — 渲染与正常路径', () => {
  it('renders the title, primary export action, and filter bar', async () => {
    renderWithQuery(<AuditPage />);
    expect(screen.getByRole('heading', { name: '审计 Audit' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /导出 JSONL/ })).toBeInTheDocument();
    expect(screen.getByRole('form', { name: '审计筛选条' })).toBeInTheDocument();
    // The forced-pagination footer is present.
    await waitFor(() => expect(screen.getByLabelText('每页条数')).toBeInTheDocument());
  });

  it('lists events reverse-chron and pairs intent/outcome by request_id', async () => {
    renderWithQuery(<AuditPage />);
    // fixtures: 2 request events for req-aa01 (intent+outcome) collapse to ONE
    // row; the deny (req-aa02) is a separate row ⇒ 2 list items total.
    await waitFor(() =>
      expect(screen.getAllByRole('listitem').length).toBeGreaterThan(0),
    );
    const items = screen.getAllByRole('listitem');
    expect(items).toHaveLength(2);
    // total reflected in the filter bar count and the footer.
    expect(screen.getByText(/匹配 3 条/)).toBeInTheDocument();
    expect(screen.getByText(/共 3 条 · 第 1\/1 页/)).toBeInTheDocument();
  });

  it('expands a paired request row to reveal outcome-only fields (response_digest + duration)', async () => {
    renderWithQuery(<AuditPage />);
    await waitFor(() => expect(screen.getAllByRole('listitem').length).toBe(2));
    // The first row is the allow request (intent+outcome paired).
    const rows = screen.getAllByRole('listitem');
    const toggle = within(rows[0]!).getAllByRole('button')[0]!;
    fireEvent.click(toggle);
    // Outcome-only fields are only visible once expanded (A-3).
    expect(screen.getByText('response_digest')).toBeInTheDocument();
    expect(screen.getByText(fx.auditEvents[0]!.response_digest!)).toBeInTheDocument();
    expect(screen.getByText('duration_ms')).toBeInTheDocument();
  });
});

describe('AuditPage — 筛选 (§4.1)', () => {
  it('applying filters sends since/principal/kind/decision and resets to page 1', async () => {
    const urls: URL[] = [];
    server.use(captureAudit(fx.auditEvents, (u) => urls.push(u)));
    renderWithQuery(<AuditPage />);
    await waitFor(() => expect(urls.length).toBeGreaterThan(0));

    fireEvent.change(screen.getByLabelText(/principal/), { target: { value: 'agent3' } });
    fireEvent.change(screen.getByLabelText(/kind/), { target: { value: 'request' } });
    // decision segmented single-select — pick deny.
    fireEvent.click(screen.getByLabelText('deny'));
    fireEvent.click(screen.getByRole('button', { name: /应用/ }));

    await waitFor(() => {
      const last = urls[urls.length - 1]!;
      expect(last.searchParams.get('principal')).toBe('agent3');
      expect(last.searchParams.get('kind')).toBe('request');
      expect(last.searchParams.get('decision')).toBe('deny');
      expect(last.searchParams.get('page_no')).toBe('1');
    });
  });

  it('decision filter keeps escalate_denied DISTINCT from deny (not folded in the query)', async () => {
    const urls: URL[] = [];
    server.use(captureAudit(fx.auditEvents, (u) => urls.push(u)));
    renderWithQuery(<AuditPage />);
    await waitFor(() => expect(urls.length).toBeGreaterThan(0));

    fireEvent.click(screen.getByLabelText('escalate_denied'));
    fireEvent.click(screen.getByRole('button', { name: /应用/ }));

    await waitFor(() =>
      expect(urls[urls.length - 1]!.searchParams.get('decision')).toBe('escalate_denied'),
    );
  });

  it('clearing filters drops all filter params and returns to page 1', async () => {
    const urls: URL[] = [];
    server.use(captureAudit(fx.auditEvents, (u) => urls.push(u)));
    renderWithQuery(<AuditPage />);
    await waitFor(() => expect(urls.length).toBeGreaterThan(0));

    fireEvent.change(screen.getByLabelText(/principal/), { target: { value: 'agent3' } });
    fireEvent.click(screen.getByRole('button', { name: /应用/ }));
    await waitFor(() =>
      expect(urls[urls.length - 1]!.searchParams.get('principal')).toBe('agent3'),
    );

    fireEvent.click(screen.getByRole('button', { name: /清空/ }));
    await waitFor(() => {
      const last = urls[urls.length - 1]!;
      expect(last.searchParams.get('principal')).toBeNull();
      expect(last.searchParams.get('page_no')).toBe('1');
    });
  });
});

describe('AuditPage — 分页契约 (DB_PAGINATION_MANDATORY)', () => {
  // 45 synthetic non-request events so each is its own list item (no pairing).
  const many: AuditEvent[] = Array.from({ length: 45 }, (_, i) => ({
    v: 1,
    kind: 'lifecycle',
    entry: 'mcp',
    origin: 'unix:uid=1000',
    principal: `p${i}`,
    resource: 'db-main',
    capability: null,
    objects: [],
    decision: 'allow',
    stage: null,
    reason: '',
    policy_rev: '4187',
    id: `730000000000000${String(8000 + i)}`,
    ts: `2026-06-14T03:${String(i % 60).padStart(2, '0')}:00Z`,
  }));

  it('default page size is 20 and footer shows total / page count', async () => {
    const urls: URL[] = [];
    server.use(captureAudit(many, (u) => urls.push(u)));
    renderWithQuery(<AuditPage />);
    await waitFor(() =>
      expect(urls[urls.length - 1]!.searchParams.get('page_size')).toBe('20'),
    );
    // 45 total at size 20 ⇒ 3 pages.
    expect(await screen.findByText(/共 45 条 · 第 1\/3 页/)).toBeInTheDocument();
    expect(screen.getAllByRole('listitem')).toHaveLength(20);
  });

  it('next page requests page_no=2 from the SERVER (no client slicing of the full set)', async () => {
    const urls: URL[] = [];
    server.use(captureAudit(many, (u) => urls.push(u)));
    renderWithQuery(<AuditPage />);
    await screen.findByText(/共 45 条 · 第/);

    fireEvent.click(screen.getByRole('button', { name: '下一页' }));
    await waitFor(() =>
      expect(urls[urls.length - 1]!.searchParams.get('page_no')).toBe('2'),
    );
    // 45 total, page 2 of 3 ⇒ still 20 rows.
    await waitFor(() => expect(screen.getAllByRole('listitem')).toHaveLength(20));
  });

  it('selecting a page size clamps to <=200 and re-requests from the server', async () => {
    const urls: URL[] = [];
    server.use(captureAudit(many, (u) => urls.push(u)));
    renderWithQuery(<AuditPage />);
    await screen.findByText(/共 45 条 · 第/);

    fireEvent.change(screen.getByLabelText('每页条数'), { target: { value: '200' } });
    await waitFor(() =>
      expect(urls[urls.length - 1]!.searchParams.get('page_size')).toBe('200'),
    );
    // size 200 ≥ 45 ⇒ all on one page.
    await waitFor(() => expect(screen.getAllByRole('listitem')).toHaveLength(45));
  });

  it('the pagination footer is present even on an empty result (forced pagination)', async () => {
    server.use(captureAudit([]));
    renderWithQuery(<AuditPage />);
    expect(await screen.findByText(/共 0 条 · 第 1\/1 页/)).toBeInTheDocument();
    expect(screen.getByLabelText('每页条数')).toBeInTheDocument();
  });
});

describe('AuditPage — 三态 fail-closed (§3.2)', () => {
  it('shows a loading skeleton (no fake data) before data arrives', () => {
    // Never-resolving handler keeps the query pending.
    server.use(http.get(`${BASE}/audit`, () => new Promise(() => {})));
    renderWithQuery(<AuditPage />);
    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.queryAllByRole('listitem')).toHaveLength(0);
  });

  it('renders a fail-closed ERROR state on 500 — no rows, not a deceptive empty list', async () => {
    server.use(
      http.get(`${BASE}/audit`, () =>
        HttpResponse.json({ error: { code: 'internal', message: 'boom' } }, { status: 500 }),
      ),
    );
    renderWithQuery(<AuditPage />);
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('审计查询失败');
    // Distinct from empty: the empty-state copy must NOT appear.
    expect(screen.queryByText('当前筛选无匹配事件')).not.toBeInTheDocument();
    expect(screen.queryAllByRole('listitem')).toHaveLength(0);
    // Retry control is offered.
    expect(within(alert).getByRole('button', { name: '重试' })).toBeInTheDocument();
  });

  it('401/403 surfaces as an error state and leaks NO event content', async () => {
    server.use(
      http.get(`${BASE}/audit`, () =>
        HttpResponse.json({ error: { code: 'forbidden', message: '无权访问审计' } }, { status: 403 }),
      ),
    );
    renderWithQuery(<AuditPage />);
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    // No event field (e.g. a fixture resource code) leaks into an error.
    expect(screen.queryByText('agent-order-bot')).not.toBeInTheDocument();
    expect(screen.queryAllByRole('listitem')).toHaveLength(0);
  });

  it('an empty result is EMPTY (not error): shows the empty copy + a clear-filters guide', async () => {
    server.use(captureAudit([]));
    renderWithQuery(<AuditPage />);
    expect(await screen.findByText('当前筛选无匹配事件')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '清空筛选' })).toBeInTheDocument();
  });

  it('retry after an error re-queries and recovers to data', async () => {
    let fail = true;
    server.use(
      http.get(`${BASE}/audit`, ({ request }) => {
        if (fail) {
          fail = false;
          return HttpResponse.json({ error: { code: 'internal', message: 'boom' } }, { status: 500 });
        }
        const url = new URL(request.url);
        const page_no = Math.max(1, Number(url.searchParams.get('page_no') ?? '1'));
        const page_size = Math.min(200, Math.max(1, Number(url.searchParams.get('page_size') ?? '20')));
        const start = (page_no - 1) * page_size;
        return HttpResponse.json({
          items: fx.auditEvents.slice(start, start + page_size),
          page_no,
          page_size,
          total: fx.auditEvents.length,
        });
      }),
    );
    renderWithQuery(<AuditPage />);
    const alert = await screen.findByRole('alert');
    fireEvent.click(within(alert).getByRole('button', { name: '重试' }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(screen.getAllByRole('listitem')).toHaveLength(2);
  });
});

describe('AuditPage — 契约: 雪花 id 不丢精度 / deny 不泄存在性 / 两阶段诚实', () => {
  it('renders the snowflake id as a STRING (truncated) and copies the FULL value, never a Number', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    renderWithQuery(<AuditPage />);
    await waitFor(() => expect(screen.getAllByRole('listitem').length).toBe(2));

    const rows = screen.getAllByRole('listitem');
    fireEvent.click(within(rows[0]!).getAllByRole('button')[0]!); // expand
    const fullId = fx.auditEvents[0]!.id!;
    // The id is well beyond 2^53 — confirm it would lose precision as a Number.
    expect(String(Number(fullId))).not.toBe(fullId);
    // The truncated, mono id is shown with the full value available on hover.
    const idEl = screen.getByTitle(fullId);
    expect(idEl).toBeInTheDocument();
    expect(idEl.textContent).not.toBe(fullId); // middle-truncated display
    // Copy yields the full string id — precision intact.
    fireEvent.click(screen.getByRole('button', { name: '复制完整 id' }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(fullId));
  });

  it('a deny row shows stage + verbatim reason but only an anonymized resource code (no real address / no scope leak)', async () => {
    renderWithQuery(<AuditPage />);
    await waitFor(() => expect(screen.getAllByRole('listitem').length).toBe(2));
    // The deny is the 2nd row (req-aa02).
    const denyRow = screen.getAllByRole('listitem')[1]!;
    // DecisionBadge shows deny; expand the row to reveal the verbatim reason.
    expect(within(denyRow).getByText('deny')).toBeInTheDocument();
    fireEvent.click(within(denyRow).getAllByRole('button')[0]!);
    expect(within(denyRow).getByText(fx.auditEvents[2]!.reason)).toBeInTheDocument();
    // Resource is the codename only — never a host/address.
    expect(within(denyRow).getAllByText('db-main').length).toBeGreaterThan(0);
    expect(within(denyRow).queryByText(/\/\//)).not.toBeInTheDocument(); // no URL/address
  });

  it('an intent-only write event is honestly marked "intent only" (no fabricated outcome)', async () => {
    const intentOnly: AuditEvent = {
      v: 1,
      kind: 'request',
      entry: 'mcp',
      origin: 'unix:uid=1000',
      principal: 'agent-x',
      resource: 'db-main',
      capability: 'mutate',
      objects: ['table:orders'],
      decision: 'deny',
      stage: 'rbac',
      reason: 'denied before exec',
      policy_rev: '4187',
      id: '7300000000000009001',
      request_id: 'req-orphan',
      ts: '2026-06-14T03:30:00Z',
      intent_digest: 'sha256:aa…bb',
    };
    server.use(captureAudit([intentOnly]));
    renderWithQuery(<AuditPage />);
    await waitFor(() => expect(screen.getAllByRole('listitem').length).toBe(1));
    expect(screen.getByText('intent only')).toBeInTheDocument();
    // No outcome field fabricated.
    fireEvent.click(screen.getAllByRole('listitem')[0]!.querySelector('button')!);
    expect(screen.queryByText('response_digest')).not.toBeInTheDocument();
    expect(screen.queryByText('duration_ms')).not.toBeInTheDocument();
  });

  it('a None principal / None capability render as "—" without exposing absence of out-of-scope facts', async () => {
    const sparse: AuditEvent = {
      v: 1,
      kind: 'request',
      entry: 'mcp',
      origin: 'unix:uid=1000',
      principal: null, // pre-step[1] deny: no principal yet
      resource: 'db-main',
      capability: null, // classify deny: no capability
      objects: [],
      decision: 'deny',
      stage: 'auth',
      reason: 'untrusted origin',
      policy_rev: '4187',
      id: '7300000000000009002',
      request_id: 'req-sparse',
      ts: '2026-06-14T03:31:00Z',
    };
    server.use(captureAudit([sparse]));
    renderWithQuery(<AuditPage />);
    await waitFor(() => expect(screen.getAllByRole('listitem').length).toBe(1));
    const row = screen.getAllByRole('listitem')[0]!;
    expect(within(row).getByText('—')).toBeInTheDocument();
  });
});

describe('AuditPage — 导出 JSONL (§4.3，非写操作)', () => {
  beforeEach(() => {
    // jsdom lacks URL.createObjectURL / Blob#text; stub the download plumbing.
    Object.assign(URL, {
      createObjectURL: vi.fn(() => 'blob:mock'),
      revokeObjectURL: vi.fn(),
    });
  });

  it('exports the current filtered window as JSONL with snowflake ids kept as strings', async () => {
    // jsdom Blob lacks #text(); capture the JSONL the page passes to Blob().
    // Filter to the page's ndjson blob (MSW also builds Blobs internally).
    const parts: string[] = [];
    const RealBlob = globalThis.Blob;
    const blobSpy = vi
      .spyOn(globalThis, 'Blob')
      .mockImplementation((blobParts?: BlobPart[], opts?: BlobPropertyBag) => {
        if (blobParts && opts?.type === 'application/x-ndjson') {
          parts.push(...(blobParts as string[]));
        }
        return new RealBlob(blobParts, opts);
      });
    const clickSpy = vi
      .spyOn(HTMLAnchorElement.prototype, 'click')
      .mockImplementation(() => {});

    renderWithQuery(<AuditPage />);
    await waitFor(() => expect(screen.getAllByRole('listitem').length).toBe(2));

    fireEvent.click(screen.getByRole('button', { name: /导出 JSONL/ }));
    fireEvent.click(await screen.findByRole('menuitem'));

    await waitFor(() => expect(parts.length).toBe(1));
    const lines = parts[0]!.split('\n').filter(Boolean);
    expect(lines).toHaveLength(fx.auditEvents.length);
    // Each line is valid JSON and the id is a quoted STRING (precision-safe).
    const first = JSON.parse(lines[0]!) as { id: string };
    expect(typeof first.id).toBe('string');
    expect(lines[0]).toContain(`"${fx.auditEvents[0]!.id!}"`);
    blobSpy.mockRestore();
    clickSpy.mockRestore();
  });

  it('the export action is disabled when the query is in an error state', async () => {
    server.use(
      http.get(`${BASE}/audit`, () =>
        HttpResponse.json({ error: { code: 'internal', message: 'boom' } }, { status: 500 }),
      ),
    );
    renderWithQuery(<AuditPage />);
    await screen.findByRole('alert');
    expect(screen.getByRole('button', { name: /导出 JSONL/ })).toBeDisabled();
  });
});
