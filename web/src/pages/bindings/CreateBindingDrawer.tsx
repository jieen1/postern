/**
 * CreateBindingDrawer — the unified write flow for creating a binding
 * (07-bindings.md §4.1; 设计系统 §7 写操作统一流程):
 *   form (RHF+Zod) → live expansion preview → 提交前摘要预览 → create
 *   → invalidate refresh → success(policy_rev↑) / error(no view change) / 409.
 *
 * The principal carries the optimistic-lock version (§4.1.4): its read-time
 * `version` is the SOLE source of the expected version sent on create.
 */

import { useEffect, useState } from 'react';
import { useForm } from 'react-hook-form';
import { z } from 'zod';
import { ConflictError } from '@/api/client';
import { FormDrawer } from '@/components';
import type { PrincipalRow, Role, ScopeKind } from '@/api/types';
import { ScopeEditor } from './ScopeEditor';
import { ExpansionPreview } from './ExpansionPreview';
import { useCreateBinding, previewExpansion, type ExpansionPreview as PreviewData } from './api';
import {
  buildResourceSpec,
  buildSelectorSpec,
  hasScopeContent,
  type SelectorRow,
} from './scope';

const formSchema = z.object({
  principal: z.string().min(1, '请选择主体'),
  role: z.string().min(1, '请选择角色'),
});
type FormValues = z.infer<typeof formSchema>;

export function CreateBindingDrawer({
  open,
  onClose,
  principals,
  roles,
  resourceOptions,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  principals: PrincipalRow[];
  roles: Role[];
  resourceOptions: string[];
  /** Called with the new policy_rev after a successful create. */
  onCreated: (policyRev: string) => void;
}) {
  const {
    register,
    handleSubmit,
    watch,
    reset,
    formState: { errors },
  } = useForm<FormValues>({
    defaultValues: { principal: '', role: '' },
  });

  const [scopeKind, setScopeKind] = useState<ScopeKind>('selector');
  const [rows, setRows] = useState<SelectorRow[]>([{ key: 'host', value: '' }]);
  const [codes, setCodes] = useState<string[]>([]);

  const [preview, setPreview] = useState<PreviewData | undefined>(undefined);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState(false);

  const [showSummary, setShowSummary] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);

  const role = watch('role');
  const principalName = watch('principal');

  const create = useCreateBinding();

  const scopeSpec =
    scopeKind === 'selector' ? buildSelectorSpec(rows) : buildResourceSpec(codes);
  const hasContent = hasScopeContent(scopeKind, rows, codes);

  // Live expansion preview (§4.1.2): re-probe the daemon whenever the spec or
  // role changes. Fail-closed — a failed probe sets previewError, never an
  // optimistic resource set.
  useEffect(() => {
    if (!open || !role || !hasContent) {
      setPreview(undefined);
      setPreviewError(false);
      return;
    }
    let cancelled = false;
    setPreviewLoading(true);
    setPreviewError(false);
    previewExpansion({ role, scope_kind: scopeKind, scope_spec: scopeSpec })
      .then((data) => {
        if (!cancelled) setPreview(data);
      })
      .catch(() => {
        if (!cancelled) {
          setPreview(undefined);
          setPreviewError(true);
        }
      })
      .finally(() => {
        if (!cancelled) setPreviewLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, role, scopeKind, scopeSpec, hasContent]);

  function close() {
    reset();
    setScopeKind('selector');
    setRows([{ key: 'host', value: '' }]);
    setCodes([]);
    setPreview(undefined);
    setPreviewError(false);
    setShowSummary(false);
    setSubmitError(null);
    setConflict(false);
    onClose();
  }

  const parsedValues = formSchema.safeParse({ principal: principalName, role });
  const canPreviewSummary = parsedValues.success && hasContent;

  function onConfirmCreate() {
    if (!parsedValues.success) return;
    const principal = principals.find((p) => p.name === parsedValues.data.principal);
    if (!principal) {
      setSubmitError('主体不存在');
      return;
    }
    setSubmitError(null);
    setConflict(false);
    create.mutate(
      {
        principal: parsedValues.data.principal,
        role: parsedValues.data.role,
        scope_kind: scopeKind,
        scope_spec: scopeSpec,
        version: principal.version,
      },
      {
        onSuccess: (ack) => {
          onCreated(ack.policy_rev);
          close();
        },
        onError: (err) => {
          if (err instanceof ConflictError) {
            setConflict(true);
            setShowSummary(false);
          } else {
            setSubmitError(err instanceof Error ? err.message : '创建失败');
            setShowSummary(false);
          }
        },
      },
    );
  }

  return (
    <FormDrawer
      open={open}
      title="新建绑定"
      onClose={close}
      footer={
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={close}
            className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
          >
            取消
          </button>
          <button
            type="button"
            disabled={!canPreviewSummary || create.isPending}
            onClick={handleSubmit(() => setShowSummary(true))}
            className="rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40 hover:enabled:brightness-110"
          >
            预览摘要并创建
          </button>
        </div>
      }
    >
      <form className="flex flex-col gap-4" onSubmit={(e) => e.preventDefault()}>
        <label className="flex flex-col gap-1 text-sm">
          <span className="font-medium">Principal *</span>
          <select
            {...register('principal')}
            className="rounded-card border border-border bg-bg px-2 py-1"
          >
            <option value="">选择主体…</option>
            {principals.map((p) => (
              <option key={p.id} value={p.name}>
                {p.name}
              </option>
            ))}
          </select>
          {errors.principal && (
            <span className="text-xs text-deny">{errors.principal.message}</span>
          )}
        </label>

        <label className="flex flex-col gap-1 text-sm">
          <span className="font-medium">Role *</span>
          <select
            {...register('role')}
            className="rounded-card border border-border bg-bg px-2 py-1"
          >
            <option value="">选择角色…</option>
            {roles.map((r) => (
              <option key={r.id} value={r.name}>
                {r.name}
              </option>
            ))}
          </select>
          {errors.role && (
            <span className="text-xs text-deny">{errors.role.message}</span>
          )}
        </label>

        <ScopeEditor
          kind={scopeKind}
          onKindChange={setScopeKind}
          rows={rows}
          onRowsChange={setRows}
          codes={codes}
          onCodesChange={setCodes}
          resourceOptions={resourceOptions}
        />

        <section className="rounded-card border border-border bg-surface-2 p-3">
          <p className="mb-2 text-sm font-medium">展开预览（只读，daemon 回报）</p>
          {role && hasContent ? (
            <ExpansionPreview
              data={preview}
              loading={previewLoading}
              error={previewError}
            />
          ) : (
            <p className="text-xs text-text-muted">
              选择 Role 并填写 Scope 后显示展开结果
            </p>
          )}
        </section>

        {conflict && (
          <p
            role="alert"
            className="rounded-card border border-warn/40 bg-warn/5 px-3 py-2 text-xs text-warn"
          >
            他人已改、请刷新重试——重新读取最新 version 再提交
          </p>
        )}
        {submitError && (
          <p
            role="alert"
            className="rounded-card border border-deny/40 bg-deny/5 px-3 py-2 text-xs text-deny"
          >
            {submitError}
          </p>
        )}
      </form>

      {showSummary && parsedValues.success && (
        <SummaryConfirm
          principal={parsedValues.data.principal}
          role={parsedValues.data.role}
          scopeKind={scopeKind}
          scopeSpec={scopeSpec}
          preview={preview}
          pending={create.isPending}
          onCancel={() => setShowSummary(false)}
          onConfirm={onConfirmCreate}
        />
      )}
    </FormDrawer>
  );
}

function SummaryConfirm({
  principal,
  role,
  scopeKind,
  scopeSpec,
  preview,
  pending,
  onCancel,
  onConfirm,
}: {
  principal: string;
  role: string;
  scopeKind: ScopeKind;
  scopeSpec: string;
  preview: PreviewData | undefined;
  pending: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const count = preview?.expanded_resources.length ?? 0;
  const wide = count >= 5;
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="确认创建绑定"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-md rounded-card border border-border bg-surface p-5 shadow-lg">
        <h2 className="mb-3 text-lg font-medium">确认创建绑定</h2>
        <dl className="mb-3 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm">
          <dt className="text-text-muted">principal</dt>
          <dd className="font-mono">{principal}</dd>
          <dt className="text-text-muted">role</dt>
          <dd className="font-mono">{role}</dd>
          <dt className="text-text-muted">scope</dt>
          <dd className="break-all font-mono">
            {scopeKind} {scopeSpec}
          </dd>
          <dt className="text-text-muted">展开</dt>
          <dd className="font-mono" data-testid="summary-expansion">
            {preview
              ? `[${preview.expanded_resources.join(', ')}]`
              : '（展开未知）'}
          </dd>
        </dl>
        {wide && (
          <p className="mb-3 rounded-card border border-warn/40 bg-warn/5 px-3 py-2 text-xs text-warn">
            将新增 {count} 个资源的授权格——请确认范围
          </p>
        )}
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
          >
            返回修改
          </button>
          <button
            type="button"
            disabled={pending}
            onClick={onConfirm}
            className="rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40 hover:enabled:brightness-110"
          >
            确认创建
          </button>
        </div>
      </div>
    </div>
  );
}
