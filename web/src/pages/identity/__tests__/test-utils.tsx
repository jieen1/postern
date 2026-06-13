import type { ReactElement } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, type RenderResult } from '@testing-library/react';

/**
 * Render a page under a fresh QueryClient (retry off so error states surface
 * immediately and deterministically in tests). MSW is on globally via
 * src/test/setup.ts; per-test overrides use server.use().
 */
export function renderWithQuery(ui: ReactElement): RenderResult & { client: QueryClient } {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0 },
      mutations: { retry: false },
    },
  });
  const result = render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
  return { ...result, client };
}
