/**
 * Tests for docs/08 细则/条件/拒绝指引 page — real assertions over normal AND
 * abnormal expectations (docs §六): three fail-closed states, forced pagination
 * clamp, snowflake-id-as-string, verbatim operator_note, the unified write flow
 * (create → policy_rev advance), optimistic-lock 409, delete = scope-widening
 * danger confirm, condition scope-widen confirm, and the adapter kind matrix.
 *
 * MSW per-test overrides (server.use) synthesize empty/error/409 — the shared
 * src/mocks/handlers are NOT modified.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import * as fx from '../../../mocks/fixtures';
import ConstraintsPage from '../index';

const BASE = '/v1';

function renderPage() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={qc}>
      <ConstraintsPage />
    </QueryClientProvider>,
  );
}

afterEach(cleanup);

describe('ConstraintsPage — 正常渲染与段切换', () => {
  it('renders the title and the constraints table with the seeded row', async () => {
    renderPage();
    expect(
      screen.getByRole('heading', { name: '细则与条件' }),
    ).toBeInTheDocument();
    // The seeded constraint row (db-main / query / table_allow) shows up.
    expect(await screen.findByText('table_allow')).toBeInTheDocument();
    // Resource is rendered as a CODE badge, never a real address.
    expect(screen.getAllByText('db-main').length).toBeGreaterThan(0);
  });

  it('switches segments: conditions table shows the predicate column/row', async () => {
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('tab', { name: /条件 Conditions/ }));
    // condition fixture has predicate rate_limit and a NULL capability → "*".
    expect(await screen.findByText('rate_limit')).toBeInTheDocument();
    // The condition table header includes predicate, not kind.
    expect(screen.getByText('predicate')).toBeInTheDocument();
    expect(screen.queryByText('kind')).not.toBeInTheDocument();
  });

  it('deny-notes segment shows the note VERBATIM (== operator_note, 公理六)', async () => {
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('tab', { name: /拒绝指引 Deny-notes/ }));
    const note = fx.denyNotes[0]!.note;
    // Verbatim, unmodified — exactly the operator_note text.
    expect(await screen.findByText(note)).toBeInTheDocument();
    // The column is labelled as the operator_note source.
    expect(
      screen.getByText(/越权时 Agent 收到的 operator_note/),
    ).toBeInTheDocument();
  });
});

describe('ConstraintsPage — 三态 fail-closed', () => {
  it('error: ErrorState with the verbatim code, NO fake rows leaked', async () => {
    server.use(
      http.get(`${BASE}/constraints`, () =>
        HttpResponse.json(
          { error: { code: 'daemon_unreachable', message: 'control.sock down' } },
          { status: 503 },
        ),
      ),
    );
    renderPage();
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('control.sock down');
    // fail-closed: the would-be seeded row is NOT rendered from stale cache.
    expect(screen.queryByText('table_allow')).not.toBeInTheDocument();
  });

  it('error also disables the primary create button (fail-closed write)', async () => {
    server.use(
      http.get(`${BASE}/constraints`, () =>
        HttpResponse.json(
          { error: { code: 'forbidden', message: '403' } },
          { status: 403 },
        ),
      ),
    );
    renderPage();
    await screen.findByRole('alert');
    expect(screen.getByRole('button', { name: /新建细则/ })).toBeDisabled();
  });

  it('empty: EmptyState with the segment-specific fact text', async () => {
    server.use(
      http.get(`${BASE}/constraints`, ({ request }) => {
        const url = new URL(request.url);
        return HttpResponse.json({
          items: [],
          page_no: Number(url.searchParams.get('page_no') ?? 1),
          page_size: Number(url.searchParams.get('page_size') ?? 20),
          total: 0,
        });
      }),
    );
    renderPage();
    expect(await screen.findByText('该范围尚无对象细则')).toBeInTheDocument();
    // The empty fact spells out what "no constraint" means.
    expect(
      screen.getByText(/未挂细则的动词其作用面由角色与 tier 决定/),
    ).toBeInTheDocument();
  });

  it('deny-notes empty fact: no operator_note field when unset (skip_serializing)', async () => {
    server.use(
      http.get(`${BASE}/deny-notes`, () =>
        HttpResponse.json({ items: [], page_no: 1, page_size: 20, total: 0 }),
      ),
    );
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('tab', { name: /拒绝指引 Deny-notes/ }));
    expect(await screen.findByText('尚无拒绝指引')).toBeInTheDocument();
    expect(
      screen.getByText(/无预写则越权响应不含 operator_note 字段/),
    ).toBeInTheDocument();
  });
});

describe('ConstraintsPage — 契约：分页钳制 & 雪花 id 不丢精度', () => {
  it('GET /constraints is always paginated with page_no/page_size (default 20)', async () => {
    const seen: string[] = [];
    server.use(
      http.get(`${BASE}/constraints`, ({ request }) => {
        const url = new URL(request.url);
        seen.push(url.search);
        return HttpResponse.json({
          items: fx.constraints,
          page_no: 1,
          page_size: 20,
          total: fx.constraints.length,
        });
      }),
    );
    renderPage();
    await screen.findByText('table_allow');
    expect(seen.length).toBeGreaterThan(0);
    expect(seen[0]).toContain('page_no=1');
    expect(seen[0]).toContain('page_size=20');
  });

  it('selecting page size 200 is the clamp ceiling and re-requests size 200', async () => {
    const seen: string[] = [];
    server.use(
      http.get(`${BASE}/constraints`, ({ request }) => {
        seen.push(new URL(request.url).search);
        return HttpResponse.json({
          items: fx.constraints,
          page_no: 1,
          page_size: 20,
          total: 60,
        });
      }),
    );
    renderPage();
    await screen.findByText('table_allow');
    // The page-size selector is the combobox whose options are the legal sizes.
    const comboboxes = screen.getAllByRole('combobox');
    const pageSizeSelect = comboboxes.find((el) =>
      within(el)
        .queryAllByRole('option')
        .some((o) => o.textContent === '200'),
    )!;
    fireEvent.change(pageSizeSelect, { target: { value: '200' } });
    // After choosing 200 the next GET carries page_size=200 (the 200 ceiling).
    await waitFor(() =>
      expect(seen.some((s) => s.includes('page_size=200'))).toBe(true),
    );
  });

  it('snowflake id is preserved as a STRING (no >2^53 precision loss) in detail', async () => {
    renderPage();
    await screen.findByText('table_allow');
    // Open the detail drawer for the constraint row.
    fireEvent.click(screen.getByRole('button', { name: '查看详情' }));
    const fullId = fx.constraints[0]!.id; // 7300000000000005001 (> 2^53)
    // The full id appears verbatim (title attr) — never coerced to Number.
    const idEl = await screen.findByTitle(fullId);
    expect(idEl).toBeInTheDocument();
    // Number coercion would corrupt this value; assert it survived intact.
    expect(fullId).toBe('7300000000000005001');
    expect(Number(fullId).toString()).not.toBe(fullId); // proves the trap is real
  });
});

describe('ConstraintsPage — 写流程（create constraint）', () => {
  it('create: invalid (non-JSON) spec blocks submit with a syntax error', async () => {
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('button', { name: /新建细则/ }));
    // Fill resource + kind, but a non-JSON spec.
    const drawer = screen.getByRole('dialog');
    fireEvent.change(within(drawer).getByDisplayValue('选择资源…'), {
      target: { value: 'db-main' },
    });
    fireEvent.change(within(drawer).getByDisplayValue('选择 kind…'), {
      target: { value: 'table_allow' },
    });
    fireEvent.change(within(drawer).getByPlaceholderText('{"prefix":"app-"}'), {
      target: { value: 'not json' },
    });
    fireEvent.click(within(drawer).getByRole('button', { name: '提交' }));
    expect(
      await within(drawer).findByText('spec 必须是可解析的 JSON'),
    ).toBeInTheDocument();
  });

  it('create: valid submit posts and surfaces policy_rev advanced', async () => {
    let posted: unknown = null;
    server.use(
      http.post(`${BASE}/constraints`, async ({ request }) => {
        posted = await request.json();
        return HttpResponse.json({ policy_rev: '9001' });
      }),
    );
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('button', { name: /新建细则/ }));
    const drawer = screen.getByRole('dialog');
    fireEvent.change(within(drawer).getByDisplayValue('选择资源…'), {
      target: { value: 'db-main' },
    });
    fireEvent.change(within(drawer).getByDisplayValue('选择 kind…'), {
      target: { value: 'table_allow' },
    });
    fireEvent.change(within(drawer).getByPlaceholderText('{"prefix":"app-"}'), {
      target: { value: '{"tables":["orders"]}' },
    });
    fireEvent.click(within(drawer).getByRole('button', { name: '提交' }));
    expect(
      await screen.findByText(/细则已挂载，policy_rev 前进至 9001/),
    ).toBeInTheDocument();
    expect(posted).toMatchObject({
      resource: 'db-main',
      kind: 'table_allow',
      spec: '{"tables":["orders"]}',
    });
  });

  it('kind matrix narrows to the adapter (postgres → no container_prefix)', async () => {
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('button', { name: /新建细则/ }));
    const drawer = screen.getByRole('dialog');
    fireEvent.change(within(drawer).getByDisplayValue('选择资源…'), {
      target: { value: 'db-main' },
    });
    const kindSelect = within(drawer).getByDisplayValue('选择 kind…');
    const optionTexts = within(kindSelect)
      .getAllByRole('option')
      .map((o) => o.textContent);
    // postgres adapter declares table_allow/column_mask/mask_fields — NOT docker's container_prefix.
    expect(optionTexts).toContain('table_allow');
    expect(optionTexts).not.toContain('container_prefix');
  });
});

describe('ConstraintsPage — 乐观锁 409（编辑/删除）', () => {
  it('delete conflict (409) surfaces "refresh & retry", view NOT mutated', async () => {
    server.use(
      http.post(`${BASE}/constraints`, () =>
        HttpResponse.json(
          { error: { code: 'version_conflict', message: 'stale version' } },
          { status: 409 },
        ),
      ),
    );
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('button', { name: '删除' }));
    // ConfirmDialog requires the explicit checkbox-word for constraints.
    const dialog = screen.getByRole('dialog');
    fireEvent.change(within(dialog).getByRole('textbox'), {
      target: { value: '我已知此操作扩大授权作用面' },
    });
    fireEvent.click(within(dialog).getByRole('button', { name: '删除' }));
    expect(
      await screen.findByText(/他人已修改此记录，请刷新后基于最新 version 重试/),
    ).toBeInTheDocument();
    // The row is still present — no silent overwrite / no optimistic removal.
    expect(screen.getByText('table_allow')).toBeInTheDocument();
  });
});

describe('ConstraintsPage — 删除=扩大作用面（危险确认）', () => {
  it('delete requires the explicit scope-widening acknowledgement word', async () => {
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('button', { name: '删除' }));
    const dialog = screen.getByRole('dialog');
    // The danger dialog spells out scope widening.
    expect(dialog).toHaveTextContent(/放宽/);
    // Confirm is disabled until the exact acknowledgement word is typed.
    const confirmBtn = within(dialog).getByRole('button', { name: '删除' });
    expect(confirmBtn).toBeDisabled();
    fireEvent.change(within(dialog).getByRole('textbox'), {
      target: { value: '我已知此操作扩大授权作用面' },
    });
    expect(confirmBtn).toBeEnabled();
  });

  it('confirmed delete posts delete_flag=1 with the row version', async () => {
    let posted: { delete_flag?: number; version?: number } | null = null;
    server.use(
      http.post(`${BASE}/constraints`, async ({ request }) => {
        posted = (await request.json()) as typeof posted;
        return HttpResponse.json({ policy_rev: '9100' });
      }),
    );
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('button', { name: '删除' }));
    const dialog = screen.getByRole('dialog');
    fireEvent.change(within(dialog).getByRole('textbox'), {
      target: { value: '我已知此操作扩大授权作用面' },
    });
    fireEvent.click(within(dialog).getByRole('button', { name: '删除' }));
    await waitFor(() => expect(posted).not.toBeNull());
    expect(posted!.delete_flag).toBe(1);
    expect(posted!.version).toBe(fx.constraints[0]!.version);
  });
});

describe('ConstraintsPage — 条件作用域留空 → 二次确认（范围放大）', () => {
  it('empty scope routes through a ConfirmDialog before posting', async () => {
    const postSpy = vi.fn();
    server.use(
      http.post(`${BASE}/conditions`, async ({ request }) => {
        postSpy(await request.json());
        return HttpResponse.json({ policy_rev: '9200' });
      }),
    );
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('tab', { name: /条件 Conditions/ }));
    await screen.findByText('rate_limit');
    fireEvent.click(screen.getByRole('button', { name: /新建条件/ }));
    const drawer = screen.getByRole('dialog');
    // Leave resource + capability empty (= 全资源/全动词), fill a valid spec.
    fireEvent.change(within(drawer).getByPlaceholderText('{"per_minute":60}'), {
      target: { value: '{"per_minute":60}' },
    });
    // Summary preview强提示 the widened scope.
    expect(within(drawer).getByTestId('scope-widen-hint')).toHaveTextContent(
      /全资源\/全动词/,
    );
    fireEvent.click(within(drawer).getByRole('button', { name: '提交' }));
    // A confirm dialog intercepts before the POST fires.
    const confirm = await screen.findByRole('dialog', { name: '确认放大作用域' });
    expect(postSpy).not.toHaveBeenCalled();
    fireEvent.click(within(confirm).getByRole('button', { name: '确认创建' }));
    await waitFor(() => expect(postSpy).toHaveBeenCalledTimes(1));
    expect(postSpy).toHaveBeenCalledWith(
      expect.objectContaining({ resource: null, capability: null, predicate: 'rate_limit' }),
    );
  });
});

describe('ConstraintsPage — deny-note 唯一性：已存在 → 编辑语态', () => {
  it('editing an existing note shows the edit phrasing and carries version', async () => {
    renderPage();
    await screen.findByText('table_allow');
    fireEvent.click(screen.getByRole('tab', { name: /拒绝指引 Deny-notes/ }));
    await screen.findByText(fx.denyNotes[0]!.note);
    fireEvent.click(screen.getByRole('button', { name: '编辑' }));
    const drawer = screen.getByRole('dialog');
    // Edit phrasing (not "create a second one").
    expect(drawer).toHaveTextContent(/已有生效拒绝指引/);
    // The verbatim-relay warning is pinned at the top.
    expect(drawer).toHaveTextContent(
      /此文本越权时将原样回给 Agent（operator_note）/,
    );
    // Prefilled with the existing note text.
    expect(within(drawer).getByDisplayValue(fx.denyNotes[0]!.note)).toBeInTheDocument();
  });
});
