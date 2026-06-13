import { describe, expect, it } from 'vitest';
import { screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { HealthCard } from '../HealthCard';
import { renderWithProviders } from './harness';

describe('HealthCard', () => {
  it('renders daemon UP / store WRITABLE / policy_rev / capacity from /v1/health', async () => {
    renderWithProviders(<HealthCard />);
    expect(await screen.findByText('UP')).toBeInTheDocument();
    expect(screen.getByText('WRITABLE')).toBeInTheDocument();
    // policy_rev rendered as a string (SnowflakeId), 4187 from the fixture.
    expect(screen.getByText(/4187/)).toBeInTheDocument();
    // capacity watermark 0.12 ⇒ 12%, healthy tone (no "逼近上限").
    expect(screen.getByText('12%')).toBeInTheDocument();
    expect(screen.queryByText(/逼近上限/)).not.toBeInTheDocument();
    const meter = screen.getByRole('meter');
    expect(meter).toHaveAttribute('aria-valuenow', '12');
  });

  it('fail-closed: on a /v1/health error it shows "daemon 不可达" and never a fake UP', async () => {
    server.use(
      http.get('/v1/health', () => HttpResponse.json({ error: { code: 'x', message: 'boom' } }, { status: 503 })),
    );
    renderWithProviders(<HealthCard />);
    expect(await screen.findByText('daemon 不可达')).toBeInTheDocument();
    // No optimistic health: UP / WRITABLE must NOT be rendered on error.
    expect(screen.queryByText('UP')).not.toBeInTheDocument();
    expect(screen.queryByText('WRITABLE')).not.toBeInTheDocument();
  });

  it('turns the capacity bar to a warning ("逼近上限") near the ceiling', async () => {
    server.use(
      http.get('/v1/health', () =>
        HttpResponse.json({
          status: 'up',
          audit_writable: true,
          audit_watermark: 0.93,
          policy_rev: '4187',
          uptime_ms: 1000,
        }),
      ),
    );
    renderWithProviders(<HealthCard />);
    expect(await screen.findByText(/93%/)).toBeInTheDocument();
    expect(screen.getByText(/逼近上限/)).toBeInTheDocument();
  });

  it('shows DOWN (not UP) when the daemon reports a down status', async () => {
    server.use(
      http.get('/v1/health', () =>
        HttpResponse.json({
          status: 'down',
          audit_writable: false,
          audit_watermark: 0.5,
          policy_rev: '4187',
          uptime_ms: 0,
        }),
      ),
    );
    renderWithProviders(<HealthCard />);
    expect(await screen.findByText('DOWN')).toBeInTheDocument();
    expect(screen.getByText('READ-ONLY')).toBeInTheDocument();
    expect(screen.queryByText('UP')).not.toBeInTheDocument();
  });

  it('shows a loading skeleton before data resolves', async () => {
    renderWithProviders(<HealthCard />);
    expect(screen.getByRole('status', { name: '加载中' })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText('UP')).toBeInTheDocument());
  });
});
