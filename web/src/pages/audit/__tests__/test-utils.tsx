import type { ReactElement } from 'react';
import { render } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';

/**
 * Render a component inside a fresh QueryClient (no retries / no cache leak
 * between tests, so per-test MSW overrides are deterministic).
 */
export function renderWithQuery(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, staleTime: 0 } },
  });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}
