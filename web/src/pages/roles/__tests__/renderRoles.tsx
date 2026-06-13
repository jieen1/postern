/**
 * Test harness for the Roles page: wraps it in a fresh QueryClient (retry off
 * so error states surface immediately) + a MemoryRouter. MSW is already on
 * globally via src/test/setup.ts; per-test handlers override via server.use().
 */

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import { render, screen, within, type BoundFunctions, type queries } from '@testing-library/react';
import { RolesPage } from '../index';

export function renderRoles() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <RolesPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The DataTable's `<table>` — role names appear both here and in the
 * LadderGraph, so table-scoped queries disambiguate. */
export function getTable(): BoundFunctions<typeof queries> {
  return within(screen.getByRole('table'));
}

/** Wait for a role row to appear in the TABLE (not the ladder) and return its
 * scoped queries. Disambiguates by the NAME column (first cell): a role name
 * can also appear in another row's 继承自 cell. */
export async function findRoleRow(name: string): Promise<BoundFunctions<typeof queries>> {
  // settle: wait until the name appears in a first-column (name) cell.
  await getTable().findAllByText(name);
  const table = screen.getByRole('table');
  const rows = within(table).getAllByRole('row');
  for (const tr of rows) {
    const firstCell = tr.querySelector('td');
    if (firstCell && firstCell.textContent?.trim() === name) {
      return within(tr);
    }
  }
  throw new Error(`name-cell row for ${name} not found`);
}
