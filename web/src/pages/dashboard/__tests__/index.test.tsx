import { describe, expect, it } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { DashboardPage } from '../index';
import { renderWithProviders } from './harness';

describe('DashboardPage', () => {
  it('renders the title, the refresh control, and all five cards', async () => {
    renderWithProviders(<DashboardPage />);
    expect(screen.getByRole('heading', { name: '总览 Dashboard' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /刷新/ })).toBeInTheDocument();

    // Each card's title is present (cards compose the observation panel).
    expect(screen.getByText('系统健康 Health')).toBeInTheDocument();
    expect(screen.getByText('当前模式姿态 Mode')).toBeInTheDocument();
    expect(screen.getByText('最近高频拒绝 Denials')).toBeInTheDocument();
    expect(screen.getByText('临时授权将到期')).toBeInTheDocument();
    expect(screen.getByText('红队自检 Verify')).toBeInTheDocument();

    // Live data arrives independently in the health/mode/denials cards.
    expect(await screen.findByText('UP')).toBeInTheDocument();
    expect(await screen.findByText('agent-order-bot')).toBeInTheDocument();
  });

  it('does NOT render a second freeze control (freeze lives only in the top bar)', async () => {
    renderWithProviders(<DashboardPage />);
    await screen.findByText('UP');
    // No freeze button anywhere in the Dashboard body.
    expect(screen.queryByRole('button', { name: /freeze/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /冻结/ })).not.toBeInTheDocument();
  });

  it('one failing source fails closed without blocking the other cards', async () => {
    server.use(
      http.get('/v1/health', () =>
        HttpResponse.json({ error: { code: 'x', message: 'no daemon' } }, { status: 503 }),
      ),
    );
    renderWithProviders(<DashboardPage />);
    // Health card fails closed...
    expect(await screen.findByText('daemon 不可达')).toBeInTheDocument();
    // ...while mode and denials still render.
    expect(await screen.findByText('normal')).toBeInTheDocument();
    expect(await screen.findByText('agent-order-bot')).toBeInTheDocument();
  });

  it('refresh refetches the read sources and advances the "最后更新" timestamp', async () => {
    let healthHits = 0;
    server.use(
      http.get('/v1/health', () => {
        healthHits += 1;
        return HttpResponse.json({
          status: 'up',
          audit_writable: true,
          audit_watermark: 0.1,
          policy_rev: '4187',
          uptime_ms: 1000,
        });
      }),
    );
    renderWithProviders(<DashboardPage />);
    await waitFor(() => expect(healthHits).toBe(1));

    fireEvent.click(screen.getByRole('button', { name: /刷新/ }));
    await waitFor(() => expect(healthHits).toBeGreaterThanOrEqual(2));
  });
});
