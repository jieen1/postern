import { QueryClientProvider } from '@tanstack/react-query';
import { BrowserRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { AppShell } from './AppShell';
import { AppRoutes } from './routes';
import { queryClient } from './queryClient';
import { ErrorBoundary } from '../components/ErrorBoundary';

/** Reset the error boundary on navigation, so a crash on one page does not
 * stick after the user moves to another (keyed by route). */
function RoutedBoundary({ children }: { children: ReactNode }) {
  const { pathname } = useLocation();
  return <ErrorBoundary key={pathname}>{children}</ErrorBoundary>;
}

/** Root: providers (router + query) wrapping the shell and route table. */
export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <AppShell>
          <RoutedBoundary>
            <AppRoutes />
          </RoutedBoundary>
        </AppShell>
      </BrowserRouter>
    </QueryClientProvider>
  );
}
