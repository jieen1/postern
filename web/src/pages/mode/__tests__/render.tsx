import type { ReactElement } from 'react';
import { render } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';

/**
 * Test render helper: wraps a page in a fresh QueryClient (retry off so error
 * states surface immediately, no refetch noise). MSW is on globally via
 * src/test/setup.ts; per-test handlers use server.use(...) to craft scenarios.
 */
export function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0, staleTime: 0 },
      mutations: { retry: false },
    },
  });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}
