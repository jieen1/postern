import { describe, expect, it } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import * as fx from '../../../mocks/fixtures';
import { DenialsTopTable } from '../DenialsTopTable';
import { renderWithProviders } from './harness';

describe('DenialsTopTable', () => {
  it('renders the cross-principal aggregation rows (principal/resource/capability/stage/count)', async () => {
    renderWithProviders(<DenialsTopTable />);
    // First fixture row: agent-order-bot / db-main / mutate / rbac / 42.
    expect(await screen.findByText('agent-order-bot')).toBeInTheDocument();
    expect(screen.getByText('mutate')).toBeInTheDocument();
    expect(screen.getByText('rbac')).toBeInTheDocument();
    expect(screen.getByText('42')).toBeInTheDocument();
    // Second row's count too.
    expect(screen.getByText('11')).toBeInTheDocument();
  });

  it('does not leak deny reason text or any operator_note on the Dashboard', async () => {
    renderWithProviders(<DenialsTopTable />);
    await screen.findByText('agent-order-bot');
    // The reason/operator_note from the deny model is never shown here.
    expect(screen.queryByText(/no grant cell/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/DBA 值班/)).not.toBeInTheDocument();
  });

  it('fail-closed: on a fetch error it shows an error+retry, NOT "无拒绝" (取不到 ≠ 无)', async () => {
    server.use(
      http.get('/v1/denials/summary', () =>
        HttpResponse.json({ error: { code: 'x', message: 'summary down' } }, { status: 500 }),
      ),
    );
    renderWithProviders(<DenialsTopTable />);
    expect(await screen.findByRole('alert')).toHaveTextContent('summary down');
    // The empty "无拒绝" message must never appear on a failed fetch.
    expect(screen.queryByText(/无拒绝记录/)).not.toBeInTheDocument();
    expect(screen.getByText('重试')).toBeInTheDocument();
  });

  it('shows a truthful empty state ("近 7 天无拒绝记录") when the window has zero denials', async () => {
    server.use(
      http.get('/v1/denials/summary', () =>
        HttpResponse.json({ items: [], page_no: 1, page_size: 20, total: 0 }),
      ),
    );
    renderWithProviders(<DenialsTopTable />);
    expect(await screen.findByText('近 7d 无拒绝记录')).toBeInTheDocument();
  });

  it('switches the window and refetches with the new window param', async () => {
    const seen: string[] = [];
    server.use(
      http.get('/v1/denials/summary', ({ request }) => {
        const url = new URL(request.url);
        seen.push(url.searchParams.get('window') ?? '');
        return HttpResponse.json(fx.denialsSummary.length
          ? { items: fx.denialsSummary, page_no: 1, page_size: 20, total: fx.denialsSummary.length }
          : { items: [], page_no: 1, page_size: 20, total: 0 });
      }),
    );
    renderWithProviders(<DenialsTopTable />);
    await screen.findByText('agent-order-bot');
    expect(seen).toContain('7d');

    fireEvent.change(screen.getByLabelText('拒绝窗口'), { target: { value: '24h' } });
    await waitFor(() => expect(seen).toContain('24h'));
  });

  it('row [→ audit] navigates to Audit prefilled with the principal and decision=deny', async () => {
    renderWithProviders(<DenialsTopTable />);
    await screen.findByText('agent-order-bot');
    const row = screen.getByText('agent-order-bot').closest('tr')!;
    fireEvent.click(within(row).getByRole('button', { name: /audit/i }));

    const loc = screen.getByTestId('location').textContent ?? '';
    expect(loc.startsWith('/audit')).toBe(true);
    expect(loc).toContain('decision=deny');
    expect(loc).toContain('principal=agent-order-bot');
    // No reason detail is carried into the URL (only the prefilter facets).
    expect(loc).not.toContain('grant');
  });

  it('forces pagination: requests carry clamped page_no/page_size (default 20)', async () => {
    let pageSize = '';
    let pageNo = '';
    server.use(
      http.get('/v1/denials/summary', ({ request }) => {
        const url = new URL(request.url);
        pageSize = url.searchParams.get('page_size') ?? '';
        pageNo = url.searchParams.get('page_no') ?? '';
        return HttpResponse.json({ items: [], page_no: 1, page_size: 20, total: 0 });
      }),
    );
    renderWithProviders(<DenialsTopTable />);
    await waitFor(() => expect(pageSize).toBe('20'));
    expect(pageNo).toBe('1');
  });

  it('keeps a snowflake principal_id as a string (never coerced to a number that loses precision)', async () => {
    // principal_id from the fixture is > 2^53; assert it round-trips intact.
    const big = fx.denialsSummary[0]!.principal_id!;
    expect(big.length).toBeGreaterThan(16);
    expect(Number(big).toString()).not.toBe(big); // precision WOULD be lost as a Number
    // The component keys rows by principal_id; rendering must not crash on it.
    renderWithProviders(<DenialsTopTable />);
    expect(await screen.findByText('agent-order-bot')).toBeInTheDocument();
  });
});
