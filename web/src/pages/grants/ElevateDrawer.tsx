import { useMemo } from 'react';
import { useForm, type Resolver } from 'react-hook-form';
import { FormDrawer } from '../../components';
import { CAPABILITIES } from '../../api/types';
import { formatTime } from '../../lib/format';
import { cn } from '../../lib/cn';
import {
  elevateSchema,
  ttlToMs,
  type ElevateForm,
  type TtlUnit,
} from './elevateSchema';

/**
 * Zod resolver without @hookform/resolvers (not installed): run safeParse and
 * map field errors into RHF's shape. Keeps Zod as the single validation source.
 */
const zodResolver: Resolver<ElevateForm> = (values) => {
  const parsed = elevateSchema.safeParse(values);
  if (parsed.success) return { values: parsed.data, errors: {} };
  const errors: Record<string, { type: string; message: string }> = {};
  for (const issue of parsed.error.issues) {
    const key = String(issue.path[0] ?? 'root');
    if (!errors[key]) errors[key] = { type: issue.code, message: issue.message };
  }
  return { values: {}, errors: errors as never };
};

/**
 * Elevate (临时提权) form drawer — 扩权高危动作.
 * RHF+Zod: resource + capability + TTL(必填,>0). Renders a summary preview
 * (将给 <principal> 在 <resource> 上临时授予 <capability>, 于 <expires_at 预估>
 * 后自动回收), then defers the danger ConfirmDialog to the page.
 */
export function ElevateDrawer({
  open,
  principal,
  resources,
  now,
  submitting,
  conflict,
  errorMessage,
  onClose,
  onSubmit,
}: {
  open: boolean;
  principal: string;
  /** Scope-bounded resource codes (the matrix rows for this principal). */
  resources: string[];
  now: number;
  submitting: boolean;
  conflict: boolean;
  errorMessage: string | null;
  onClose: () => void;
  /** Called with the validated form once the page-level confirm passes. */
  onSubmit: (form: ElevateForm) => void;
}) {
  const {
    register,
    handleSubmit,
    watch,
    formState: { errors },
  } = useForm<ElevateForm>({
    resolver: zodResolver,
    defaultValues: { resource: '', capability: undefined, ttlValue: 30, ttlUnit: 'minute' },
  });

  const resource = watch('resource');
  const capability = watch('capability');
  const ttlValue = watch('ttlValue');
  const ttlUnit = watch('ttlUnit');

  const estimatedExpiry = useMemo(() => {
    const v = Number(ttlValue);
    if (!Number.isFinite(v) || v <= 0) return null;
    return new Date(now + ttlToMs(v, ttlUnit as TtlUnit)).toISOString();
  }, [ttlValue, ttlUnit, now]);

  return (
    <FormDrawer
      open={open}
      title="临时提权 Elevate"
      onClose={onClose}
      footer={
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
          >
            取消
          </button>
          <button
            type="submit"
            form="elevate-form"
            disabled={submitting}
            className="rounded-card bg-deny px-3 py-1.5 text-sm text-white disabled:opacity-40 hover:enabled:brightness-110"
          >
            提权…（危险确认）
          </button>
        </div>
      }
    >
      <form
        id="elevate-form"
        onSubmit={handleSubmit(onSubmit)}
        className="flex flex-col gap-4 text-sm"
      >
        <div className="flex flex-col gap-1">
          <span className="text-text-muted">Principal</span>
          <span className="rounded-card border border-border bg-surface-2 px-2 py-1 font-mono">
            {principal}（锁定）
          </span>
        </div>

        <label className="flex flex-col gap-1">
          <span className="text-text-muted">Resource *</span>
          <select
            {...register('resource')}
            className="rounded-card border border-border bg-surface px-2 py-1"
          >
            <option value="">选择资源…</option>
            {resources.map((code) => (
              <option key={code} value={code}>
                {code}
              </option>
            ))}
          </select>
          {errors.resource && (
            <span className="text-xs text-deny">{errors.resource.message}</span>
          )}
        </label>

        <label className="flex flex-col gap-1">
          <span className="text-text-muted">Capability *</span>
          <select
            {...register('capability')}
            className="rounded-card border border-border bg-surface px-2 py-1"
          >
            <option value="">选择能力…</option>
            {CAPABILITIES.map((cap) => (
              <option key={cap} value={cap}>
                {cap}
              </option>
            ))}
          </select>
          {errors.capability && (
            <span className="text-xs text-deny">{errors.capability.message}</span>
          )}
        </label>

        <div className="flex flex-col gap-1">
          <span className="text-text-muted">TTL *（必填）</span>
          <div className="flex gap-2">
            <input
              type="number"
              min={1}
              {...register('ttlValue')}
              aria-label="TTL 数值"
              className="w-24 rounded-card border border-border bg-surface px-2 py-1"
            />
            <select
              {...register('ttlUnit')}
              aria-label="TTL 单位"
              className="rounded-card border border-border bg-surface px-2 py-1"
            >
              <option value="minute">分钟</option>
              <option value="hour">小时</option>
              <option value="day">天</option>
            </select>
          </div>
          {errors.ttlValue && (
            <span className="text-xs text-deny">{errors.ttlValue.message}</span>
          )}
        </div>

        <section className="rounded-card border border-warn/40 bg-warn/5 px-3 py-2">
          <div className="mb-1 font-medium text-warn">摘要预览</div>
          {resource && capability && estimatedExpiry ? (
            <p className="text-text-muted">
              将给 <span className="font-mono text-text">{principal}</span> 在{' '}
              <span className="font-mono text-text">{resource}</span> 上临时授予{' '}
              <span className="font-mono text-text">{capability}</span>，于{' '}
              <span className="font-mono text-text">{formatTime(estimatedExpiry)}</span>
              （now+TTL）后自动回收。这会<span className="text-deny">扩大</span>该主体的授权面。
            </p>
          ) : (
            <p className="text-text-muted">填写资源、能力与 TTL 后显示摘要。</p>
          )}
        </section>

        {conflict && (
          <p role="alert" className="text-xs text-warn">
            他人已改该授权，请刷新重读最新状态再试（409 Conflict）。
          </p>
        )}
        {!conflict && errorMessage && (
          <p role="alert" className={cn('text-xs text-deny')}>
            提权失败：{errorMessage}
          </p>
        )}
      </form>
    </FormDrawer>
  );
}
