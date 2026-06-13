import type { ReactElement } from 'react';
import { render } from '@testing-library/react';
import { QueryClientProvider, QueryClient } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';

/**
 * Probe that mirrors the router's current location into the DOM so navigation
 * assertions don't depend on window.location (MemoryRouter doesn't touch it).
 */
function LocationProbe() {
  const loc = useLocation();
  return (
    <span data-testid="location" data-pathname={loc.pathname} data-search={loc.search} />
  );
}

/**
 * Test harness: fresh QueryClient per render (no cross-test cache bleed) +
 * MemoryRouter (the page emits navigations/links). MSW is already on globally
 * via src/test/setup.ts; individual tests override /v1/denials/summary with
 * server.use() to fabricate empty/error/paging scenarios WITHOUT touching the
 * shared handlers.
 */
export function renderPage(ui: ReactElement, initialPath = '/denials') {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[initialPath]}>
        {ui}
        <LocationProbe />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}
