import type { ReactElement, ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { render } from '@testing-library/react';

/**
 * Test harness for Dashboard cards/pages: a fresh QueryClient (no retries, no
 * cross-test leakage) + a MemoryRouter. A hidden <LocationProbe/> surfaces the
 * current path+search so navigation assertions can read where a jump landed.
 */
export function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="location">{loc.pathname + loc.search}</div>;
}

export function renderWithProviders(
  ui: ReactElement,
  { route = '/' }: { route?: string } = {},
) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0, staleTime: 0 },
      mutations: { retry: false },
    },
  });

  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>
          <Routes>
            <Route
              path="*"
              element={
                <>
                  {children}
                  <LocationProbe />
                </>
              }
            />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    );
  }

  return { queryClient, ...render(ui, { wrapper: Wrapper }) };
}
