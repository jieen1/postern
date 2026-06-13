import { useForm } from 'react-hook-form';
import type { PrincipalRow } from '../../api/types';
import {
  CREDENTIAL_KINDS,
  credentialCreateSummary,
  credentialSchema,
  zodResolver,
  type CredentialFormValues,
} from './schema';

/**
 * 新建凭证表单（§4.2）。已绑定当前主体（只读显示所属主体）。RHF + 本地 zod
 * resolver；按 kind 自适应是否录入明文：
 *  - local_process：进程上下文凭证，不录入 secret；
 *  - api_key：由 daemon 生成，表单不录入明文（成功后一次性展示，由父层处理）；
 *  - token：录入既有令牌值（明文经控制面转发、本地不留存、提交后即清）。
 *
 * 明文零接触纪律：表单永不显示也不接收 secret_hash；secret 字段 type=password、
 * autoComplete=off，提交后由父层清空表单（卸载 drawer）。
 */
export function CredentialForm({
  principal,
  submitting,
  submitError,
  onSubmit,
  onCancel,
}: {
  principal: PrincipalRow;
  submitting: boolean;
  submitError: string | null;
  onSubmit: (values: CredentialFormValues) => void;
  onCancel: () => void;
}) {
  const {
    register,
    handleSubmit,
    watch,
    formState: { errors, isValid },
  } = useForm<CredentialFormValues>({
    resolver: zodResolver(credentialSchema),
    mode: 'onChange',
    defaultValues: { kind: 'api_key', trust_domain: '', ttl_days: '', secret: '' },
  });

  const values = watch();
  const isToken = values.kind === 'token';

  return (
    <form
      onSubmit={handleSubmit(onSubmit)}
      className="flex h-full flex-col gap-4"
      aria-label="新建凭证表单"
    >
      <div className="rounded-card border border-border bg-surface-2 p-2 text-xs text-text-muted">
        所属主体：<span className="font-mono text-text">{principal.name}</span>（只读）
      </div>

      <fieldset className="flex flex-col gap-1 text-sm">
        <legend className="text-text-muted">
          kind <span className="text-deny">*</span>
        </legend>
        <div className="flex flex-wrap gap-3">
          {CREDENTIAL_KINDS.map((k) => (
            <label key={k} className="flex items-center gap-1">
              <input type="radio" value={k} {...register('kind')} />
              <span className="font-mono text-xs">{k}</span>
            </label>
          ))}
        </div>
      </fieldset>

      <label className="flex flex-col gap-1 text-sm">
        <span className="text-text-muted">
          可信域 <span className="text-deny">*</span>
        </span>
        <input
          {...register('trust_domain')}
          aria-label="可信域"
          aria-invalid={errors.trust_domain ? 'true' : 'false'}
          className="rounded-card border border-border bg-bg px-2 py-1.5 font-mono"
          autoComplete="off"
        />
        {errors.trust_domain && (
          <span role="alert" className="text-xs text-deny">
            {errors.trust_domain.message}
          </span>
        )}
      </label>

      <label className="flex flex-col gap-1 text-sm">
        <span className="text-text-muted">有效期（天，留空＝长期有效）</span>
        <input
          {...register('ttl_days')}
          aria-label="有效期天数"
          inputMode="numeric"
          aria-invalid={errors.ttl_days ? 'true' : 'false'}
          className="rounded-card border border-border bg-bg px-2 py-1.5 font-mono"
          autoComplete="off"
          placeholder="留空＝长期有效"
        />
        {errors.ttl_days && (
          <span role="alert" className="text-xs text-deny">
            {errors.ttl_days.message}
          </span>
        )}
      </label>

      {/* 仅 token 录入既有令牌值；其余 kind 不接收明文。 */}
      {isToken && (
        <label className="flex flex-col gap-1 text-sm">
          <span className="text-text-muted">
            令牌值 <span className="text-deny">*</span>
          </span>
          <input
            {...register('secret')}
            type="password"
            aria-label="令牌值"
            aria-invalid={errors.secret ? 'true' : 'false'}
            className="rounded-card border border-border bg-bg px-2 py-1.5 font-mono"
            autoComplete="off"
          />
          <span className="text-xs text-text-muted">
            明文经控制面转发，本地不留存、提交后即清，列表永不回显。
          </span>
          {errors.secret && (
            <span role="alert" className="text-xs text-deny">
              {errors.secret.message}
            </span>
          )}
        </label>
      )}

      {values.kind === 'api_key' && (
        <div className="rounded-card border border-border bg-surface-2 p-2 text-xs text-text-muted">
          api_key 由 daemon 生成，表单不录入明文；成功后一次性显示明文，关闭即不可再得。
        </div>
      )}

      {isValid && (
        <div className="rounded-card border border-border bg-surface-2 p-3 text-xs text-text-muted">
          <div className="mb-1 font-medium text-text">摘要预览</div>
          {credentialCreateSummary(principal.name, values)}
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
          {submitting ? '创建中…' : '创建凭证'}
        </button>
      </div>
    </form>
  );
}
