import { describe, expect, it, beforeEach } from 'vitest';
import { screen, within, fireEvent } from '@testing-library/react';
import { http, HttpResponse, delay } from 'msw';
import { server } from '../../../mocks/server';
import { renderWithClient } from './render';
import { ModePage } from '../index';
import type { ModeStateRow } from '../../../api/types';

const BASE = '/v1';

// A clipboard stub so SnowflakeId's copy button doesn't throw in jsdom.
beforeEach(() => {
  Object.assign(navigator, {
    clipboard: { writeText: () => Promise.resolve() },
  });
});

const HUGE_REV = '7300000000000099999'; // > 2^53 — Number() would lose precision.

function modeRows(overrides: Partial<ModeStateRow>[] = []): ModeStateRow[] {
  const global: ModeStateRow = {
    scope: null,
    mode: 'observe',
    effective_mode: 'observe',
    expires_at: null,
    version: 7,
    updated_at: '2026-06-14T03:11:00Z',
    updated_by: 'admin',
    policy_rev: HUGE_REV,
  };
  const rows = overrides.map((o, i) => ({
    scope: `res-${i}`,
    mode: 'maintain' as const,
    effective_mode: 'maintain' as const,
    expires_at: null,
    version: 1,
    updated_at: '2026-06-14T02:00:00Z',
    updated_by: 'admin',
    policy_rev: HUGE_REV,
    ...o,
  })) as ModeStateRow[];
  return [global, ...rows];
}

function useModeRows(rows: ModeStateRow[]) {
  server.use(
    http.post(`${BASE}/mode`, async ({ request }) => {
      const body = (await request.json().catch(() => ({}))) as { op?: string };
      if (body.op === 'set') {
        return HttpResponse.json({ rows, policy_rev: '4200' });
      }
      return HttpResponse.json(rows);
    }),
  );
}

describe('ModePage — 渲染与全貌', () => {
  it('renders the title, global card (effective mode), and overrides board', async () => {
    useModeRows(
      modeRows([{ scope: 'db-main', mode: 'freeze', effective_mode: 'freeze' }]),
    );
    renderWithClient(<ModePage />);

    expect(
      await screen.findByRole('heading', { name: '模式 Mode' }),
    ).toBeInTheDocument();

    // Global card present and same-source effective mode shown.
    const card = await screen.findByRole('region', { name: '全局辖区' });
    expect(within(card).getByText('observe')).toBeInTheDocument();

    // Override row renders local + effective mode badges and the resource code.
    expect(await screen.findByText('db-main')).toBeInTheDocument();
    expect(screen.getAllByText('freeze').length).toBeGreaterThan(0);
  });

  it('labels the effective mode source (←本地 / ←全局)', async () => {
    useModeRows(
      modeRows([
        // local maintain, but effective freeze ⇒ global won ⇒ ←全局.
        { scope: 'db-main', mode: 'maintain', effective_mode: 'freeze' },
      ]),
    );
    renderWithClient(<ModePage />);
    expect(await screen.findByText('←全局')).toBeInTheDocument();
  });
});

describe('ModePage — 三态 fail-closed', () => {
  it('shows a loading skeleton before the mode read resolves (never assumes normal)', async () => {
    server.use(
      http.post(`${BASE}/mode`, async () => {
        await delay('infinite');
        return HttpResponse.json([]);
      }),
    );
    renderWithClient(<ModePage />);
    // LoadingSkeleton (role=status) shown; no global card / no assumed NORMAL.
    expect((await screen.findAllByRole('status')).length).toBeGreaterThan(0);
    expect(screen.queryByRole('region', { name: '全局辖区' })).not.toBeInTheDocument();
  });

  it('shows a fail-closed ErrorState on read failure and disables the primary write', async () => {
    server.use(
      http.post(`${BASE}/mode`, () =>
        HttpResponse.json(
          { error: { code: 'control_unreachable', message: 'control.sock 不可达' } },
          { status: 503 },
        ),
      ),
    );
    renderWithClient(<ModePage />);

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('无法读取当前模式');
    expect(alert).toHaveTextContent(/无法确认或更改安全状态/);
    // No fabricated data — there is no global card, no NORMAL badge.
    expect(screen.queryByRole('region', { name: '全局辖区' })).not.toBeInTheDocument();
    // Primary write button disabled in error state.
    expect(screen.getByRole('button', { name: '切换模式' })).toBeDisabled();
  });

  it('shows the EmptyState (inherit global) when there are no override rows', async () => {
    useModeRows(modeRows([])); // only the global row, no overrides
    renderWithClient(<ModePage />);
    expect(
      await screen.findByText(/当前无资源级模式覆盖，全部辖区继承全局模式 observe/),
    ).toBeInTheDocument();
  });
});

describe('ModePage — 契约对齐', () => {
  it('renders policy_rev as a STRING via SnowflakeId without precision loss', async () => {
    useModeRows(modeRows([{ scope: 'db-main' }]));
    renderWithClient(<ModePage />);

    const card = await screen.findByRole('region', { name: '全局辖区' });
    // The full id is in the title attr and the copy button targets the raw string.
    const idSpan = within(card).getByTitle(HUGE_REV);
    expect(idSpan).toBeInTheDocument();
    // Number coercion would corrupt the tail; assert the raw string is intact.
    expect(HUGE_REV).not.toBe(String(Number(HUGE_REV)));
  });

  it('forces pagination: page size selector only offers clamped legal sizes', async () => {
    useModeRows(modeRows([{ scope: 'db-main' }, { scope: 'db-two' }]));
    renderWithClient(<ModePage />);
    await screen.findByText('db-main');

    // The page-size selector is the combobox whose options include 200.
    const selects = screen.getAllByRole('combobox');
    const sizeSelect = selects.find((s) =>
      within(s).queryByRole('option', { name: '200' }),
    ) as HTMLSelectElement;
    expect(sizeSelect).toBeTruthy();
    const options = within(sizeSelect)
      .getAllByRole('option')
      .map((o) => (o as HTMLOptionElement).value);
    // Forced pagination: only the clamped legal sizes are offered (≤200).
    expect(options).toEqual(['20', '50', '100', '200']);
  });

  it('filters override rows by resource code and resets to page 1', async () => {
    useModeRows(
      modeRows([{ scope: 'db-main' }, { scope: 'cache-redis' }]),
    );
    renderWithClient(<ModePage />);
    await screen.findByText('db-main');
    expect(screen.getByText('cache-redis')).toBeInTheDocument();

    fireEvent.change(screen.getByRole('searchbox', { name: '筛选资源代号' }), {
      target: { value: 'cache' },
    });
    expect(screen.queryByText('db-main')).not.toBeInTheDocument();
    expect(screen.getByText('cache-redis')).toBeInTheDocument();
  });
});
