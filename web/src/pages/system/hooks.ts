/**
 * Page-local write hooks for the System page.
 *
 * The shared api/hooks.ts ships READ hooks for settings/approvals (useSettings,
 * useApprovals) but no write hooks for this page's four endpoints. Rather than
 * mutate the shared layer, these thin mutation hooks live with the page. Each
 * invalidates the relevant cache on success; 409 ConflictError surfaces to the
 * caller (the form layer renders "他人已改，请刷新重读 version 再改"), never a
 * silent overwrite or silent retry.
 *
 * suggest_shared: useSaveSettings / useAdjudicateApproval / useExportPolicy /
 * useImportPolicy / useShutdown — promote to api/hooks.ts when a second page needs them.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import * as api from '../../api/endpoints';
import { qk } from '../../api/hooks';

/** One key/value/version triple submitted in a single POST /v1/settings. */
export interface SettingWrite {
  key: string;
  value: string;
  version: number;
}

export function useSaveSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (changes: SettingWrite[]) =>
      api.postSettings({ changes }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.settings }),
  });
}

export interface AdjudicateWrite {
  id: string;
  /**
   * Optimistic-lock anchor = the item's `policy_rev` (snowflake-discipline u64,
   * carried verbatim as a string — never Number()-parsed, >2^53 would lose
   * precision and poison stale-write detection).
   */
  version: string;
  decision: 'deny' | 'allow_once';
}

export function useAdjudicateApproval() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: AdjudicateWrite) => api.adjudicateApproval(body),
    // approvals list + settings (policy_rev/state) may change.
    onSuccess: () => qc.invalidateQueries({ queryKey: ['approvals'] }),
  });
}

export function useExportPolicy() {
  return useMutation({ mutationFn: () => api.exportPolicy() });
}

/** Import mode: declarative merge vs. destructive overwrite (覆盖). */
export type ImportMode = 'merge' | 'overwrite';

export interface ImportWrite {
  toml: string;
  mode: ImportMode;
  /** true = validate-only (dry-run); false = apply. */
  dry_run: boolean;
}

export function useImportPolicy() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: ImportWrite) => api.importPolicy(body),
    onSuccess: (_data, vars) => {
      // Only an applied import mutates policy; dry-run never invalidates.
      if (!vars.dry_run) qc.invalidateQueries();
    },
  });
}

export function useShutdown() {
  return useMutation({ mutationFn: () => api.shutdown() });
}
