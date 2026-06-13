/**
 * Page-local data layer for 细则/条件/拒绝指引 (docs/08).
 *
 * The shared `api/hooks.ts` ships READ hooks (useConstraints/useConditions/
 * useDenyNotes/useResources) but no WRITE hooks for these three collections, so
 * the create/edit/delete mutations live here, page-local. Each write goes
 * through the existing endpoint functions (postConstraint/postCondition/
 * postDenyNote) carrying an optimistic-lock `version`; on success it invalidates
 * the matching collection so the table refetches the fresh policy state.
 *
 * Nothing here invents a route or a wire field — bodies follow docs §四/§七.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import {
  postConstraint,
  postCondition,
  postDenyNote,
} from '../../api/endpoints';
import type { Capability, WriteAck } from '../../api/types';

/** Segment discriminator — drives table columns, form and the write endpoint. */
export type Segment = 'constraints' | 'conditions' | 'deny-notes';

/** Constraint kinds an adapter may declare (docs §3.1). */
export const CONSTRAINT_KINDS = [
  'table_allow',
  'column_mask',
  'container_prefix',
  'http_route',
  'command_template',
  'command_class',
  'key_prefix',
  'mask_fields',
] as const;
export type ConstraintKind = (typeof CONSTRAINT_KINDS)[number];

/** Built-in condition predicates (docs §3.1). */
export const CONDITION_PREDICATES = [
  'rate_limit',
  'time_window',
  'mode',
  'ttl',
] as const;
export type ConditionPredicate = (typeof CONDITION_PREDICATES)[number];

/**
 * Which constraint kinds each adapter declares it supports. The kind dropdown
 * narrows to the selected resource's adapter (front-end convenience only — the
 * authoritative narrowing is in the daemon; docs §五 KindMatrixSelect). An
 * unknown adapter falls back to the full set rather than hiding choices.
 */
export const ADAPTER_KIND_MATRIX: Record<string, readonly ConstraintKind[]> = {
  postgres: ['table_allow', 'column_mask', 'mask_fields'],
  http: ['http_route'],
  docker: ['container_prefix', 'command_template', 'command_class'],
};

export function kindsForAdapter(
  adapter: string | undefined,
): readonly ConstraintKind[] {
  if (!adapter) return CONSTRAINT_KINDS;
  return ADAPTER_KIND_MATRIX[adapter] ?? CONSTRAINT_KINDS;
}

// ── Write bodies (carry version for optimistic lock; delete_flag for delete) ──

export interface ConstraintWrite {
  /** Present on edit/delete only. */
  id?: string;
  resource: string;
  capability: Capability;
  kind: string;
  spec: string;
  /** Expected version on edit/delete; absent on create. */
  version?: number;
  delete_flag?: 1;
}

export interface ConditionWrite {
  id?: string;
  /** null = 全资源. */
  resource: string | null;
  /** null = 全动词. */
  capability: Capability | null;
  predicate: string;
  spec: string;
  version?: number;
  delete_flag?: 1;
}

export interface DenyNoteWrite {
  id?: string;
  resource: string;
  capability: Capability;
  note: string;
  version?: number;
  delete_flag?: 1;
}

export function useWriteConstraint() {
  const qc = useQueryClient();
  return useMutation<WriteAck, Error, ConstraintWrite>({
    mutationFn: (body) => postConstraint(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['constraints'] }),
  });
}

export function useWriteCondition() {
  const qc = useQueryClient();
  return useMutation<WriteAck, Error, ConditionWrite>({
    mutationFn: (body) => postCondition(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['conditions'] }),
  });
}

export function useWriteDenyNote() {
  const qc = useQueryClient();
  return useMutation<WriteAck, Error, DenyNoteWrite>({
    mutationFn: (body) => postDenyNote(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['deny-notes'] }),
  });
}

/** A JSON `spec` is valid for front-end purposes iff it parses (docs §六.2). */
export function isParsableJson(text: string): boolean {
  if (text.trim() === '') return false;
  try {
    JSON.parse(text);
    return true;
  } catch {
    return false;
  }
}

/** One-line spec summary for the dense table cell (never re-interprets it). */
export function specSummary(spec: string): string {
  try {
    const v = JSON.parse(spec) as unknown;
    if (Array.isArray(v)) return `[${v.length}] …`;
    if (v && typeof v === 'object') {
      const keys = Object.keys(v as Record<string, unknown>);
      return keys.slice(0, 3).join(', ') + (keys.length > 3 ? ' …' : '');
    }
    return String(v);
  } catch {
    return spec.length > 40 ? spec.slice(0, 40) + '…' : spec;
  }
}
