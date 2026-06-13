import { useFieldArray, useForm } from 'react-hook-form';
import { Plus, X } from 'lucide-react';
import { CAPABILITIES, type Capability, type ResourceRow } from '../../../api/types';
import {
  ADAPTERS,
  TRANSPORTS,
  resourceResolver,
  type ResourceFormValues,
} from '../resourceForm';

/**
 * Sectioned access/edit form (设计 §2.2): ① basics ② real address (anonymized)
 * ③ tiers. RHF holds form state; the manual Zod resolver enforces the front-end
 * invariants (slug code, ≥1 read-only tier). On valid submit the parent runs the
 * summary preview → (danger) confirm → write flow; the form never writes itself.
 *
 * Address discipline (设计原则 6 / §6 步骤2): the address input is transient. On
 * submit the summary states it becomes `vault://{code}/target`; plaintext is
 * never echoed back as an address and never carried as a usable address field.
 */

function emptyValues(): ResourceFormValues {
  return {
    code: '',
    adapter: 'postgres',
    transport: 'direct',
    engine_enforced: true,
    address: '',
    labels: [],
    tiers: [{ tier: 'ro', capabilities: ['observe', 'query'] }],
  };
}

function fromRow(row: ResourceRow): ResourceFormValues {
  return {
    code: row.code,
    adapter: row.adapter,
    transport: (TRANSPORTS as readonly string[]).includes(row.transport)
      ? (row.transport as ResourceFormValues['transport'])
      : 'direct',
    engine_enforced: true,
    address: '',
    labels: row.labels.map((l) => ({ key: l.key, value: l.value })),
    tiers: row.tiers.map((t) => ({ tier: t.tier, capabilities: t.capabilities })),
  };
}

export function ResourceForm({
  editing,
  onValid,
  onCancel,
  formId,
}: {
  /** The row being edited, or null for a new resource. */
  editing: ResourceRow | null;
  /** Called with validated values when the user requests the summary preview. */
  onValid: (values: ResourceFormValues) => void;
  onCancel: () => void;
  /** Stable id so an external footer button can submit this form. */
  formId: string;
}) {
  const {
    register,
    control,
    handleSubmit,
    watch,
    setValue,
    formState: { errors },
  } = useForm<ResourceFormValues>({
    resolver: resourceResolver,
    defaultValues: editing ? fromRow(editing) : emptyValues(),
  });

  const labels = useFieldArray({ control, name: 'labels' });
  const tiers = useFieldArray({ control, name: 'tiers' });
  const code = watch('code');
  const tierValues = watch('tiers');
  const engineEnforced = watch('engine_enforced');

  function toggleCap(tierIdx: number, cap: Capability) {
    const current = tierValues?.[tierIdx]?.capabilities ?? [];
    const next = current.includes(cap)
      ? current.filter((c) => c !== cap)
      : [...current, cap];
    setValue(`tiers.${tierIdx}.capabilities`, next, { shouldValidate: false });
  }

  return (
    <form id={formId} onSubmit={handleSubmit(onValid)} className="flex flex-col gap-6">
      {/* ① 基本信息 */}
      <section className="flex flex-col gap-3">
        <h3 className="text-sm font-medium text-text">① 基本信息</h3>

        <label className="flex flex-col gap-1 text-sm">
          代号 code（唯一，未删集内）
          <input
            {...register('code')}
            readOnly={Boolean(editing)}
            aria-invalid={Boolean(errors.code)}
            className="rounded-card border border-border bg-bg px-2 py-1 font-mono read-only:opacity-60"
            placeholder="db-main"
          />
          {errors.code && (
            <span role="alert" className="text-xs text-deny">
              {errors.code.message}
            </span>
          )}
        </label>

        <div className="grid grid-cols-2 gap-3">
          <label className="flex flex-col gap-1 text-sm">
            adapter
            <select
              {...register('adapter')}
              className="rounded-card border border-border bg-bg px-2 py-1"
            >
              {ADAPTERS.map((a) => (
                <option key={a} value={a}>
                  {a}
                </option>
              ))}
            </select>
          </label>
          <label className="flex flex-col gap-1 text-sm">
            transport
            <select
              {...register('transport')}
              className="rounded-card border border-border bg-bg px-2 py-1"
            >
              {TRANSPORTS.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </label>
        </div>

        <fieldset className="flex flex-col gap-1 text-sm">
          <legend className="mb-1">engine_enforced（adapter 决定缺省）</legend>
          <div className="flex gap-4">
            <label className="flex items-center gap-1">
              <input
                type="radio"
                name="engine_enforced"
                checked={engineEnforced === true}
                onChange={() => setValue('engine_enforced', true)}
              />
              true
            </label>
            <label className="flex items-center gap-1">
              <input
                type="radio"
                name="engine_enforced"
                checked={engineEnforced === false}
                onChange={() => setValue('engine_enforced', false)}
              />
              false
            </label>
          </div>
        </fieldset>

        <div className="flex flex-col gap-1 text-sm">
          <span>标签 labels</span>
          <ul className="flex flex-col gap-1">
            {labels.fields.map((f, i) => (
              <li key={f.id} className="flex items-center gap-1">
                <input
                  {...register(`labels.${i}.key`)}
                  aria-label={`标签键 ${i + 1}`}
                  className="w-28 rounded-card border border-border bg-bg px-2 py-1 font-mono text-xs"
                  placeholder="env"
                />
                <span>=</span>
                <input
                  {...register(`labels.${i}.value`)}
                  aria-label={`标签值 ${i + 1}`}
                  className="w-28 rounded-card border border-border bg-bg px-2 py-1 font-mono text-xs"
                  placeholder="prod"
                />
                <button
                  type="button"
                  aria-label={`删除标签 ${i + 1}`}
                  onClick={() => labels.remove(i)}
                  className="text-text-muted hover:text-deny"
                >
                  <X size={14} />
                </button>
              </li>
            ))}
          </ul>
          <button
            type="button"
            onClick={() => labels.append({ key: '', value: '' })}
            className="inline-flex w-fit items-center gap-1 rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
          >
            <Plus size={12} /> 添加标签
          </button>
        </div>
      </section>

      {/* ② 真实地址（匿名化） */}
      <section className="flex flex-col gap-2">
        <h3 className="text-sm font-medium text-text">② 真实地址（匿名化）</h3>
        <label className="flex flex-col gap-1 text-sm">
          host / port（提交即转 vault 引用，明文不入库）
          <input
            {...register('address')}
            type="password"
            autoComplete="off"
            className="rounded-card border border-border bg-bg px-2 py-1 font-mono"
            placeholder="········"
          />
        </label>
        <p className="font-mono text-xs text-text-muted">
          现值: vault://{code || '{code}'}/target
        </p>
      </section>

      {/* ③ 凭据等级 tiers */}
      <section className="flex flex-col gap-2">
        <h3 className="text-sm font-medium text-text">③ 凭据等级 tiers（≥1 只读 tier）</h3>
        {errors.tiers && (
          <span role="alert" className="text-xs text-deny">
            {errors.tiers.message ?? '至少声明一个 tier'}
          </span>
        )}
        <ul className="flex flex-col gap-2">
          {tiers.fields.map((f, i) => (
            <li key={f.id} className="rounded-card border border-border p-2">
              <div className="mb-2 flex items-center gap-2">
                <input
                  {...register(`tiers.${i}.tier`)}
                  aria-label={`tier 代号 ${i + 1}`}
                  className="w-24 rounded-card border border-border bg-bg px-2 py-1 font-mono text-xs"
                  placeholder="ro"
                />
                <button
                  type="button"
                  aria-label={`删除 tier ${i + 1}`}
                  onClick={() => tiers.remove(i)}
                  className="ml-auto text-text-muted hover:text-deny"
                >
                  <X size={14} />
                </button>
              </div>
              <div className="flex flex-wrap gap-2">
                {CAPABILITIES.map((cap) => {
                  const on = (tierValues?.[i]?.capabilities ?? []).includes(cap);
                  return (
                    <label key={cap} className="flex items-center gap-1 text-xs">
                      <input
                        type="checkbox"
                        checked={on}
                        onChange={() => toggleCap(i, cap)}
                        aria-label={`tier ${i + 1} 动词 ${cap}`}
                      />
                      <span className="font-mono">{cap}</span>
                    </label>
                  );
                })}
              </div>
              <p className="mt-1 font-mono text-[11px] text-text-muted">
                secret_ref: vault://{code || '{code}'}/{watch(`tiers.${i}.tier`) || 'tier'}
              </p>
            </li>
          ))}
        </ul>
        <button
          type="button"
          onClick={() => tiers.append({ tier: '', capabilities: [] })}
          className="inline-flex w-fit items-center gap-1 rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
        >
          <Plus size={12} /> 添加 tier
        </button>
        <p className="text-xs text-text-muted">
          （tier 凭据明文 → 10-credentials 录入；本页仅声明动词集与引用）
        </p>
      </section>

      {/* keep onCancel referenced for a11y/escape symmetry */}
      <button type="button" hidden onClick={onCancel} aria-hidden="true" tabIndex={-1} />
    </form>
  );
}
