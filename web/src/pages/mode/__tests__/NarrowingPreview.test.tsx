import { describe, expect, it } from 'vitest';
import { screen, within, fireEvent } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { renderWithClient } from './render';
import { NarrowingPreview } from '../NarrowingPreview';

const BASE = '/v1';

describe('NarrowingPreview — 收窄影响预览（只读 GET /v1/grants）', () => {
  it('contrasts RBAC original verbs vs verbs surviving the observe mode', async () => {
    server.use(
      http.get(`${BASE}/grants`, () =>
        HttpResponse.json({
          your_grants: { 'db-main': ['observe', 'query', 'mutate'] },
          temp_grants: [],
        }),
      ),
    );
    renderWithClient(<NarrowingPreview mode="observe" />);

    fireEvent.click(screen.getByRole('button', { name: /收窄影响预览/ }));

    const rows = await screen.findAllByRole('row');
    const dataRow = rows.find((r) => within(r).queryByText('db-main'));
    expect(dataRow).toBeTruthy();
    // observe admits only observe/query — mutate must NOT appear in "remaining".
    const cells = within(dataRow!).getAllByRole('cell');
    // cells: [resource, original, remaining]
    expect(within(cells[1]!).getByText('mutate')).toBeInTheDocument(); // original
    expect(within(cells[2]!).queryByText('mutate')).not.toBeInTheDocument(); // remaining
    expect(within(cells[2]!).getByText('observe')).toBeInTheDocument();
    expect(within(cells[2]!).getByText('query')).toBeInTheDocument();
  });

  it('shows "全部被拒" for freeze mode (admits no verbs)', async () => {
    server.use(
      http.get(`${BASE}/grants`, () =>
        HttpResponse.json({
          your_grants: { 'db-main': ['observe', 'query'] },
          temp_grants: [],
        }),
      ),
    );
    renderWithClient(<NarrowingPreview mode="freeze" />);
    fireEvent.click(screen.getByRole('button', { name: /收窄影响预览/ }));
    expect(await screen.findByText('全部被拒')).toBeInTheDocument();
  });

  it('fail-closed: a grants read error shows ErrorState, no fabricated rows', async () => {
    server.use(
      http.get(`${BASE}/grants`, () =>
        HttpResponse.json(
          { error: { code: 'forbidden', message: '越权读授权世界' } },
          { status: 403 },
        ),
      ),
    );
    renderWithClient(<NarrowingPreview mode="observe" />);
    fireEvent.click(screen.getByRole('button', { name: /收窄影响预览/ }));

    expect(await screen.findByRole('alert')).toHaveTextContent('无法读取授权世界');
    // No table leaked through the error.
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });

  it('scope-bounded empty: no your_grants → EmptyState, not default-filled', async () => {
    server.use(
      http.get(`${BASE}/grants`, () =>
        HttpResponse.json({ your_grants: {}, temp_grants: [] }),
      ),
    );
    renderWithClient(<NarrowingPreview mode="observe" />);
    fireEvent.click(screen.getByRole('button', { name: /收窄影响预览/ }));

    expect(await screen.findByText('无可对比的授权数据')).toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });
});
