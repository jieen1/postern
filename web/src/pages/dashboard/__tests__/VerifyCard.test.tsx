import { describe, expect, it, vi } from 'vitest';
import { screen } from '@testing-library/react';
import type { VerifyReport } from '../../../api/types';
import { VerifyCard, LAST_VERIFY_KEY } from '../VerifyCard';
import { renderWithProviders } from './harness';

describe('VerifyCard', () => {
  it('shows "尚未运行红队自检" when no report is cached (never a fabricated PASS)', () => {
    renderWithProviders(<VerifyCard />);
    expect(screen.getByText('尚未运行红队自检')).toBeInTheDocument();
    // No fake counts when there is no run.
    expect(screen.queryByText(/PASS/)).not.toBeInTheDocument();
  });

  it('does NOT actively trigger verify (issues no POST /v1/verify)', () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    renderWithProviders(<VerifyCard />);
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });

  it('renders the cached report summary (pass/total + fail count) WITHOUT per-probe gap_note', () => {
    const report: VerifyReport = {
      all_pass: false,
      items: [
        { name: 'scope_out_mutate', pass: true, gap_note: null },
        { name: 'disguised_write', pass: false, gap_note: '伪装写未被拦截' },
        { name: 'session_tamper', pass: true, gap_note: null },
      ],
    };
    const { queryClient, rerender } = renderWithProviders(<VerifyCard />);
    // The Verify page would cache this; the Dashboard only reads it.
    queryClient.setQueryData(LAST_VERIFY_KEY, report);
    rerender(<VerifyCard />);

    expect(screen.getByText('2/3 PASS')).toBeInTheDocument();
    expect(screen.getByText('✗1')).toBeInTheDocument();
    // The verbatim gap_note must NOT surface on the Dashboard summary.
    expect(screen.queryByText('伪装写未被拦截')).not.toBeInTheDocument();
  });

  it('shows all-pass summary without a fail marker when every probe passed', () => {
    const report: VerifyReport = {
      all_pass: true,
      items: [
        { name: 'a', pass: true, gap_note: null },
        { name: 'b', pass: true, gap_note: null },
      ],
    };
    const { queryClient, rerender } = renderWithProviders(<VerifyCard />);
    queryClient.setQueryData(LAST_VERIFY_KEY, report);
    rerender(<VerifyCard />);

    expect(screen.getByText('2/2 PASS')).toBeInTheDocument();
    expect(screen.queryByText(/✗/)).not.toBeInTheDocument();
    expect(screen.getByText('全部防线通过')).toBeInTheDocument();
  });
});
