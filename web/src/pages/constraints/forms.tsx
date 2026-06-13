/**
 * The three create/edit forms inside the shared FormDrawer (docs §四).
 *
 * Unified write flow: RHF + Zod validation → 摘要预览 →（危险则）ConfirmDialog →
 * submit (carries `version` for optimistic lock) → 成功(policy_rev 前进)/失败(红色,
 * 不改视图)/409(提示刷新). `@hookform/resolvers` is not a project dependency, so a
 * tiny inline zodResolver bridges Zod issues into RHF's error shape — no new dep.
 */

import { useState } from 'react';
import {
  useForm,
  type Resolver,
  type FieldValues,
  type SubmitHandler,
} from 'react-hook-form';
import { z, type ZodType } from 'zod';
import { CAPABILITIES, type Capability } from '../../api/types';
import { ConfirmDialog } from '../../components';
import { ConflictError } from '../../api/client';
import {
  CONDITION_PREDICATES,
  isParsableJson,
  type ConditionWrite,
  type ConstraintWrite,
  type DenyNoteWrite,
} from './data';
import {
  IntersectionHint,
  JsonPreview,
  KindMatrixSelect,
  ScopeWidenHint,
  SummaryLine,
  VerbatimNote,
} from './parts';
import type { Adapter } from '../../api/types';

/** Inline Zod→RHF resolver (avoids the @hookform/resolvers dependency). */
function zodResolver<T extends FieldValues>(schema: ZodType<T>): Resolver<T> {
  return (values) => {
    const result = schema.safeParse(values);
    if (result.success) return { values: result.data, errors: {} };
    const errors: Record<string, { type: string; message: string }> = {};
    for (const issue of result.error.issues) {
      const key = issue.path[0];
      if (typeof key === 'string' && !errors[key]) {
        errors[key] = { type: 'validation', message: issue.message };
      }
    }
    return { values: {}, errors } as ReturnType<Resolver<T>>;
  };
}

const jsonSpec = z.string().refine(isParsableJson, 'spec 必须是可解析的 JSON');
const capabilityEnum = z.enum(
  CAPABILITIES as unknown as [Capability, ...Capability[]],
);

/** A 409 surfaced to the caller — page shows "他人已改，请刷新重试". */
export interface WriteError {
  conflict: boolean;
  code: string;
  message: string;
}

function toWriteError(err: unknown): WriteError {
  if (err instanceof ConflictError) {
    return { conflict: true, code: err.code, message: err.message };
  }
  if (err instanceof Error) {
    return {
      conflict: false,
      code: (err as { code?: string }).code ?? 'error',
      message: err.message,
    };
  }
  return { conflict: false, code: 'error', message: String(err) };
}

interface ResourceOption {
  code: string;
  adapter: Adapter;
}

function FormError({ children }: { children: React.ReactNode }) {
  if (!children) return null;
  return (
    <p role="alert" className="mt-1 text-xs text-deny">
      {children}
    </p>
  );
}

function ConflictBanner({ error }: { error: WriteError | null }) {
  if (!error) return null;
  return (
    <div
      role="alert"
      className="rounded-card border border-deny/40 bg-deny/5 px-3 py-2 text-xs"
    >
      <div className="font-medium text-deny">
        {error.conflict
          ? '他人已修改此记录，请刷新后基于最新 version 重试'
          : '提交失败'}
      </div>
      <div className="mt-0.5 font-mono text-text-muted">
        {error.code}: {error.message}
      </div>
    </div>
  );
}

function Footer({
  onCancel,
  submitting,
  submitId,
}: {
  onCancel: () => void;
  submitting: boolean;
  submitId: string;
}) {
  return (
    <div className="flex justify-end gap-2">
      <button
        type="button"
        onClick={onCancel}
        className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
      >
        取消
      </button>
      <button
        type="submit"
        form={submitId}
        disabled={submitting}
        className="rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40 hover:enabled:brightness-110"
      >
        {submitting ? '提交中…' : '提交'}
      </button>
    </div>
  );
}

// ── 细则 Constraint form ──────────────────────────────────────────────────────

interface ConstraintFields {
  resource: string;
  capability: Capability;
  kind: string;
  spec: string;
}

const constraintSchema = z.object({
  resource: z.string().min(1, '请选择资源'),
  capability: capabilityEnum,
  kind: z.string().min(1, '请选择 kind'),
  spec: jsonSpec,
});

export function ConstraintForm({
  resources,
  initial,
  /** count of EXISTING same-(resource,capability,kind) rows for the交集提示. */
  sameKindCount,
  onSubmit,
  onCancel,
}: {
  resources: ResourceOption[];
  initial?: Partial<ConstraintFields> & { id?: string; version?: number };
  sameKindCount: number;
  onSubmit: (body: ConstraintWrite) => Promise<void>;
  onCancel: () => void;
}) {
  const [err, setErr] = useState<WriteError | null>(null);
  const {
    register,
    handleSubmit,
    watch,
    setValue,
    formState: { errors, isSubmitting },
  } = useForm<ConstraintFields>({
    resolver: zodResolver(constraintSchema),
    defaultValues: {
      resource: initial?.resource ?? '',
      capability: initial?.capability ?? 'observe',
      kind: initial?.kind ?? '',
      spec: initial?.spec ?? '',
    },
  });

  const values = watch();
  const adapter = resources.find((r) => r.code === values.resource)?.adapter;

  const submit: SubmitHandler<ConstraintFields> = async (data) => {
    setErr(null);
    try {
      await onSubmit({
        ...(initial?.id ? { id: initial.id, version: initial.version } : {}),
        resource: data.resource,
        capability: data.capability,
        kind: data.kind,
        spec: data.spec,
      });
    } catch (e) {
      setErr(toWriteError(e));
    }
  };

  return (
    <>
      <form id="constraint-form" onSubmit={handleSubmit(submit)} className="flex flex-col gap-3">
        <label className="text-sm">
          资源
          <select
            {...register('resource')}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1"
          >
            <option value="">选择资源…</option>
            {resources.map((r) => (
              <option key={r.code} value={r.code}>
                {r.code} ({r.adapter})
              </option>
            ))}
          </select>
          <FormError>{errors.resource?.message}</FormError>
        </label>

        <label className="text-sm">
          动词
          <select
            {...register('capability')}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1"
          >
            {CAPABILITIES.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        </label>

        <label className="text-sm" htmlFor="constraint-kind">
          kind
        </label>
        <KindMatrixSelect
          id="constraint-kind"
          adapter={adapter}
          value={values.kind}
          onChange={(k) => setValue('kind', k, { shouldValidate: true })}
        />
        <FormError>{errors.kind?.message}</FormError>

        <label className="text-sm">
          spec（raw JSON）
          <textarea
            {...register('spec')}
            rows={4}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1 font-mono text-xs"
            placeholder='{"prefix":"app-"}'
          />
          <FormError>{errors.spec?.message}</FormError>
        </label>

        <ConflictBanner error={err} />
      </form>

      <section aria-label="摘要预览" className="mt-4 flex flex-col gap-2 border-t border-border pt-3">
        <h3 className="text-xs font-medium uppercase text-text-muted">摘要预览</h3>
        <SummaryLine>
          将给 <code className="font-mono">{values.resource || '—'}</code> 的{' '}
          <code className="font-mono">{values.capability}</code> 挂{' '}
          <code className="font-mono">{values.kind || '—'}</code> 细则，按 spec 收窄对象作用面。
        </SummaryLine>
        {isParsableJson(values.spec) && <JsonPreview spec={values.spec} />}
        <IntersectionHint count={sameKindCount} />
      </section>

      <div className="mt-4">
        <Footer onCancel={onCancel} submitting={isSubmitting} submitId="constraint-form" />
      </div>
    </>
  );
}

// ── 条件 Condition form ───────────────────────────────────────────────────────

interface ConditionFields {
  resource: string;
  capability: string;
  predicate: string;
  spec: string;
}

const conditionSchema = z.object({
  resource: z.string(),
  capability: z.string(),
  predicate: z.enum(
    CONDITION_PREDICATES as unknown as [string, ...string[]],
  ),
  spec: jsonSpec,
});

export function ConditionForm({
  resources,
  initial,
  onSubmit,
  onCancel,
}: {
  resources: ResourceOption[];
  initial?: Partial<ConditionFields> & { id?: string; version?: number };
  onSubmit: (body: ConditionWrite) => Promise<void>;
  onCancel: () => void;
}) {
  const [err, setErr] = useState<WriteError | null>(null);
  const [confirmWiden, setConfirmWiden] = useState<null | ConditionFields>(null);
  const {
    register,
    handleSubmit,
    watch,
    formState: { errors, isSubmitting },
  } = useForm<ConditionFields>({
    resolver: zodResolver(conditionSchema),
    defaultValues: {
      resource: initial?.resource ?? '',
      capability: initial?.capability ?? '',
      predicate: initial?.predicate ?? 'rate_limit',
      spec: initial?.spec ?? '',
    },
  });

  const values = watch();
  const scopeEmpty = values.resource === '' || values.capability === '';
  const scopeLabel =
    values.resource === '' && values.capability === ''
      ? '全资源/全动词'
      : values.resource === ''
        ? '全资源'
        : values.capability === ''
          ? '全动词'
          : `(${values.resource}, ${values.capability})`;

  async function commit(data: ConditionFields) {
    setErr(null);
    try {
      await onSubmit({
        ...(initial?.id ? { id: initial.id, version: initial.version } : {}),
        resource: data.resource === '' ? null : data.resource,
        capability: data.capability === '' ? null : (data.capability as Capability),
        predicate: data.predicate,
        spec: data.spec,
      });
    } catch (e) {
      setErr(toWriteError(e));
    }
  }

  const submit: SubmitHandler<ConditionFields> = (data) => {
    // 作用域留空 = 范围放大 → 二次确认 (docs §4.7).
    if (data.resource === '' || data.capability === '') {
      setConfirmWiden(data);
      return;
    }
    void commit(data);
  };

  return (
    <>
      <form id="condition-form" onSubmit={handleSubmit(submit)} className="flex flex-col gap-3">
        <label className="text-sm">
          资源（留空=全资源）
          <select
            {...register('resource')}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1"
          >
            <option value="">* 全资源</option>
            {resources.map((r) => (
              <option key={r.code} value={r.code}>
                {r.code} ({r.adapter})
              </option>
            ))}
          </select>
        </label>

        <label className="text-sm">
          动词（留空=全动词）
          <select
            {...register('capability')}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1"
          >
            <option value="">* 全动词</option>
            {CAPABILITIES.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        </label>

        <label className="text-sm">
          predicate
          <select
            {...register('predicate')}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1"
          >
            {CONDITION_PREDICATES.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
        </label>

        <label className="text-sm">
          spec（raw JSON）
          <textarea
            {...register('spec')}
            rows={4}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1 font-mono text-xs"
            placeholder='{"per_minute":60}'
          />
          <FormError>{errors.spec?.message}</FormError>
        </label>

        <ConflictBanner error={err} />
      </form>

      <section aria-label="摘要预览" className="mt-4 flex flex-col gap-2 border-t border-border pt-3">
        <h3 className="text-xs font-medium uppercase text-text-muted">摘要预览</h3>
        <SummaryLine>
          将给 <code className="font-mono">{scopeLabel}</code> 附加{' '}
          <code className="font-mono">{values.predicate}</code> 条件。
        </SummaryLine>
        {isParsableJson(values.spec) && <JsonPreview spec={values.spec} />}
        {scopeEmpty && <ScopeWidenHint scope={scopeLabel} />}
      </section>

      <div className="mt-4">
        <Footer onCancel={onCancel} submitting={isSubmitting} submitId="condition-form" />
      </div>

      <ConfirmDialog
        open={confirmWiden !== null}
        title="确认放大作用域"
        body={`作用域=${scopeLabel}，该条件将作用于更广的范围。确认创建？`}
        confirmLabel="确认创建"
        danger
        onConfirm={() => {
          const data = confirmWiden;
          setConfirmWiden(null);
          if (data) void commit(data);
        }}
        onCancel={() => setConfirmWiden(null)}
      />
    </>
  );
}

// ── 拒绝指引 Deny-note form ───────────────────────────────────────────────────

interface DenyNoteFields {
  resource: string;
  capability: Capability;
  note: string;
}

const denyNoteSchema = z.object({
  resource: z.string().min(1, '请选择资源'),
  capability: capabilityEnum,
  note: z.string().min(1, 'note 不能为空'),
});

export function DenyNoteForm({
  resources,
  initial,
  /** true when a生效 note already exists for (resource,capability) → 编辑语态. */
  editing,
  onSubmit,
  onCancel,
}: {
  resources: ResourceOption[];
  initial?: Partial<DenyNoteFields> & { id?: string; version?: number };
  editing: boolean;
  onSubmit: (body: DenyNoteWrite) => Promise<void>;
  onCancel: () => void;
}) {
  const [err, setErr] = useState<WriteError | null>(null);
  const {
    register,
    handleSubmit,
    watch,
    formState: { errors, isSubmitting },
  } = useForm<DenyNoteFields>({
    resolver: zodResolver(denyNoteSchema),
    defaultValues: {
      resource: initial?.resource ?? '',
      capability: initial?.capability ?? 'observe',
      note: initial?.note ?? '',
    },
  });

  const values = watch();

  const submit: SubmitHandler<DenyNoteFields> = async (data) => {
    setErr(null);
    try {
      await onSubmit({
        ...(initial?.id ? { id: initial.id, version: initial.version } : {}),
        resource: data.resource,
        capability: data.capability,
        note: data.note,
      });
    } catch (e) {
      setErr(toWriteError(e));
    }
  };

  return (
    <>
      <p className="mb-3 rounded-card border border-warn/40 bg-warn/5 px-3 py-2 text-xs text-warn">
        此文本越权时将原样回给 Agent（operator_note），网关不加工。写你想让对方看到的人话。
      </p>
      {editing && (
        <p className="mb-3 text-xs text-text-muted">
          该 (资源, 动词) 已有生效拒绝指引——此处为<strong>编辑</strong>既有记录（带 version），不创建第二条。
        </p>
      )}

      <form id="deny-note-form" onSubmit={handleSubmit(submit)} className="flex flex-col gap-3">
        <label className="text-sm">
          资源
          <select
            {...register('resource')}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1"
          >
            <option value="">选择资源…</option>
            {resources.map((r) => (
              <option key={r.code} value={r.code}>
                {r.code} ({r.adapter})
              </option>
            ))}
          </select>
          <FormError>{errors.resource?.message}</FormError>
        </label>

        <label className="text-sm">
          动词
          <select
            {...register('capability')}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1"
          >
            {CAPABILITIES.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        </label>

        <label className="text-sm">
          note（多行纯文本，原样转述）
          <textarea
            {...register('note')}
            rows={5}
            className="mt-1 w-full rounded-card border border-border bg-bg px-2 py-1 font-mono text-xs"
          />
          <FormError>{errors.note?.message}</FormError>
        </label>

        <ConflictBanner error={err} />
      </form>

      <section aria-label="摘要预览" className="mt-4 flex flex-col gap-2 border-t border-border pt-3">
        <h3 className="text-xs font-medium uppercase text-text-muted">摘要预览（Agent 所见原文）</h3>
        <SummaryLine>
          此后对 <code className="font-mono">{values.resource || '—'}</code> 的{' '}
          <code className="font-mono">{values.capability}</code> 越权拒绝将附带以下 note：
        </SummaryLine>
        {values.note && <VerbatimNote note={values.note} />}
      </section>

      <div className="mt-4">
        <Footer onCancel={onCancel} submitting={isSubmitting} submitId="deny-note-form" />
      </div>
    </>
  );
}
