/**
 * Bindings page-local API surface.
 *
 * The shared scaffold (`@/api`) exposes `getBindings` (list) and `postBinding`
 * (create) plus the `useBindings` read hook, but has NO write hook for create,
 * NO logical-delete endpoint, and NO expansion-preview probe. Those three are
 * page-specific to bindings (07-bindings.md §2.2/§4), so they live here rather
 * than being added to the shared layer. All reuse the shared `http` client so
 * 409 optimistic-lock conflicts surface as the shared `ConflictError` and
 * non-2xx is fail-closed (any unhandled status throws).
 *
 * `previewExpansion` is the read-only daemon probe (§4.1.2): the SPA holds ZERO
 * expansion logic — it submits the raw spec and renders only what the daemon
 * reports. `deleteBinding` is a logical delete (delete_flag=1) carrying the
 * read-time version for the optimistic lock (§4.2).
 */

import {
  useMutation,
  useQueryClient,
} from '@tanstack/react-query';
import { http } from '@/api/client';
import { postBinding } from '@/api/endpoints';
import { qk } from '@/api/hooks';
import type {
  Capability,
  GrantAction,
  ScopeKind,
  WriteAck,
} from '@/api/types';

/** One resource × verb cell the daemon would grant under this binding. */
export interface PreviewGrantCell {
  resource: string;
  capability: Capability;
  /** Per-cell routing (allow / escalate); tier is daemon-chosen, read-only. */
  action: GrantAction;
  /** Daemon-selected credential tier name (read-only). */
  tier: string | null;
}

/**
 * Read-only expansion preview (`POST /v1/bindings/preview`, doc-specified).
 * The daemon expands the raw spec under the current snapshot and reports the
 * concrete resource set + the (resource × verb) cells it would contribute.
 * `parse_error` is set when the selector syntax is unparseable (异常 C); the
 * daemon never "best-efforts" a wider surface — the SPA renders fail-closed.
 */
export interface ExpansionPreview {
  expanded_resources: string[];
  grants: PreviewGrantCell[];
  /** Verbatim parser error when the selector cannot be parsed; else null. */
  parse_error: string | null;
}

export interface PreviewRequest {
  role: string;
  scope_kind: ScopeKind;
  /** Raw spec text: selector `{all:[...]}` JSON or comma-joined resource codes. */
  scope_spec: string;
}

export const previewExpansion = (body: PreviewRequest) =>
  http.post<ExpansionPreview>('/bindings/preview', body);

export interface CreateBindingRequest {
  principal: string;
  role: string;
  scope_kind: ScopeKind;
  scope_spec: string;
  /** Optimistic-lock version (the principal's read-time version). */
  version: number;
}

/** Logical delete of a binding (delete_flag=1), carrying the read-time version. */
export const deleteBinding = (id: string, version: number) =>
  http.post<WriteAck>(`/bindings/${encodeURIComponent(id)}/delete`, { version });

/** Create a binding; invalidates the bindings collection + grants on success. */
export function useCreateBinding() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateBindingRequest) => postBinding(body),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['bindings'] });
      qc.invalidateQueries({ queryKey: ['grants'] });
    },
  });
}

/** Logical-delete a binding; invalidates bindings + grants on success. */
export function useDeleteBinding() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, version }: { id: string; version: number }) =>
      deleteBinding(id, version),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['bindings'] });
      qc.invalidateQueries({ queryKey: ['grants'] });
    },
  });
}

/** Re-export the shared read-hook query key for test/invalidation convenience. */
export const bindingsKey = qk.bindings;
