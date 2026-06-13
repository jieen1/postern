import { describe, expect, it } from 'vitest';
import { screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import * as fx from '../../../mocks/fixtures';
import { ModePanel } from '../ModePanel';
import { renderWithProviders } from './harness';

describe('ModePanel', () => {
  it('renders the global mode badge + per-resource overrides with a TTL', async () => {
    renderWithProviders(<ModePanel />);
    // Global row: normal (from fixture mode_state). Badge text is the mode word.
    expect(await screen.findByText('normal')).toBeInTheDocument();
    // Override row for db-main at maintain (resource shown only as a code).
    expect(screen.getByText('db-main')).toBeInTheDocument();
    expect(screen.getByText('maintain')).toBeInTheDocument();
    // Override count header reflects the single override.
    expect(screen.getByText('资源覆盖 (1)')).toBeInTheDocument();
  });

  it('fail-closed: a fetch error shows "模式状态未知" and NEVER defaults to NORMAL', async () => {
    server.use(
      http.post('/v1/mode', () =>
        HttpResponse.json({ error: { code: 'x', message: 'down' } }, { status: 500 }),
      ),
    );
    renderWithProviders(<ModePanel />);
    expect(await screen.findByText('模式状态未知')).toBeInTheDocument();
    // Uncertain mode is not rendered as the unrestricted "normal".
    expect(screen.queryByText('normal')).not.toBeInTheDocument();
  });

  it('shows "无资源级模式覆盖" when only the global jurisdiction is set', async () => {
    server.use(
      http.post('/v1/mode', () =>
        HttpResponse.json([
          {
            scope: null,
            mode: 'normal',
            effective_mode: 'normal',
            expires_at: null,
            version: 1,
            updated_at: null,
            updated_by: null,
            policy_rev: '4187',
          },
        ]),
      ),
    );
    renderWithProviders(<ModePanel />);
    expect(await screen.findByText('无资源级模式覆盖')).toBeInTheDocument();
    expect(screen.getByText('资源覆盖 (0)')).toBeInTheDocument();
  });

  it('renders the effective_mode for the global row (strictest meet), not the raw mode', async () => {
    server.use(
      http.post('/v1/mode', () =>
        HttpResponse.json([
          {
            scope: null,
            mode: 'normal',
            effective_mode: 'freeze',
            expires_at: null,
            version: 9,
            updated_at: null,
            updated_by: null,
            policy_rev: '4187',
          },
        ]),
      ),
    );
    renderWithProviders(<ModePanel />);
    // Global shows effective freeze even though the raw global mode is normal.
    expect(await screen.findByText('freeze')).toBeInTheDocument();
  });

  it('links each override row to the Mode page preselecting that resource', async () => {
    renderWithProviders(<ModePanel />);
    await screen.findByText('db-main');
    const link = screen.getByText('db-main').closest('a');
    expect(link).toHaveAttribute('href', expect.stringContaining('/mode?resource=db-main'));
    // sanity: the fixture's db-main override drives this assertion.
    expect(fx.modeState.some((r) => r.scope === 'db-main')).toBe(true);
  });
});
