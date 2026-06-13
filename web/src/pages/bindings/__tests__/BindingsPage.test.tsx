import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { screen, fireEvent, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '@/mocks/server';
import type { Binding } from '@/api/types';
import { renderWithProviders } from './renderPage';
import { BindingsPage } from '../index';

const BASE = '/v1';

// An id well beyond 2^53 — Number(...) would corrupt it (precision trap).
const BIG_VER_ID = '7300000000000000456';

const ROWS: Binding[] = [
  {
    id: '7300000000000000010',
    principal: 'agent2',
    principal_id: '7300000000000000123',
    role: 'maintainer',
    scope_kind: 'selector',
    scope_spec: '{"all":[{"key":"host","value":"A"},{"key":"kind","value":"docker"}]}',
    expanded_resources: ['docker-A'],
    version: 3,
  },
  {
    id: BIG_VER_ID,
    principal: 'agent3',
    principal_id: '7300000000000000999',
    role: 'observer',
    scope_kind: 'selector',
    scope_spec: '{"all":[{"key":"env","value":"staging"}]}',
    expanded_resources: [],
    version: 2,
  },
  {
    id: '7300000000000000020',
    principal: 'agent2',
    principal_id: '7300000000000000123',
    role: 'observer',
    scope_kind: 'resource',
    scope_spec: 'db-main',
    expanded_resources: ['db-main'],
    version: 0,
  },
];

/** Override only the bindings list; the shared handlers cover the rest. */
function withBindings(items: Binding[]) {
  server.use(
    http.get(`${BASE}/bindings`, ({ request }) => {
      const url = new URL(request.url);
      const page_no = Number(url.searchParams.get('page_no') ?? '1');
      const page_size = Number(url.searchParams.get('page_size') ?? '20');
      const start = (page_no - 1) * page_size;
      return HttpResponse.json({
        items: items.slice(start, start + page_size),
        page_no,
        page_size,
        total: items.length,
      });
    }),
  );
}

beforeEach(() => {
  vi.restoreAllMocks();
  Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
});
afterEach(() => {
  server.resetHandlers();
});

describe('BindingsPage — list rendering & contract', () => {
  it('renders the title, primary action and each binding row', async () => {
    withBindings(ROWS);
    renderWithProviders(<BindingsPage />);

    expect(screen.getByRole('heading', { name: /绑定 Bindings/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /新建绑定/ })).toBeInTheDocument();

    expect(await screen.findByText('maintainer')).toBeInTheDocument();
    // Two agent2 rows + one agent3 row.
    expect(screen.getAllByText('agent2')).toHaveLength(2);
    expect(screen.getByText('agent3')).toBeInTheDocument();
  });

  it('shows the snowflake id truncated WITHOUT precision loss (string discipline)', async () => {
    withBindings(ROWS);
    renderWithProviders(<BindingsPage />);

    // The full id is preserved verbatim in the title — never coerced to Number.
    const idEl = await screen.findByTitle('7300000000000000010');
    expect(idEl).toBeInTheDocument();
    // Sanity: this id really is past the safe-integer boundary.
    expect(Number('7300000000000000010') > Number.MAX_SAFE_INTEGER).toBe(true);
    // Truncated head…tail, not the rounded number.
    expect(idEl).toHaveTextContent('7300…0010');
  });

  it('renders selector scope as JSON spec and resource scope as code badge', async () => {
    withBindings(ROWS);
    renderWithProviders(<BindingsPage />);

    await screen.findByText('maintainer');
    // selector spec shown verbatim (machine fact).
    expect(screen.getAllByLabelText('selector spec').length).toBeGreaterThan(0);
    // resource-kind row shows db-main as a code badge (also appears as a code).
    expect(screen.getAllByText('db-main').length).toBeGreaterThan(0);
  });

  it('marks an empty expansion set with the amber 无匹配 fact, not an error (异常 B)', async () => {
    withBindings(ROWS);
    renderWithProviders(<BindingsPage />);

    const amber = await screen.findByText(/0 资源 · 无匹配/);
    expect(amber).toBeInTheDocument();
    // It carries the explanatory title — a fact, not a failure.
    expect(amber).toHaveAttribute('title', expect.stringContaining('无匹配标签'));
    // Non-empty rows show their count.
    expect(screen.getAllByText('1 资源').length).toBe(2);
  });
});

describe('BindingsPage — three states (fail-closed)', () => {
  it('renders a fail-closed ErrorState with NO leaked rows on list error', async () => {
    server.use(
      http.get(`${BASE}/bindings`, () =>
        HttpResponse.json({ error: { code: 'boom', message: '后端炸了' } }, { status: 500 }),
      ),
    );
    renderWithProviders(<BindingsPage />);

    expect(await screen.findByRole('alert')).toHaveTextContent('后端炸了');
    // No fabricated/leaked binding data behind the error.
    expect(screen.queryByText('maintainer')).not.toBeInTheDocument();
    expect(screen.queryByText('agent2')).not.toBeInTheDocument();
  });

  it('renders an EmptyState with a create CTA when there are no bindings', async () => {
    withBindings([]);
    renderWithProviders(<BindingsPage />);

    expect(await screen.findByText('还没有绑定')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /新建第一条绑定/ })).toBeInTheDocument();
  });
});

describe('BindingsPage — filters (do not leak hidden-row counts)', () => {
  it('filters by Role within the page without announcing hidden rows', async () => {
    withBindings(ROWS);
    renderWithProviders(<BindingsPage />);
    await screen.findByText('maintainer');

    fireEvent.change(screen.getByLabelText('按 Role 筛选'), {
      target: { value: 'observer' },
    });

    // maintainer row is filtered out; observer rows remain.
    await waitFor(() => {
      expect(screen.queryByText('maintainer')).not.toBeInTheDocument();
    });
    expect(screen.getByText('agent3')).toBeInTheDocument();
    // Never an "N rows hidden" disclosure (existence non-leak, §3.2).
    expect(screen.queryByText(/被隐藏/)).not.toBeInTheDocument();
    expect(screen.queryByText(/还有 \d+ 行/)).not.toBeInTheDocument();
  });

  it('filters by Scope 类型 = resource', async () => {
    withBindings(ROWS);
    renderWithProviders(<BindingsPage />);
    await screen.findByText('maintainer');

    fireEvent.change(screen.getByLabelText('按 Scope 类型筛选'), {
      target: { value: 'resource' },
    });
    await waitFor(() => {
      expect(screen.queryByText('maintainer')).not.toBeInTheDocument();
    });
    // The selector rows are gone; only the resource-kind row (db-main) remains.
    expect(screen.getByText('db-main')).toBeInTheDocument();
  });
});

describe('BindingsPage — row menu navigation', () => {
  it('opens the detail drawer via 查看展开', async () => {
    withBindings(ROWS);
    renderWithProviders(<BindingsPage />);
    await screen.findByText('maintainer');

    const menus = screen.getAllByRole('button', { name: '行操作' });
    fireEvent.click(menus[0]!);
    fireEvent.click(screen.getByRole('menuitem', { name: '查看展开' }));

    expect(await screen.findByRole('dialog', { name: '绑定展开详情' })).toBeInTheDocument();
    // The expansion result shows the daemon-reported resource code.
    const drawer = screen.getByRole('dialog', { name: '绑定展开详情' });
    expect(within(drawer).getByText('docker-A')).toBeInTheDocument();
  });

  it('shows the 0-resource fact (not an error) in the detail drawer (异常 B)', async () => {
    withBindings(ROWS);
    renderWithProviders(<BindingsPage />);
    await screen.findByText('maintainer');

    // agent3 row (index: the second row) has an empty expansion.
    const menus = screen.getAllByRole('button', { name: '行操作' });
    fireEvent.click(menus[1]!);
    fireEvent.click(screen.getByRole('menuitem', { name: '查看展开' }));

    const drawer = await screen.findByRole('dialog', { name: '绑定展开详情' });
    expect(within(drawer).getByText('展开为 0 个资源（无匹配标签）')).toBeInTheDocument();
    // Not rendered as an alert/error.
    expect(within(drawer).queryByRole('alert')).not.toBeInTheDocument();
  });
});

describe('BindingsPage — delete (danger confirm, optimistic lock)', () => {
  it('requires the DELETE confirm word, then succeeds and advances policy_rev', async () => {
    withBindings(ROWS);
    let posted: { id?: string; version?: number } = {};
    server.use(
      http.post(`${BASE}/bindings/:id/delete`, async ({ request, params }) => {
        posted = { id: String(params.id), ...(await request.json() as { version: number }) };
        return HttpResponse.json({ policy_rev: '9001' });
      }),
    );
    renderWithProviders(<BindingsPage />);
    await screen.findByText('maintainer');

    const menus = screen.getAllByRole('button', { name: '行操作' });
    fireEvent.click(menus[0]!);
    fireEvent.click(screen.getByRole('menuitem', { name: '删除绑定' }));

    const dialog = await screen.findByRole('dialog', { name: '删除绑定' });
    // The summary names the resources whose grants disappear (缩权方向).
    expect(within(dialog).getByText(/docker-A/)).toBeInTheDocument();
    expect(within(dialog).getByText(/缩权方向/)).toBeInTheDocument();

    const confirmBtn = within(dialog).getByRole('button', { name: '确认删除' });
    // Anti-misclick: disabled until the confirm word is typed.
    expect(confirmBtn).toBeDisabled();
    fireEvent.change(within(dialog).getByRole('textbox'), { target: { value: 'DELETE' } });
    expect(confirmBtn).toBeEnabled();
    fireEvent.click(confirmBtn);

    expect(await screen.findByText(/policy_rev 前进至 9001/)).toBeInTheDocument();
    // The read-time version was carried into the optimistic-lock write.
    expect(posted.version).toBe(3);
    expect(posted.id).toBe('7300000000000000010');
  });

  it('on 409 conflict, surfaces "刷新重试" and does NOT silently overwrite (异常 F)', async () => {
    withBindings(ROWS);
    const deleteCall = vi.fn();
    server.use(
      http.post(`${BASE}/bindings/:id/delete`, () => {
        deleteCall();
        return HttpResponse.json(
          { error: { code: 'conflict', message: 'version mismatch' } },
          { status: 409 },
        );
      }),
    );
    renderWithProviders(<BindingsPage />);
    await screen.findByText('maintainer');

    const menus = screen.getAllByRole('button', { name: '行操作' });
    fireEvent.click(menus[0]!);
    fireEvent.click(screen.getByRole('menuitem', { name: '删除绑定' }));
    const dialog = await screen.findByRole('dialog', { name: '删除绑定' });
    fireEvent.change(within(dialog).getByRole('textbox'), { target: { value: 'DELETE' } });
    fireEvent.click(within(dialog).getByRole('button', { name: '确认删除' }));

    expect(await screen.findByText(/刷新重试/)).toBeInTheDocument();
    expect(deleteCall).toHaveBeenCalledTimes(1); // no silent retry
  });
});
