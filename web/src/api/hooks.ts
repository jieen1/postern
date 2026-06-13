/**
 * TanStack Query hooks over the endpoint functions.
 *
 * Read hooks are keyed for cache/invalidation; write hooks invalidate the
 * relevant collection and surface 409 conflicts to the form layer. This is the
 * scaffold's shared data access — pages compose these rather than calling
 * fetch directly. Several (health/audit/denials/mode/verify) are wired
 * end-to-end against MSW; the rest are placeholders ready for their pages.
 */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseQueryOptions,
} from '@tanstack/react-query';
import * as api from './endpoints';
import type {
  AuditQuery,
  DenialWindow,
  ElevateRequest,
  ModeSetRequest,
  PageQuery,
  RevokeRequest,
} from './types';

export const qk = {
  health: ['health'] as const,
  audit: (q: AuditQuery) => ['audit', q] as const,
  denials: (w: DenialWindow, p: Partial<PageQuery>) => ['denials', w, p] as const,
  modeState: ['mode-state'] as const,
  grants: (principal?: string) => ['grants', principal ?? null] as const,
  principals: (p: Partial<PageQuery>) => ['principals', p] as const,
  credentials: (p: Partial<PageQuery>, principal?: string) =>
    ['credentials', p, principal ?? null] as const,
  roles: (p: Partial<PageQuery>) => ['roles', p] as const,
  bindings: (p: Partial<PageQuery>) => ['bindings', p] as const,
  constraints: (p: Partial<PageQuery>) => ['constraints', p] as const,
  conditions: (p: Partial<PageQuery>) => ['conditions', p] as const,
  denyNotes: (p: Partial<PageQuery>) => ['deny-notes', p] as const,
  resources: (p: Partial<PageQuery>) => ['resources', p] as const,
  settings: ['settings'] as const,
  approvals: (p: Partial<PageQuery>) => ['approvals', p] as const,
};

// ── Wired end-to-end against MSW ──────────────────────────────────────────────

export function useHealth() {
  return useQuery({
    queryKey: qk.health,
    queryFn: api.getHealth,
    refetchInterval: 15_000,
  });
}

export function useAudit(query: AuditQuery = {}) {
  return useQuery({
    queryKey: qk.audit(query),
    queryFn: () => api.getAudit(query),
  });
}

export function useDenials(window: DenialWindow = '7d', page: Partial<PageQuery> = {}) {
  return useQuery({
    queryKey: qk.denials(window, page),
    queryFn: () => api.getDenialsSummary(window, page),
  });
}

export function useModeState() {
  return useQuery({ queryKey: qk.modeState, queryFn: api.getModeState });
}

export function useVerify() {
  // Verify is an explicit-trigger action, not a passive read.
  return useMutation({ mutationFn: api.runVerify });
}

export function useSetMode() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: ModeSetRequest) => api.setMode(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.modeState }),
  });
}

// ── Placeholder read hooks (ready for their pages) ────────────────────────────

export function useGrants(principal?: string, page: Partial<PageQuery> = {}) {
  return useQuery({
    queryKey: qk.grants(principal),
    queryFn: () => api.getGrants(principal, page),
  });
}

function useCollection<T>(
  key: readonly unknown[],
  fn: () => Promise<T>,
  opts?: Partial<UseQueryOptions<T>>,
) {
  return useQuery({ queryKey: key, queryFn: fn, ...opts });
}

export const usePrincipals = (p: Partial<PageQuery> = {}) =>
  useCollection(qk.principals(p), () => api.getPrincipals(p));
export const useCredentials = (p: Partial<PageQuery> = {}, principal?: string) =>
  useCollection(qk.credentials(p, principal), () => api.getCredentials(p, principal));
export const useRoles = (p: Partial<PageQuery> = {}) =>
  useCollection(qk.roles(p), () => api.getRoles(p));
export const useBindings = (p: Partial<PageQuery> = {}) =>
  useCollection(qk.bindings(p), () => api.getBindings(p));
export const useConstraints = (p: Partial<PageQuery> = {}) =>
  useCollection(qk.constraints(p), () => api.getConstraints(p));
export const useConditions = (p: Partial<PageQuery> = {}) =>
  useCollection(qk.conditions(p), () => api.getConditions(p));
export const useDenyNotes = (p: Partial<PageQuery> = {}) =>
  useCollection(qk.denyNotes(p), () => api.getDenyNotes(p));
export const useResources = (p: Partial<PageQuery> = {}) =>
  useCollection(qk.resources(p), () => api.getResources(p));
export const useSettings = () => useCollection(qk.settings, api.getSettings);
export const useApprovals = (p: Partial<PageQuery> = {}) =>
  useCollection(qk.approvals(p), () => api.getApprovals(p));

// ── Placeholder write hooks ───────────────────────────────────────────────────

export function useElevateGrant() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: ElevateRequest) => api.elevateGrant(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['grants'] }),
  });
}

export function useRevokeGrant() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: RevokeRequest) => api.revokeGrant(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['grants'] }),
  });
}
