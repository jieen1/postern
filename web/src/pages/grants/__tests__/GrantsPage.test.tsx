import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';
import { screen, waitFor, within, fireEvent } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import type { GrantsView } from '../../../api/types';
import { GrantsPage } from '../index';
import { renderWithProviders } from './renderPage';

const BASE = '/v1';

// Pin Date.now so TtlBadge / temp-grant liveness are deterministic, while
// leaving real timers/promises intact for React Query + MSW async.
const FIXED_NOW = Date.parse('2026-06-14T00:00:00Z');
const futureIso = (h: number) => new Date(FIXED_NOW + h * 3_600_000).toISOString();

beforeEach(() => {
  vi.spyOn(Date, 'now').mockReturnValue(FIXED_NOW);
});
afterEach(() => {
  vi.restoreAllMocks();
});

/** A grants view with live + scope-bounded data, used by most tests. */
const liveGrants: GrantsView = {
  your_grants: {
    'db-main': ['observe', 'query'],
    'redis-main': ['observe', 'query', 'mutate'],
  },
  temp_grants: [
    {
      id: '7300000000000003001',
      resource: 'redis-main',
      capability: 'destroy',
      granted_at: '2026-06-13T23:00:00Z',
      expires_at: futureIso(4),
      ended_at: null,
      end_reason: null,
      version: 1,
    },
  ],
};

function useGrantsHandler(view: GrantsView) {
  server.use(http.get(`${BASE}/grants`, () => HttpResponse.json(view)));
}

/** Set a native <select> by its accessible label and fire change. */
function selectByLabel(scope: HTMLElement, label: string, value: string) {
  const el = within(scope).getByLabelText(label) as HTMLSelectElement;
  fireEvent.change(el, { target: { value } });
}

/** Wait for the grant matrix to render and return its <table> element. */
async function waitForMatrix(): Promise<HTMLElement> {
  return screen.findByRole('table', { name: '生效授权矩阵' });
}

describe('GrantsPage — 矩阵渲染与 scope-bounded 契约', () => {
  it('renders the matrix with persistent + temp + default-deny cells and 6 capability columns', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />);

    const matrix = await waitForMatrix();
    expect(within(matrix).getByText('redis-main')).toBeInTheDocument();
    expect(within(matrix).getByText('db-main')).toBeInTheDocument();

    for (const cap of ['observe', 'query', 'mutate', 'execute', 'manage', 'destroy']) {
      expect(within(matrix).getByText(cap)).toBeInTheDocument();
    }

    // three-state cells keyed by aria-label (state conveyed by text+icon, not color)
    expect(screen.getByLabelText('db-main × observe：持久')).toBeInTheDocument();
    expect(screen.getByLabelText('redis-main × destroy：临时')).toBeInTheDocument();
    expect(screen.getByLabelText('db-main × mutate：默认拒绝')).toBeInTheDocument();
  });

  it('does NOT synthesize a row for a resource the daemon never returned (existence not surfaced)', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />);
    await waitForMatrix();
    expect(screen.queryByText('svc-orders')).not.toBeInTheDocument();
    expect(screen.queryByText('mq-main')).not.toBeInTheDocument();
    expect(screen.queryByText(/不存在/)).not.toBeInTheDocument();
  });

  it('renders the temp_grant id as a string without precision loss', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />);
    const idCell = await screen.findByTitle('7300000000000003001');
    expect(idCell).toBeInTheDocument();
    // sanity: this id is > 2^53 so a Number round-trip would corrupt it
    expect(Number('7300000000000003001').toString()).not.toBe('7300000000000003001');
  });
});

describe('GrantsPage — 三态 fail-closed', () => {
  it('shows a loading state before data resolves', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />);
    expect(screen.getAllByRole('status').length).toBeGreaterThan(0);
    await waitForMatrix();
  });

  it('shows a fail-closed error with NO grant cells when GET /v1/grants fails', async () => {
    server.use(
      http.get(`${BASE}/grants`, () =>
        HttpResponse.json(
          { error: { code: 'unavailable', message: 'daemon 不可达' } },
          { status: 503 },
        ),
      ),
    );
    renderWithProviders(<GrantsPage />);

    expect(await screen.findByText('无法读取授权矩阵')).toBeInTheDocument();
    expect(screen.queryByRole('table', { name: '生效授权矩阵' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/：持久$/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Elevate 提权/ })).toBeDisabled();
  });

  it('shows the empty (safe) state when the principal has no grant cells', async () => {
    useGrantsHandler({ your_grants: {}, temp_grants: [] });
    renderWithProviders(<GrantsPage />);
    expect(await screen.findByText(/无任何生效授权（默认拒绝世界）/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Elevate 提权/ })).toBeEnabled();
  });

  it('shows an empty temp-grants list when there are no live temp grants', async () => {
    useGrantsHandler({ your_grants: { 'db-main': ['observe'] }, temp_grants: [] });
    renderWithProviders(<GrantsPage />);
    expect(await screen.findByText('当前无生效临时授权')).toBeInTheDocument();
  });
});

describe('GrantsPage — Principal 选择 + policy_rev 对账锚点', () => {
  it('defaults to the first principal and shows policy_rev from health', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />);
    const select = (await screen.findByLabelText('选择 Principal')) as HTMLSelectElement;
    // principals load async; the selector defaults to the first principal
    await waitFor(() => expect(select.value).toBe('agent-order-bot'));
    expect(await screen.findByText('4187')).toBeInTheDocument();
  });
});

describe('GrantsPage — 跨页深链预填 (?principal & ?resource 回显该格)', () => {
  it('prefills the principal selector and resource filter from the URL, echoing the linked cell', async () => {
    useGrantsHandler(liveGrants);
    // Deep link from Denials: ?principal=alice (the SECOND principal, not the
    // default first) + ?resource=db-main (a real resource substring).
    renderWithProviders(<GrantsPage />, ['/grants?principal=alice&resource=db-main']);

    const select = (await screen.findByLabelText('选择 Principal')) as HTMLSelectElement;
    // The selector resolves to the deep-linked principal once principals load,
    // NOT the default first principal.
    await waitFor(() => expect(select.value).toBe('alice'));

    // The resource filter is prefilled from ?resource …
    expect((screen.getByLabelText('资源代号筛选') as HTMLInputElement).value).toBe('db-main');

    // … and the matrix converges to the linked resource (db-main remains,
    // redis-main is filtered out by the prefilled substring).
    const matrix = await waitForMatrix();
    expect(within(matrix).getByText('db-main')).toBeInTheDocument();
    expect(within(matrix).queryByText('redis-main')).not.toBeInTheDocument();
  });

  it('falls back to the default principal when the deep-linked principal is unknown', async () => {
    useGrantsHandler(liveGrants);
    // ?principal points at a principal that is NOT in the loaded list.
    renderWithProviders(<GrantsPage />, ['/grants?principal=ghost-not-real&resource=db']);

    const select = (await screen.findByLabelText('选择 Principal')) as HTMLSelectElement;
    // Unknown principal → fall back to the default first principal (no phantom).
    await waitFor(() => expect(select.value).toBe('agent-order-bot'));
    // The resource filter still honors ?resource regardless of the principal.
    expect((screen.getByLabelText('资源代号筛选') as HTMLInputElement).value).toBe('db');
  });

  it('keeps the current default when no URL params are present', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />); // no query string

    const select = (await screen.findByLabelText('选择 Principal')) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe('agent-order-bot'));
    expect((screen.getByLabelText('资源代号筛选') as HTMLInputElement).value).toBe('');
  });
});

describe('GrantsPage — Elevate 写流程 (TTL 必填 → 摘要 → 危险确认)', () => {
  it('blocks submit without a valid TTL and surfaces the Zod message', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />);
    await waitForMatrix();

    fireEvent.click(screen.getByRole('button', { name: /Elevate 提权/ }));
    const drawer = await screen.findByRole('dialog', { name: '临时提权 Elevate' });

    selectByLabel(drawer, 'Resource *', 'redis-main');
    selectByLabel(drawer, 'Capability *', 'destroy');
    // empty TTL violates >0
    fireEvent.change(within(drawer).getByLabelText('TTL 数值'), { target: { value: '' } });

    fireEvent.click(within(drawer).getByRole('button', { name: /提权…（危险确认）/ }));

    expect(await within(drawer).findByText(/提权必须带 TTL/)).toBeInTheDocument();
    // no danger confirm opened on invalid submit
    expect(
      screen.queryByRole('dialog', { name: '确认临时提权（扩权）' }),
    ).not.toBeInTheDocument();
  });

  it('shows a summary preview then requires typing the resource code to confirm; success closes the drawer', async () => {
    useGrantsHandler(liveGrants);
    const elevateSpy = vi.fn();
    server.use(
      http.post(`${BASE}/grants/temp/elevate`, async ({ request }) => {
        elevateSpy(await request.json());
        return HttpResponse.json({ policy_rev: '4200' });
      }),
    );
    renderWithProviders(<GrantsPage />);
    await waitForMatrix();

    fireEvent.click(screen.getByRole('button', { name: /Elevate 提权/ }));
    const drawer = await screen.findByRole('dialog', { name: '临时提权 Elevate' });
    selectByLabel(drawer, 'Resource *', 'redis-main');
    selectByLabel(drawer, 'Capability *', 'destroy');

    // summary preview reflects the chosen fields
    expect(within(drawer).getByText(/将给/)).toBeInTheDocument();
    expect(within(drawer).getByText('扩大')).toBeInTheDocument();

    fireEvent.click(within(drawer).getByRole('button', { name: /提权…（危险确认）/ }));

    const confirm = await screen.findByRole('dialog', { name: '确认临时提权（扩权）' });
    const confirmBtn = within(confirm).getByRole('button', { name: '确认提权' });
    expect(confirmBtn).toBeDisabled();
    fireEvent.change(within(confirm).getByRole('textbox'), { target: { value: 'redis-main' } });
    expect(confirmBtn).toBeEnabled();
    fireEvent.click(confirmBtn);

    await waitFor(() => expect(elevateSpy).toHaveBeenCalledTimes(1));
    expect(elevateSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        principal: 'agent-order-bot',
        resource: 'redis-main',
        capability: 'destroy',
        ttl_ms: 30 * 60_000,
      }),
    );
    await waitFor(() =>
      expect(
        screen.queryByRole('dialog', { name: '临时提权 Elevate' }),
      ).not.toBeInTheDocument(),
    );
  });
});

describe('GrantsPage — Revoke 写流程 (确认 + 乐观锁 version + 409)', () => {
  it('revokes a temp grant carrying the read version, after danger confirm', async () => {
    useGrantsHandler(liveGrants);
    const revokeSpy = vi.fn();
    server.use(
      http.post(`${BASE}/grants/temp/revoke`, async ({ request }) => {
        revokeSpy(await request.json());
        return HttpResponse.json({ policy_rev: '4201' });
      }),
    );
    renderWithProviders(<GrantsPage />);
    await waitForMatrix();

    fireEvent.click(screen.getByRole('button', { name: '吊销' }));
    const confirm = await screen.findByRole('dialog', { name: '确认吊销临时授权（收权）' });
    expect(within(confirm).getByText(/立即关闭/)).toBeInTheDocument();
    fireEvent.click(within(confirm).getByRole('button', { name: '确认吊销' }));

    await waitFor(() => expect(revokeSpy).toHaveBeenCalledTimes(1));
    // optimistic-lock version forwarded from the read row
    expect(revokeSpy).toHaveBeenCalledWith({ id: '7300000000000003001', version: 1 });
  });

  it('surfaces a 409 conflict (refresh & retry) without silent retry or view mutation', async () => {
    useGrantsHandler(liveGrants);
    let calls = 0;
    server.use(
      http.post(`${BASE}/grants/temp/revoke`, () => {
        calls += 1;
        return HttpResponse.json(
          { error: { code: 'conflict', message: '版本已变更' } },
          { status: 409 },
        );
      }),
    );
    renderWithProviders(<GrantsPage />);
    await waitForMatrix();

    fireEvent.click(screen.getByRole('button', { name: '吊销' }));
    const confirm = await screen.findByRole('dialog', { name: '确认吊销临时授权（收权）' });
    fireEvent.click(within(confirm).getByRole('button', { name: '确认吊销' }));

    expect(
      await within(confirm).findByText(/请刷新重读后重试/),
    ).toBeInTheDocument();
    expect(calls).toBe(1);
    // local view unchanged on failure — the temp grant row is still present
    expect(screen.getByTitle('7300000000000003001')).toBeInTheDocument();
  });
});

describe('GrantsPage — 矩阵筛选与格 provenance 抽屉', () => {
  it('filters matrix rows by resource code', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />);
    await waitForMatrix();

    fireEvent.change(screen.getByLabelText('资源代号筛选'), { target: { value: 'redis' } });
    // db-main row drops out of the matrix; redis-main remains
    await waitFor(() => expect(screen.queryByText('db-main')).not.toBeInTheDocument());
    const matrix = screen.getByRole('table', { name: '生效授权矩阵' });
    expect(within(matrix).getByText('redis-main')).toBeInTheDocument();
  });

  it('opens the temp-cell provenance drawer with a revoke entry', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />);
    await waitForMatrix();

    fireEvent.click(screen.getByLabelText('redis-main × destroy：临时'));
    const cellDrawer = await screen.findByRole('dialog', { name: '格详情' });
    expect(within(cellDrawer).getByText(/临时授权 \(allow\)/)).toBeInTheDocument();
    expect(
      within(cellDrawer).getByRole('button', { name: /立即吊销 revoke/ }),
    ).toBeInTheDocument();
    // temp drawer shows the backing temp_grant id (string, full value in title)
    expect(within(cellDrawer).getByTitle('7300000000000003001')).toBeInTheDocument();
  });

  it('opens the persistent-cell drawer which links to Bindings (read-only here)', async () => {
    useGrantsHandler(liveGrants);
    renderWithProviders(<GrantsPage />);
    await screen.findByText('db-main');

    fireEvent.click(screen.getByLabelText('db-main × observe：持久'));
    const persistentDrawer = await screen.findByRole('dialog', { name: '格详情' });
    expect(within(persistentDrawer).getByText(/持久授权 \(allow\)/)).toBeInTheDocument();
    expect(
      within(persistentDrawer).getByRole('link', { name: /去 Bindings 页修订/ }),
    ).toHaveAttribute('href', '/bindings');
  });
});
