import { describe, expect, it, beforeEach, vi } from 'vitest';
import { screen, fireEvent, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import type { Page, Role } from '../../../api/types';
import { renderRoles, getTable, findRoleRow } from './renderRoles';

// ── A full ladder fixture (observer ⊂ operator ⊂ maintainer) + a narrow role ──
// Ids are deliberately > 2^53 to catch any Number coercion.
const ROLES: Role[] = [
  {
    id: '7300000000000001001',
    name: 'observer',
    effective: [
      { capability: 'observe', action: 'allow' },
      { capability: 'query', action: 'allow' },
    ],
    direct: [
      { capability: 'observe', action: 'allow' },
      { capability: 'query', action: 'allow' },
    ],
    inherits_from: [],
    version: 0,
    updated_at: '2026-06-10T00:00:00Z',
    updated_by: 'admin',
  },
  {
    id: '7300000000000001002',
    name: 'operator',
    effective: [
      { capability: 'observe', action: 'allow' },
      { capability: 'query', action: 'allow' },
      { capability: 'mutate', action: 'allow' },
      { capability: 'execute', action: 'allow' },
    ],
    direct: [
      { capability: 'mutate', action: 'allow' },
      { capability: 'execute', action: 'allow' },
    ],
    inherits_from: ['observer'],
    version: 0,
    updated_at: '2026-06-10T00:00:00Z',
    updated_by: 'admin',
  },
  {
    id: '7300000000000001003',
    name: 'maintainer',
    effective: [
      { capability: 'observe', action: 'allow' },
      { capability: 'query', action: 'allow' },
      { capability: 'mutate', action: 'allow' },
      { capability: 'execute', action: 'allow' },
      { capability: 'manage', action: 'allow' },
    ],
    direct: [{ capability: 'manage', action: 'escalate' }],
    inherits_from: ['operator'],
    version: 0,
    updated_at: '2026-06-10T00:00:00Z',
    updated_by: 'admin',
  },
  {
    id: '7300000000000001004',
    name: 'log-observer',
    effective: [{ capability: 'observe', action: 'allow' }],
    direct: [{ capability: 'observe', action: 'allow' }],
    inherits_from: [],
    version: 0,
    updated_at: '2026-06-10T00:00:00Z',
    updated_by: 'admin',
  },
];

function rolesHandler(items: Role[] = ROLES) {
  return http.get('/v1/roles', ({ request }) => {
    const url = new URL(request.url);
    const page_no = Math.max(1, Number(url.searchParams.get('page_no') ?? '1'));
    const page_size = Math.min(200, Math.max(1, Number(url.searchParams.get('page_size') ?? '20')));
    const start = (page_no - 1) * page_size;
    const body: Page<Role> = {
      items: items.slice(start, start + page_size),
      page_no,
      page_size,
      total: items.length,
    };
    return HttpResponse.json(body);
  });
}

beforeEach(() => {
  server.use(rolesHandler());
  // jsdom lacks clipboard; SnowflakeId copy uses it.
  Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
});

describe('Roles page — render & ladder (06-roles.md §六.1)', () => {
  it('renders the title, primary action and the daemon-expanded effective sets', async () => {
    renderRoles();
    expect(screen.getByRole('heading', { name: '角色 Roles' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /新建角色/ })).toBeInTheDocument();

    // operator's row shows the daemon-expanded set {observe,query,mutate,execute}.
    const row = await findRoleRow('operator');
    expect(row.getByText('observe')).toBeInTheDocument();
    expect(row.getByText('query')).toBeInTheDocument();
    expect(row.getByText('mutate')).toBeInTheDocument();
    expect(row.getByText('execute')).toBeInTheDocument();
    // operator does NOT carry manage (that's maintainer).
    expect(row.queryByText('manage')).not.toBeInTheDocument();
    // 继承自 column shows the parent name.
    expect(row.getByText('observer')).toBeInTheDocument();
  });

  it('log-observer is a narrow role: {observe} only, no query, no inheritance', async () => {
    renderRoles();
    const row = await findRoleRow('log-observer');
    expect(row.getByText('observe')).toBeInTheDocument();
    expect(row.queryByText('query')).not.toBeInTheDocument();
    // ver column shows version 0 for the fresh role.
    expect(row.getByText('0')).toBeInTheDocument();
  });

  it('renders the read-only LadderGraph with the inherits edges and the destroy footnote', async () => {
    renderRoles();
    const ladder = await screen.findByRole('region', { name: '继承阶梯' });
    const l = within(ladder);
    // Two inherits arrows between the three rungs.
    expect(l.getAllByLabelText('inherits').length).toBeGreaterThanOrEqual(2);
    // The fixed fact footnote.
    expect(l.getByText(/destroy 不进任何角色/)).toBeInTheDocument();
    // The floating narrow role appears in the ladder's narrow section.
    expect(l.getByText('log-observer')).toBeInTheDocument();
  });
});

describe('Roles page — snowflake id discipline (§七)', () => {
  it('renders the role id middle-truncated and copies the FULL string (no precision loss)', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    renderRoles();
    // The full id is preserved in the title attr, truncated in the cell.
    const full = '7300000000000001001';
    expect(Number(full) > Number.MAX_SAFE_INTEGER).toBe(true);
    const idCell = await screen.findByTitle(full);
    expect(idCell).toHaveTextContent('7300…1001');
    // Copy emits the EXACT string, never the rounded number.
    const copyBtns = screen.getAllByRole('button', { name: '复制完整 id' });
    fireEvent.click(copyBtns[0]!);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(full));
    expect(String(Number(full))).not.toBe(full); // sanity: it WOULD round.
  });
});

describe('Roles page — filters & pagination (§七 钳制)', () => {
  it('filters the current page by name', async () => {
    renderRoles();
    await findRoleRow('observer');
    fireEvent.change(screen.getByLabelText('按名称筛选'), { target: { value: 'maint' } });
    expect(getTable().getByText('maintainer')).toBeInTheDocument();
    // log-observer no longer in the table (ladder still shows it, so scope to table).
    expect(getTable().queryByText('log-observer')).not.toBeInTheDocument();
  });

  it('filters by verb using the daemon effective set (manage ⇒ only maintainer)', async () => {
    renderRoles();
    await findRoleRow('maintainer');
    fireEvent.change(screen.getByLabelText('按动词筛选'), { target: { value: 'manage' } });
    // Only maintainer carries manage in its effective set → only its name row.
    const names = within(screen.getByRole('table'))
      .getAllByRole('row')
      .map((tr) => tr.querySelector('td')?.textContent?.trim())
      .filter(Boolean);
    expect(names).toContain('maintainer');
    expect(names).not.toContain('observer');
    expect(names).not.toContain('operator');
    expect(names).not.toContain('log-observer');
  });

  it('clamps page_size to ≤200 on the outgoing request (DB_PAGINATION_MANDATORY)', async () => {
    let lastSize: string | null = null;
    server.use(
      http.get('/v1/roles', ({ request }) => {
        lastSize = new URL(request.url).searchParams.get('page_size');
        return HttpResponse.json({ items: ROLES, page_no: 1, page_size: 200, total: ROLES.length });
      }),
    );
    renderRoles();
    await findRoleRow('observer');
    // The first request already carries a legal default size (20).
    expect(Number(lastSize)).toBeLessThanOrEqual(200);
    expect(Number(lastSize)).toBeGreaterThanOrEqual(1);
  });
});

describe('Roles page — three states fail-closed (§六.2)', () => {
  it('shows a loading skeleton before data resolves', async () => {
    renderRoles();
    expect(screen.getAllByRole('status').length).toBeGreaterThan(0);
    await findRoleRow('observer'); // settle
  });

  it('ERROR: replaces both table and ladder with ErrorState — no fake roles, no stale ladder', async () => {
    server.use(
      http.get('/v1/roles', () =>
        HttpResponse.json({ error: { code: 'control_unreachable', message: '控制面不可达' } }, { status: 503 }),
      ),
    );
    renderRoles();
    const alerts = await screen.findAllByRole('alert');
    expect(alerts.some((a) => /控制面不可达/.test(a.textContent ?? ''))).toBe(true);
    // fail-closed: no role names leaked, and no ladder region rendered.
    expect(screen.queryByText('observer')).not.toBeInTheDocument();
    expect(screen.queryByRole('region', { name: '继承阶梯' })).not.toBeInTheDocument();
  });

  it('EMPTY: legal 0-row response shows EmptyState, not an error', async () => {
    server.use(rolesHandler([]));
    renderRoles();
    expect(await screen.findByText('尚无任何角色')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
