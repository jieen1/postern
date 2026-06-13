/**
 * RoleFormDrawer — create/edit a role inside the shared FormDrawer
 * (06-roles.md §二/§四, base 写操作统一流程).
 *
 * Flow: RHF+Zod form → "预览摘要 →" summary view (what will be written incl. the
 * carried `version` on edit) → submit. The local effective-set preview labels
 * itself "最终以 daemon 为准". 409 conflicts surface as a refresh prompt; other
 * write failures show the verbatim daemon `message` and do NOT change the view.
 *
 * No admin entry exists (structural absence). The admin-name Zod guard is a
 * convenience layer; the real refusal is daemon-side.
 */

import { useEffect, useMemo, useState } from 'react';
import { useForm, type Resolver } from 'react-hook-form';
import { FormDrawer } from '../../components';
import { ConflictError } from '../../api/client';
import type { Role, RoleCapability } from '../../api/types';
import {
  isAdminName,
  previewEffective,
  roleFormSchema,
  SELECTABLE_CAPABILITIES,
  type FormCapability,
  type RoleFormValues,
} from './lib';
import { CapabilityPicker } from './CapabilityPicker';
import { CapabilityActionBadge } from './CapabilityActionBadge';
import { useRoleWrite, type RoleWriteBody } from './useRoleWrite';

type Phase = 'form' | 'summary';

export interface RoleFormDrawerProps {
  open: boolean;
  /** The role being edited, or null for create. */
  editing: Role | null;
  /** All roles (for the inheritance multi-select + effective preview). */
  roles: Role[];
  onClose: () => void;
  onSaved: (policyRev: string) => void;
}

export function RoleFormDrawer({
  open,
  editing,
  roles,
  onClose,
  onSaved,
}: RoleFormDrawerProps) {
  const [phase, setPhase] = useState<Phase>('form');
  const [conflict, setConflict] = useState(false);
  const write = useRoleWrite();

  const {
    register,
    handleSubmit,
    reset,
    watch,
    setValue,
    formState: { errors },
  } = useForm<RoleFormValues>({
    resolver: zodResolverLite,
    defaultValues: { name: '', description: '', capabilities: [], inherits_from: [] },
  });

  // Reset the form whenever the drawer opens (create vs the specific edit row).
  useEffect(() => {
    if (!open) return;
    setPhase('form');
    setConflict(false);
    write.reset();
    reset(
      editing
        ? {
            name: editing.name,
            description: '',
            capabilities: toFormCapabilities(editing.direct),
            inherits_from: editing.inherits_from,
          }
        : { name: '', description: '', capabilities: [], inherits_from: [] },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, editing]);

  const capabilities = watch('capabilities');
  const inheritsFrom = watch('inherits_from');
  const name = watch('name');

  const rolesByName = useMemo(
    () => new Map(roles.map((r) => [r.name, r] as const)),
    [roles],
  );

  // Inheritance candidates: all OTHER existing roles (cannot inherit self).
  const parentCandidates = useMemo(
    () => roles.filter((r) => r.name !== editing?.name),
    [roles, editing],
  );

  const effective = useMemo(
    () => previewEffective(capabilities, inheritsFrom, rolesByName),
    [capabilities, inheritsFrom, rolesByName],
  );

  const adminBlocked = isAdminName(name);

  function toggleParent(parentName: string, checked: boolean) {
    setValue(
      'inherits_from',
      checked
        ? [...inheritsFrom, parentName]
        : inheritsFrom.filter((p) => p !== parentName),
      { shouldValidate: true },
    );
  }

  function setCapabilities(next: FormCapability[]) {
    setValue('capabilities', next, { shouldValidate: true });
  }

  function goToSummary(values: RoleFormValues) {
    void values;
    setPhase('summary');
  }

  function submit() {
    const values = watch();
    const body: RoleWriteBody = {
      name: values.name.trim(),
      capabilities: values.capabilities,
      inherits_from: values.inherits_from,
      ...(values.description ? { description: values.description } : {}),
      ...(editing ? { id: editing.id, version: editing.version } : {}),
    };
    write.mutate(body, {
      onSuccess: (ack) => onSaved(ack.policy_rev),
      onError: (err) => {
        if (err instanceof ConflictError) setConflict(true);
      },
    });
  }

  const title = editing ? '编辑角色' : '新建角色';
  const writeError =
    write.error && !(write.error instanceof ConflictError) ? write.error.message : null;

  return (
    <FormDrawer
      open={open}
      title={title}
      onClose={onClose}
      footer={
        phase === 'form' ? (
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
              onClick={handleSubmit(goToSummary)}
              disabled={adminBlocked}
              className="rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40 hover:enabled:brightness-110"
            >
              预览摘要 →
            </button>
          </div>
        ) : (
          <div className="flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setPhase('form')}
              className="rounded-card border border-border px-3 py-1.5 text-sm hover:bg-surface-2"
            >
              ← 返回修改
            </button>
            <button
              type="button"
              onClick={submit}
              disabled={write.isPending}
              className="rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40 hover:enabled:brightness-110"
            >
              {write.isPending ? '提交中…' : '提交'}
            </button>
          </div>
        )
      }
    >
      {/* Write-time conflict / error banners (do NOT mutate the view). */}
      {conflict && (
        <div role="alert" className="mb-3 rounded-card border border-warn/50 bg-warn/10 px-3 py-2 text-sm text-warn">
          他人已改，请关闭后重新读取最新 version 再改（乐观锁冲突 409）。
        </div>
      )}
      {writeError && (
        <div role="alert" className="mb-3 rounded-card border border-deny/50 bg-deny/10 px-3 py-2 font-mono text-xs text-deny">
          {writeError}
        </div>
      )}

      {phase === 'form' ? (
        <form className="flex flex-col gap-4" onSubmit={handleSubmit(goToSummary)}>
          <label className="flex flex-col gap-1 text-sm">
            <span className="font-medium">名称</span>
            <input
              {...register('name')}
              aria-label="名称"
              className="rounded-card border border-border bg-bg px-2 py-1"
            />
            <span className="text-xs text-text-muted">唯一（delete_flag=0）· 非空</span>
            {adminBlocked && (
              <span role="alert" className="text-xs text-deny">
                admin 不可作为可授予角色（真正硬拒在 daemon）
              </span>
            )}
            {errors.name && !adminBlocked && (
              <span role="alert" className="text-xs text-deny">
                {errors.name.message}
              </span>
            )}
          </label>

          <label className="flex flex-col gap-1 text-sm">
            <span className="font-medium">描述（可选）</span>
            <input
              {...register('description')}
              aria-label="描述"
              className="rounded-card border border-border bg-bg px-2 py-1"
            />
          </label>

          <CapabilityPicker value={capabilities} onChange={setCapabilities} />
          {errors.capabilities && (
            <span role="alert" className="text-xs text-deny">
              {errors.capabilities.message}
            </span>
          )}

          <fieldset className="flex flex-col gap-2">
            <legend className="text-sm font-medium">继承自（可选 · 多选，仅已存在角色）</legend>
            {parentCandidates.length === 0 ? (
              <span className="text-xs text-text-muted">暂无其它角色可继承</span>
            ) : (
              parentCandidates.map((parent) => (
                <label key={parent.id} className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={inheritsFrom.includes(parent.name)}
                    onChange={(e) => toggleParent(parent.name, e.target.checked)}
                    aria-label={`继承 ${parent.name}`}
                  />
                  <span className="font-mono">{parent.name}</span>
                </label>
              ))
            )}
            <span className="text-xs text-text-muted">⚠ 成环的父角色由 daemon 校验拒绝</span>
          </fieldset>

          <EffectivePreview effective={effective} verbCount={capabilities.length} />
        </form>
      ) : (
        <SummaryView
          name={name}
          editing={editing}
          capabilities={capabilities}
          inheritsFrom={inheritsFrom}
          effective={effective}
        />
      )}
    </FormDrawer>
  );
}

function EffectivePreview({
  effective,
  verbCount,
}: {
  effective: RoleCapability[];
  verbCount: number;
}) {
  return (
    <div className="flex flex-col gap-1 border-t border-border pt-3">
      <span className="text-sm font-medium">有效动词集预览</span>
      <span className="text-xs text-text-muted">本地拼装 · 最终以 daemon 为准</span>
      <div className="flex flex-wrap gap-1" aria-label="有效动词集预览">
        {verbCount === 0 ? (
          <span className="text-xs text-text-muted">—（至少勾一个动词）</span>
        ) : (
          effective.map((rc) => (
            <CapabilityActionBadge key={rc.capability} capability={rc.capability} action={rc.action} />
          ))
        )}
      </div>
    </div>
  );
}

function SummaryView({
  name,
  editing,
  capabilities,
  inheritsFrom,
  effective,
}: {
  name: string;
  editing: Role | null;
  capabilities: RoleCapability[];
  inheritsFrom: string[];
  effective: RoleCapability[];
}) {
  return (
    <div className="flex flex-col gap-4 text-sm" aria-label="写入摘要">
      <p className="text-text-muted">确认将要落库的内容：</p>
      <dl className="flex flex-col gap-3">
        <div>
          <dt className="text-xs text-text-muted">名称</dt>
          <dd className="font-mono">{name.trim()}</dd>
        </div>
        <div>
          <dt className="text-xs text-text-muted">直接动词集</dt>
          <dd className="flex flex-wrap gap-1">
            {capabilities.map((rc) => (
              <CapabilityActionBadge key={rc.capability} capability={rc.capability} action={rc.action} />
            ))}
          </dd>
        </div>
        <div>
          <dt className="text-xs text-text-muted">继承边</dt>
          <dd className="font-mono">{inheritsFrom.length ? inheritsFrom.join(', ') : '—'}</dd>
        </div>
        <div>
          <dt className="text-xs text-text-muted">有效动词集（本地预览 · daemon 为准）</dt>
          <dd className="flex flex-wrap gap-1">
            {effective.map((rc) => (
              <CapabilityActionBadge key={rc.capability} capability={rc.capability} action={rc.action} />
            ))}
          </dd>
        </div>
        {editing && (
          <div>
            <dt className="text-xs text-text-muted">乐观锁 version（编辑携带）</dt>
            <dd className="font-mono">{editing.version}</dd>
          </div>
        )}
      </dl>
    </div>
  );
}

/** Narrow a daemon-reported direct set to the form's verb union, dropping any
 * non-selectable verb (never present for a role, but satisfies the type). */
function toFormCapabilities(direct: RoleCapability[]): FormCapability[] {
  const selectable = new Set<string>(SELECTABLE_CAPABILITIES);
  return direct.filter((rc): rc is FormCapability => selectable.has(rc.capability));
}

// ── Minimal Zod resolver for RHF (avoids adding @hookform/resolvers dep) ───────

const zodResolverLite: Resolver<RoleFormValues> = async (values) => {
  const parsed = roleFormSchema.safeParse(values);
  if (parsed.success) {
    return { values: parsed.data, errors: {} };
  }
  const fieldErrors: Record<string, { type: string; message: string }> = {};
  for (const issue of parsed.error.issues) {
    const key = issue.path[0];
    if (typeof key === 'string' && !fieldErrors[key]) {
      fieldErrors[key] = { type: 'validation', message: issue.message };
    }
  }
  return { values: {}, errors: fieldErrors as never };
};
