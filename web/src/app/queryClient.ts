import { QueryClient } from '@tanstack/react-query';

/**
 * Shared React Query client. Conservative defaults aligned with §6 fail-closed:
 * no aggressive refetch-on-focus, a single retry (the control plane is local),
 * and short stale time so policy changes surface promptly.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: false,
      staleTime: 5_000,
    },
  },
});
