/**
 * Local write/discover hooks for the Resources page.
 *
 * The shared scaffold exposes `useResources` (read) but no resource mutation or
 * discover hook, so these live in the page directory (reported as "suggest
 * shared"). They reuse the shared endpoint functions + qk so cache invalidation
 * stays aligned with `useResources`.
 *
 * Contract notes:
 *  - `POST /v1/resources` is optimistic-locked; a 409 surfaces as ConflictError
 *    (from the shared client) and the caller prompts refresh-and-retry.
 *  - `discover` is half-write/half-read (it really connects through transport);
 *    it returns a CapabilitySurface fact set and is NOT authorization.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { endpoints } from '../../../api';
import type { CapabilitySurface, WriteAck } from '../../../api/types';

/** POST /v1/resources — declare/revise (enable/disable folds in here too). */
export function usePostResource() {
  const qc = useQueryClient();
  return useMutation<WriteAck, Error, unknown>({
    mutationFn: (body: unknown) => endpoints.postResource(body),
    // Invalidate the whole resources collection (every page/filter key).
    onSuccess: () => qc.invalidateQueries({ queryKey: ['resources'] }),
  });
}

/** POST /v1/resources/{code}/discover — capability-surface probe (fact, not grant). */
export function useDiscoverResource() {
  return useMutation<CapabilitySurface, Error, string>({
    mutationFn: (code: string) => endpoints.discoverResource(code),
  });
}
