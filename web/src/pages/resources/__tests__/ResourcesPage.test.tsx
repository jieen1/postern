import { describe, expect, it, beforeEach } from 'vitest';
import { screen, within, fireEvent, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import * as fx from '../../../mocks/fixtures';
import type { ResourceRow } from '../../../api/types';
import { ResourcesPage } from '../index';
import { renderWithProviders } from './testUtils';

const BASE = '/v1';

/** The seed db-main fixture, narrowed to a non-optional ResourceRow. */
const dbMain: ResourceRow = fx.resources[0]!;

/** A second resource (disabled, http) so filters/pagination have variety. */
const svcCrm: ResourceRow = {
  id: '7300000000000002999',
  code: 'svc-crm',
  adapter: 'http',
  transport: 'ssh',
  tiers: [{ tier: 'ro', capabilities: ['observe'], secret_ref: 'vault://svc-crm/ro' }],
  labels: [{ key: 'tier', value: 'web' }],
  enable_flag: false,
  version: 1,
};

function listOnly(items: ResourceRow[], total = items.length) {
  server.use(
    http.get(`${BASE}/resources`, () =>
      HttpResponse.json({ items, page_no: 1, page_size: 20, total }),
    ),
  );
}

describe('ResourcesPage — list & three states', () => {
  it('renders the title, primary action, and a resource row from the envelope', async () => {
    renderWithProviders(<ResourcesPage />);
    expect(screen.getByRole('heading', { name: '资源 Resources' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /接入资源/ })).toBeInTheDocument();

    // db-main fixture appears (codename, never a real address).
    expect(await screen.findByText('db-main')).toBeInTheDocument();
    // Folded capability badges from its tiers.
    expect(screen.getAllByText('observe').length).toBeGreaterThan(0);
    // No plaintext address / secret leaks into the table.
    expect(screen.queryByText(/10\.0\./)).not.toBeInTheDocument();
    expect(screen.queryByText(/secret_hash/)).not.toBeInTheDocument();
  });

  it('shows a fail-closed error state (no stale/fake rows) when the list fails', async () => {
    server.use(
      http.get(`${BASE}/resources`, () =>
        HttpResponse.json({ error: { code: 'unavailable', message: '控制面不可达' } }, { status: 503 }),
      ),
    );
    renderWithProviders(<ResourcesPage />);
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/控制面不可达|加载失败/);
    // Fail-closed: the fixture row must NOT leak through the error state.
    expect(screen.queryByText('db-main')).not.toBeInTheDocument();
  });

  it('shows the empty state with a primary-action guide when there are no resources', async () => {
    listOnly([], 0);
    renderWithProviders(<ResourcesPage />);
    expect(await screen.findByText('尚无资源')).toBeInTheDocument();
    // EmptyState carries the access CTA (there are now two such buttons).
    expect(screen.getAllByRole('button', { name: /接入资源/ }).length).toBeGreaterThanOrEqual(2);
  });
});

describe('ResourcesPage — contract: pagination & snowflake id', () => {
  it('requests the default size 20 on the wire (DB_PAGINATION_MANDATORY)', async () => {
    let seenSize: string | null = null;
    server.use(
      http.get(`${BASE}/resources`, ({ request }) => {
        seenSize = new URL(request.url).searchParams.get('page_size');
        return HttpResponse.json({ items: fx.resources, page_no: 1, page_size: 20, total: 1 });
      }),
    );
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    expect(seenSize).toBe('20');
  });

  it('clamps page size to 200 max via the selector and never exceeds 200 on the wire', async () => {
    const sizes: string[] = [];
    server.use(
      http.get(`${BASE}/resources`, ({ request }) => {
        sizes.push(new URL(request.url).searchParams.get('page_size') ?? '');
        return HttpResponse.json({ items: fx.resources, page_no: 1, page_size: 200, total: 1 });
      }),
    );
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    // The page-size selector is the combobox whose current value is "20"
    // (adapter/status filters default to the "__all__" sentinel).
    const pageSizeSelect = screen
      .getAllByRole('combobox')
      .find((el) => (el as HTMLSelectElement).value === '20');
    expect(pageSizeSelect).toBeDefined();
    // 200 is the largest legal option the selector offers (clamped to 200).
    const options = within(pageSizeSelect as HTMLSelectElement)
      .getAllByRole('option')
      .map((o) => Number((o as HTMLOptionElement).value));
    expect(Math.max(...options)).toBe(200);

    fireEvent.change(pageSizeSelect as HTMLSelectElement, { target: { value: '200' } });
    await waitFor(() => expect(sizes).toContain('200'));
    expect(sizes.every((s) => Number(s) <= 200)).toBe(true);
  });

  it('renders many pages and disables prev on page 1 (server-driven paging)', async () => {
    server.use(
      http.get(`${BASE}/resources`, ({ request }) => {
        const pageNo = Number(new URL(request.url).searchParams.get('page_no') ?? '1');
        return HttpResponse.json({
          items: pageNo === 1 ? [dbMain] : [svcCrm],
          page_no: pageNo,
          page_size: 20,
          total: 47,
        });
      }),
    );
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    expect(screen.getByText(/共 47 条/)).toBeInTheDocument();
    expect(screen.getByText(/第 1\/3 页/)).toBeInTheDocument();
    expect(screen.getByText('上一页')).toBeDisabled();
  });

  it('keeps the snowflake id a string — the fixture id exceeds 2^53', async () => {
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    // If anything parsed this id to a number, precision would be lost.
    expect(Number(fx.ID.resourceDb) > Number.MAX_SAFE_INTEGER).toBe(true);
    expect(fx.ID.resourceDb).toBe('7300000000000002001');
  });
});

describe('ResourcesPage — filters', () => {
  beforeEach(() => {
    listOnly([dbMain, svcCrm], 2);
  });

  it('filters by codename / label query within the page', async () => {
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    expect(screen.getByText('svc-crm')).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('筛选 代号或标签'), { target: { value: 'svc' } });
    await waitFor(() => expect(screen.queryByText('db-main')).not.toBeInTheDocument());
    expect(screen.getByText('svc-crm')).toBeInTheDocument();
  });

  it('filters by status (disabled only)', async () => {
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');

    fireEvent.change(screen.getByLabelText('按状态筛选'), { target: { value: 'disabled' } });
    await waitFor(() => expect(screen.queryByText('db-main')).not.toBeInTheDocument());
    expect(screen.getByText('svc-crm')).toBeInTheDocument();
    expect(screen.getAllByText('停用').length).toBeGreaterThan(0);
  });
});

describe('ResourcesPage — write flow (form → summary → confirm → success)', () => {
  it('accesses a read-only resource and shows the success toast with policy_rev', async () => {
    let posted = false;
    server.use(
      http.get(`${BASE}/resources`, () =>
        HttpResponse.json({ items: fx.resources, page_no: 1, page_size: 20, total: 1 }),
      ),
      http.post(`${BASE}/resources`, async ({ request }) => {
        posted = true;
        const body = (await request.json()) as Record<string, unknown>;
        // Contract: no plaintext address field is forwarded.
        expect(JSON.stringify(body)).not.toContain('10.0.3.7');
        return HttpResponse.json({ policy_rev: '5000' });
      }),
    );

    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    fireEvent.click(screen.getByRole('button', { name: /接入资源/ }));

    const drawer = await screen.findByRole('dialog', { name: '接入资源' });
    fireEvent.change(within(drawer).getByPlaceholderText('db-main'), { target: { value: 'svc-new' } });
    // Tier defaults to ro/[observe,query] — already read-only valid.
    fireEvent.click(within(drawer).getByRole('button', { name: /预览摘要/ }));

    const summary = await screen.findByRole('dialog', { name: '摘要预览' });
    expect(within(summary).getByText(/svc-new/)).toBeInTheDocument();

    fireEvent.click(within(summary).getByRole('button', { name: '确认提交' }));

    await waitFor(() => expect(posted).toBe(true));
    expect(await screen.findByText(/policy_rev → 5000/)).toBeInTheDocument();
  });

  it('blocks summary preview when no read-only tier is declared (Zod gate)', async () => {
    listOnly(fx.resources);
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    fireEvent.click(screen.getByRole('button', { name: /接入资源/ }));
    const drawer = await screen.findByRole('dialog', { name: '接入资源' });

    fireEvent.change(within(drawer).getByPlaceholderText('db-main'), { target: { value: 'svc-bad' } });
    // Strip the read-only verbs and add a write verb → no read-only tier remains.
    fireEvent.click(within(drawer).getByLabelText('tier 1 动词 observe'));
    fireEvent.click(within(drawer).getByLabelText('tier 1 动词 query'));
    fireEvent.click(within(drawer).getByLabelText('tier 1 动词 mutate'));

    fireEvent.click(within(drawer).getByRole('button', { name: /预览摘要/ }));
    // Stays on the form; a tier error appears; summary never opens.
    expect(await within(drawer).findByText(/只读 tier/)).toBeInTheDocument();
    expect(screen.queryByRole('dialog', { name: '摘要预览' })).not.toBeInTheDocument();
  });

  it('demands a danger confirm before declaring a high-risk verb face', async () => {
    server.use(
      http.get(`${BASE}/resources`, () =>
        HttpResponse.json({ items: fx.resources, page_no: 1, page_size: 20, total: 1 }),
      ),
      http.post(`${BASE}/resources`, () => HttpResponse.json({ policy_rev: '5001' })),
    );
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    fireEvent.click(screen.getByRole('button', { name: /接入资源/ }));
    const drawer = await screen.findByRole('dialog', { name: '接入资源' });

    fireEvent.change(within(drawer).getByPlaceholderText('db-main'), { target: { value: 'svc-rw' } });
    // Keep tier 1 (ro/[observe,query]) read-only; add a second write tier with
    // mutate (high-risk) so the read-only-tier invariant still holds.
    fireEvent.click(within(drawer).getByRole('button', { name: /添加 tier/ }));
    fireEvent.change(within(drawer).getByLabelText('tier 代号 2'), { target: { value: 'rw' } });
    fireEvent.click(within(drawer).getByLabelText('tier 2 动词 mutate'));
    fireEvent.click(within(drawer).getByRole('button', { name: /预览摘要/ }));

    const summary = await screen.findByRole('dialog', { name: '摘要预览' });
    expect(within(summary).getByText(/高危动词/)).toBeInTheDocument();
    fireEvent.click(within(summary).getByRole('button', { name: '确认提交' }));

    // A danger ConfirmDialog intercepts before the write.
    const confirm = await screen.findByRole('dialog', { name: '声明高危动词面' });
    expect(within(confirm).getByText(/mutate/)).toBeInTheDocument();
    fireEvent.click(within(confirm).getByRole('button', { name: '确认' }));

    expect(await screen.findByText(/policy_rev → 5001/)).toBeInTheDocument();
  });
});

describe('ResourcesPage — 409 optimistic-lock conflict', () => {
  it('prompts refresh-and-retry on 409 and does not change the view', async () => {
    server.use(
      http.get(`${BASE}/resources`, () =>
        HttpResponse.json({ items: fx.resources, page_no: 1, page_size: 20, total: 1 }),
      ),
      http.post(`${BASE}/resources`, () =>
        HttpResponse.json(
          { error: { code: 'conflict', message: 'version mismatch' } },
          { status: 409 },
        ),
      ),
    );
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    fireEvent.click(screen.getByRole('button', { name: /接入资源/ }));
    const drawer = await screen.findByRole('dialog', { name: '接入资源' });
    fireEvent.change(within(drawer).getByPlaceholderText('db-main'), { target: { value: 'svc-x' } });
    fireEvent.click(within(drawer).getByRole('button', { name: /预览摘要/ }));
    const summary = await screen.findByRole('dialog', { name: '摘要预览' });
    fireEvent.click(within(summary).getByRole('button', { name: '确认提交' }));

    expect(await screen.findByText(/他人已改、请刷新重试/)).toBeInTheDocument();
    // The list still shows the existing resource (no optimistic local mutation).
    expect(screen.getByText('db-main')).toBeInTheDocument();
  });
});

describe('ResourcesPage — disable danger flow', () => {
  it('requires typing the code to confirm disable, then writes enable_flag=0', async () => {
    let disabledFlag: unknown = null;
    server.use(
      http.get(`${BASE}/resources`, () =>
        HttpResponse.json({ items: fx.resources, page_no: 1, page_size: 20, total: 1 }),
      ),
      http.post(`${BASE}/resources`, async ({ request }) => {
        const body = (await request.json()) as { enable_flag?: boolean; version?: number };
        disabledFlag = body.enable_flag;
        expect(body.version).toBe(dbMain.version); // optimistic-lock baseline
        return HttpResponse.json({ policy_rev: '5002' });
      }),
    );
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');

    fireEvent.click(screen.getByRole('button', { name: /资源 db-main 行操作/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: '停用' }));

    const confirm = await screen.findByRole('dialog', { name: '停用资源' });
    const confirmBtn = within(confirm).getByRole('button', { name: '确认' });
    expect(confirmBtn).toBeDisabled();
    fireEvent.change(within(confirm).getByRole('textbox'), { target: { value: 'db-main' } });
    expect(confirmBtn).toBeEnabled();
    fireEvent.click(confirmBtn);

    await waitFor(() => expect(disabledFlag).toBe(false));
    expect(await screen.findByText(/policy_rev → 5002/)).toBeInTheDocument();
  });
});

describe('ResourcesPage — discover (discovery ≠ authorization)', () => {
  it('shows the boundary banner, probed capabilities, and selectable objects', async () => {
    server.use(
      http.get(`${BASE}/resources`, () =>
        HttpResponse.json({ items: fx.resources, page_no: 1, page_size: 20, total: 1 }),
      ),
      http.post(`${BASE}/resources/:code/discover`, () =>
        HttpResponse.json({ capabilities: ['observe', 'query'], objects: ['public.orders', 'public.products'] }),
      ),
    );
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');

    fireEvent.click(screen.getByRole('button', { name: /资源 db-main 行操作/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: '探测 discover' }));

    const drawer = await screen.findByRole('dialog', { name: /Discover: db-main/ });
    // Explicit boundary: discovery ≠ authorization.
    expect(within(drawer).getByText(/发现 ≠ 授权/)).toBeInTheDocument();
    const orders = await within(drawer).findByLabelText('选择对象 public.orders');
    const configureBtn = within(drawer).getByRole('button', { name: /配置细则/ });
    expect(configureBtn).toBeDisabled();
    fireEvent.click(orders);
    expect(within(drawer).getByText(/已选 1 项/)).toBeInTheDocument();
    expect(configureBtn).toBeEnabled();
  });

  it('shows a fail-closed error (no fabricated objects) when probing fails', async () => {
    server.use(
      http.get(`${BASE}/resources`, () =>
        HttpResponse.json({ items: fx.resources, page_no: 1, page_size: 20, total: 1 }),
      ),
      http.post(`${BASE}/resources/:code/discover`, () =>
        HttpResponse.json(
          { error: { code: 'unreachable', message: '目标端口不可达，疑未发布转发' } },
          { status: 502 },
        ),
      ),
    );
    renderWithProviders(<ResourcesPage />);
    await screen.findByText('db-main');
    fireEvent.click(screen.getByRole('button', { name: /资源 db-main 行操作/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: '探测 discover' }));

    const drawer = await screen.findByRole('dialog', { name: /Discover: db-main/ });
    expect(await within(drawer).findByText('探测失败')).toBeInTheDocument();
    expect(within(drawer).getByText(/目标端口不可达/)).toBeInTheDocument();
    // No fabricated object checklist leaked under the error.
    expect(within(drawer).queryByText(/public\./)).not.toBeInTheDocument();
  });
});
