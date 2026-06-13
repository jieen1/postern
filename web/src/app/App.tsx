import { QueryClientProvider } from '@tanstack/react-query';
import { BrowserRouter } from 'react-router-dom';
import { AppShell } from './AppShell';
import { AppRoutes } from './routes';
import { queryClient } from './queryClient';

/** Root: providers (router + query) wrapping the shell and route table. */
export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <AppShell>
          <AppRoutes />
        </AppShell>
      </BrowserRouter>
    </QueryClientProvider>
  );
}
