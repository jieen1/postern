import { useEffect, useState } from 'react';
import { useForm, type Resolver } from 'react-hook-form';
import { z } from 'zod';
import { FormDrawer } from '../../components/FormDrawer';
import { ConfirmDialog } from '../../components/ConfirmDialog';
import { ModeBadge } from '../../components/ModeBadge';
import { ResourceCodeBadge } from '../../components/ResourceCodeBadge';
import { SnowflakeId } from '../../components/SnowflakeId';
import { MODES, type Mode, type ModeSetRequest } from '../../api/types';
import { MODE_NARROWING, freezeConfirmWord, scopeLabel } from './mode-facts';

/**
 * Mode switch write flow (11-mode.md §四 统一写流程):
 *   form (ModeSelector + TTL) → summary preview (旧→新 + 收窄影响 + 期望 version)
 *   → danger confirm (freeze 需输入辖区标识 / 其余勾选确认) → submit POST /v1/mode
 *   (携带 version) → 失效刷新 / 成功 / 失败(不改本地视图) / 409.
 *
 * The drawer never submits on its own; it calls `onSubmit(req)` and the page
 * owns the mutation + invalidation + 409 surface. Submit-state (pending / 409 /
 * error) is passed back in so the footer reflects the real write outcome.
 */

const schema = z.object({
  // mode is constrained to the closed 4-value set; ModeSelector is physically
  // limited to these (CONS-18) — Zod also rejects anything off-set.
  mode: z.enum(['normal', 'observe', 'maintain', 'freeze']),
  // TTL minutes: optional; when present must be a positive integer.
  ttlMinutes: z
    .union([z.literal(''), z.coerce.number().int().positive()])
    .optional(),
});

type FormValues = z.infer<typeof schema>;

const resolver: Resolver<FormValues> = (values) => {
  const parsed = schema.safeParse(values);
  if (parsed.success) return { values: parsed.data, errors: {} };
  const errors: Record<string, { type: string; message: string }> = {};
  for (const issue of parsed.error.issues) {
    const key = String(issue.path[0] ?? 'root');
    if (!errors[key]) errors[key] = { type: 'validate', message: issue.message };
  }
  return { values: {}, errors };
};

export interface ModeSwitchTarget {
  /** Resource code, or null for the global jurisdiction. */
  scope: string | null;
  /** The current mode on this jurisdiction (prefilled). */
  currentMode: Mode;
  /** Expected optimistic-lock version. */
  version: number;
  /** Force the initial mode (e.g. "回落 normal" preselects normal). */
  initialMode?: Mode;
}

export interface SubmitState {
  pending: boolean;
  /** True when the last attempt returned a 409 optimistic-lock conflict. */
  conflict: boolean;
  /** Verbatim error message of the last failed attempt (non-409). */
  error: string | null;
}

export function ModeSwitchDrawer({
  open,
  target,
  submitState,
  onSubmit,
  onClose,
}: {
  open: boolean;
  target: ModeSwitchTarget | null;
  submitState: SubmitState;
  onSubmit: (req: ModeSetRequest) => void;
  onClose: () => void;
}) {
  const { register, watch, setValue, reset, handleSubmit } = useForm<FormValues>({
    resolver,
    defaultValues: { mode: 'normal', ttlMinutes: '' },
  });

  const [confirmOpen, setConfirmOpen] = useState(false);
  const [pendingReq, setPendingReq] = useState<ModeSetRequest | null>(null);

  // Re-seed the form whenever the drawer opens onto a (possibly new) target.
  useEffect(() => {
    if (open && target) {
      reset({ mode: target.initialMode ?? target.currentMode, ttlMinutes: '' });
      setConfirmOpen(false);
      setPendingReq(null);
    }
  }, [open, target, reset]);

  if (!open || !target) return null;

  const selectedMode = (watch('mode') ?? target.currentMode) as Mode;
  const ttlRaw = watch('ttlMinutes');
  const ttlMinutes =
    ttlRaw === '' || ttlRaw === undefined ? null : Number(ttlRaw);
  const ttlMs = ttlMinutes && ttlMinutes > 0 ? ttlMinutes * 60000 : null;

  const isFreeze = selectedMode === 'freeze';
  const isFallback = selectedMode === 'normal';
  const scopeName = scopeLabel(target.scope);

  // Build the request the summary previews and confirm submits.
  function buildReq(): ModeSetRequest {
    return {
      scope: target!.scope,
      mode: selectedMode,
      ttl_ms: ttlMs,
      version: target!.version,
    };
  }

  function onPreviewSubmit() {
    setPendingReq(buildReq());
    setConfirmOpen(true);
  }

  function confirmAndSubmit() {
    setConfirmOpen(false);
    if (pendingReq) onSubmit(pendingReq);
  }

  const confirmWord = isFreeze ? freezeConfirmWord(target.scope) : undefined;

  return (
    <FormDrawer
      open={open}
      title="切换模式"
      onClose={onClose}
      footer={
        <div className="flex flex-col gap-2">
          {submitState.conflict && (
            <div role="alert" className="rounded-card border border-warn/50 bg-warn/5 px-3 py-2 text-xs text-warn">
              他人已改该辖区模式（乐观锁冲突）。请关闭并刷新重读最新 version 再试，
              本页不会静默重试覆盖。
            </div>
          )}
          {submitState.error && (
            <div role="alert" className="rounded-card border border-deny/50 bg-deny/5 px-3 py-2 font-mono text-xs text-deny">
              {submitState.error}
            </div>
          )}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              onClick={onClose}
              className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
            >
              取消
            </button>
            <button
              type="button"
              onClick={handleSubmit(onPreviewSubmit)}
              disabled={submitState.pending}
              className="rounded-card border border-deny/60 bg-deny/10 px-3 py-1.5 text-sm text-deny hover:enabled:bg-deny/20 disabled:opacity-50"
            >
              {submitState.pending ? '提交中…' : '确认切换'}
            </button>
          </div>
        </div>
      }
    >
      <div className="flex flex-col gap-4">
        {/* 作用域 Scope (read-only — preselected by caller) */}
        <div>
          <div className="mb-1 text-sm font-medium">作用域 Scope</div>
          {target.scope === null ? (
            <span className="text-sm">全局 (Global)</span>
          ) : (
            <ResourceCodeBadge code={target.scope} />
          )}
        </div>

        {/* 目标模式 Mode — closed 4-value set */}
        <div>
          <div className="mb-1 text-sm font-medium">目标模式 Mode</div>
          <div role="radiogroup" aria-label="目标模式" className="flex flex-wrap gap-2">
            {MODES.map((m) => (
              <button
                key={m}
                type="button"
                role="radio"
                aria-checked={selectedMode === m}
                onClick={() => setValue('mode', m, { shouldValidate: true })}
                className={
                  'rounded-card border px-3 py-2 ' +
                  (selectedMode === m
                    ? 'border-info bg-surface-2'
                    : 'border-border hover:bg-surface-2')
                }
              >
                <ModeBadge mode={m} />
              </button>
            ))}
          </div>
          <p className="mt-2 rounded-card border border-border bg-surface-2 px-3 py-2 text-xs text-text-muted">
            {MODE_NARROWING[selectedMode]}
          </p>
        </div>

        {/* TTL (optional) */}
        <label className="flex flex-col gap-1 text-sm">
          <span className="font-medium">TTL（分钟，留空=长期）</span>
          <input
            type="number"
            min={1}
            placeholder="留空=长期，到期由 sweeper 回落上层默认"
            className="w-40 rounded-card border border-border bg-bg px-2 py-1"
            {...register('ttlMinutes')}
          />
        </label>

        {/* 摘要预览 (submit-time fact) */}
        <div className="rounded-card border border-border bg-surface-2 p-3 text-sm">
          <div className="mb-2 text-xs font-medium uppercase tracking-wide text-text-muted">
            摘要预览
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <span>辖区 {scopeName}:</span>
            <ModeBadge mode={target.currentMode} />
            <span aria-hidden>→</span>
            <ModeBadge mode={selectedMode} />
          </div>
          <div className="mt-1 text-xs text-text-muted">
            TTL: {ttlMs ? `${ttlMinutes} 分钟（到期 sweeper 自动回落上层默认）` : '长期'}
          </div>
          <div className="mt-1 text-xs text-text-muted">影响: {MODE_NARROWING[selectedMode]}</div>
          {isFreeze && ttlMs && (
            <div className="mt-1 text-xs text-warn">
              附短 TTL 的 freeze：到期将自动解冻回落，请确认这符合预期。
            </div>
          )}
          {isFallback && (
            <div className="mt-1 text-xs text-text-muted">
              解除限制经显式切换留痕（非翻 enable_flag）：将放宽至 normal。
            </div>
          )}
          <div className="mt-2 inline-flex items-center gap-1 text-xs text-text-muted">
            期望 version（乐观锁）:{' '}
            <SnowflakeId id={String(target.version)} />
          </div>
        </div>
      </div>

      <ConfirmDialog
        open={confirmOpen}
        danger
        title={
          isFreeze
            ? `切到 FREEZE — ${scopeName}（最高危）`
            : isFallback
              ? `回落 normal — ${scopeName}`
              : `切换模式 — ${scopeName}`
        }
        body={
          isFreeze ? (
            <span>
              将拒绝该辖区一切动词（含只读），在飞危险操作将被强制中断。输入辖区标识
              确认以防误触。
            </span>
          ) : isFallback ? (
            <span>将放宽该辖区至 normal（解除限制）。此操作留 mode_change 审计痕迹。</span>
          ) : (
            <span>{MODE_NARROWING[selectedMode]}</span>
          )
        }
        confirmWord={confirmWord}
        confirmLabel={isFreeze ? '确认冻结' : '确认切换'}
        onConfirm={confirmAndSubmit}
        onCancel={() => setConfirmOpen(false)}
      />
    </FormDrawer>
  );
}
