/**
 * Mode page presentation facts (本页特有，非共享原子).
 *
 * Discipline (11-mode.md §七 / §三):
 *  - The effective mode (`global.meet(scoped)` = strictest) is computed in core
 *    and arrives on `ModeStateRow.effective_mode`. The frontend NEVER recomputes
 *    it. The only thing here is, given local vs effective, label WHERE the
 *    effective value came from (`←本地` when the override won, `←全局` when the
 *    inherited global value won) — a presentation hint, not an authorization
 *    decision.
 *  - The per-mode narrowing text is a FACTUAL statement of which verbs each mode
 *    admits (core's built-in constant table), never advice (设计原则 2).
 */

import type { Mode } from '../../api/types';

/** Source of the effective mode for the inheritance annotation. */
export type EffectiveSource = 'local' | 'global';

/**
 * Where did the effective mode come from for a resource row?
 *  - `local`  : the resource's own override row is the strictest → `←本地`.
 *  - `global` : the inherited global value is the strictest → `←全局`.
 * Pure label logic: if the effective value differs from the local value, the
 * global value must have won; otherwise the local row's value stands.
 */
export function effectiveSource(
  localMode: Mode | null,
  effectiveMode: Mode,
): EffectiveSource {
  if (localMode === null) return 'global';
  return effectiveMode === localMode ? 'local' : 'global';
}

/** Factual narrowing text per mode (core constant-table semantics; not advice). */
export const MODE_NARROWING: Record<Mode, string> = {
  normal: '放行全部动词（无收窄）。',
  observe: '仅放行只读动词：observe / query。',
  maintain: '放行 observe / query / mutate / execute（不含 manage / destroy）。',
  freeze: '拒绝一切动词（含只读）；在飞危险操作将被强制中断。最高危。',
};

/** Human label for a jurisdiction scope (null = global). */
export function scopeLabel(scope: string | null): string {
  return scope ?? 'GLOBAL';
}

/**
 * The confirm word that unlocks a freeze switch (anti-misclick, §四 危险清单 1):
 * the jurisdiction identifier — `GLOBAL` for the global row, else the resource
 * code. NOT the literal word "freeze".
 */
export function freezeConfirmWord(scope: string | null): string {
  return scope ?? 'GLOBAL';
}
