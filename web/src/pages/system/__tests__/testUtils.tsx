import type { ReactElement, ReactNode } from 'react';
import { render } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';

/**
 * Test render wrapper: a fresh QueryClient per render (no cross-test cache),
 * retries off so error states surface immediately and deterministically.
 */
export function renderWithQuery(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0, staleTime: 0 },
      mutations: { retry: false },
    },
  });
  function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  }
  return render(ui, { wrapper: Wrapper });
}
