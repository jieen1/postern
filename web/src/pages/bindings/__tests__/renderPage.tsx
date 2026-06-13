import type { ReactElement } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import { render } from '@testing-library/react';

/**
 * Render a page under fresh QueryClient + MemoryRouter providers. A new client
 * per render keeps the per-test MSW overrides (server.use) from leaking via the
 * query cache. Retries off so error-state tests resolve fast and deterministically.
 */
export function renderWithProviders(ui: ReactElement) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0, staleTime: 0 },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}
