import { useForm } from 'react-hook-form';
import {
  PRINCIPAL_KINDS,
  principalCreateSummary,
  principalSchema,
  zodResolver,
  type PrincipalFormValues,
} from './schema';

/**
 * 新建主体表单（§4.1）。RHF + 本地 zod resolver；提交前展示摘要预览；非危险动作
 * （仅登记一个空主体），不走 ConfirmDialog。提交错误/409 由父层经 `submitError`
 * 注入红色提示，本表单不自行揣测后端。
 */
export function PrincipalForm({
  submitting,
  submitError,
  onSubmit,
  onCancel,
}: {
  submitting: boolean;
  submitError: string | null;
  onSubmit: (values: PrincipalFormValues) => void;
  onCancel: () => void;
}) {
  const {
    register,
    handleSubmit,
    watch,
    formState: { errors, isValid },
  } = useForm<PrincipalFormValues>({
    resolver: zodResolver(principalSchema),
    mode: 'onChange',
    defaultValues: { name: '', kind: 'agent' },
  });

  const values = watch();
  const previewReady = isValid && values.name.trim().length > 0;

  return (
    <form
      onSubmit={handleSubmit(onSubmit)}
      className="flex h-full flex-col gap-4"
      aria-label="新建主体表单"
    >
      <label className="flex flex-col gap-1 text-sm">
        <span className="text-text-muted">
          主体名 <span className="text-deny">*</span>
        </span>
        <input
          {...register('name')}
          aria-label="主体名"
          aria-invalid={errors.name ? 'true' : 'false'}
          className="rounded-card border border-border bg-bg px-2 py-1.5 font-mono"
          autoComplete="off"
        />
        {errors.name && (
          <span role="alert" className="text-xs text-deny">
            {errors.name.message}
          </span>
        )}
      </label>

      <fieldset className="flex flex-col gap-1 text-sm">
        <legend className="text-text-muted">
          kind <span className="text-deny">*</span>
        </legend>
        <div className="flex gap-3">
          {PRINCIPAL_KINDS.map((k) => (
            <label key={k} className="flex items-center gap-1">
              <input type="radio" value={k} {...register('kind')} />
              <span>{k}</span>
            </label>
          ))}
        </div>
      </fieldset>

      {previewReady && (
        <div className="rounded-card border border-border bg-surface-2 p-3 text-xs text-text-muted">
          <div className="mb-1 font-medium text-text">摘要预览</div>
          {principalCreateSummary(values)}
        </div>
      )}

      {submitError && (
        <div role="alert" className="rounded-card border border-deny/40 bg-deny/5 p-2 text-xs text-deny">
          {submitError}
        </div>
      )}

      <div className="mt-auto flex justify-end gap-2 pt-2">
        <button
          type="button"
          onClick={onCancel}
          className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
        >
          取消
        </button>
        <button
          type="submit"
          disabled={submitting}
          className="rounded-card bg-info px-3 py-1.5 text-sm text-white hover:enabled:brightness-110 disabled:opacity-40"
        >
          {submitting ? '登记中…' : '登记主体'}
        </button>
      </div>
    </form>
  );
}
