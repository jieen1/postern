import { describe, expect, it, vi } from 'vitest';
import { screen } from '@testing-library/react';
import { ExpiringGrants } from '../ExpiringGrants';
import { renderWithProviders } from './harness';

describe('ExpiringGrants (jump-only card)', () => {
  it('renders guidance + a link to Grants, and enumerates NO subject/resource/expiry', () => {
    renderWithProviders(<ExpiringGrants />);
    expect(screen.getByText('临时授权将到期')).toBeInTheDocument();
    const link = screen.getByText('前往 Grants →').closest('a');
    expect(link).toHaveAttribute('href', '/grants');
    // It must not inline any concrete temp-grant row (per-principal endpoint
    // cannot enumerate cross-principal near-expiry); db-main appears nowhere.
    expect(screen.queryByText('db-main')).not.toBeInTheDocument();
    expect(screen.queryByText(/mutate/)).not.toBeInTheDocument();
  });

  it('issues NO control-plane request (no GET /v1/grants from the Dashboard)', () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    renderWithProviders(<ExpiringGrants />);
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });
});
