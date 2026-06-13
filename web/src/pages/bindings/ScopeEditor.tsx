/**
 * ScopeEditor — the FormDrawer scope sub-form (07-bindings.md §2.2).
 *
 * selector mode: a row editor of `key:value` pairs (key is a CONTROLLED dropdown
 * limited to host/env/kind; value is free text), add/remove rows ⇒ `{all:[...]}`.
 * resource mode: a multi-select of resource codes.
 *
 * The bottom JsonViewer shows the EXACT spec text that will be submitted
 * (what-you-see-is-what-you-send). Holds ZERO expansion logic — expansion is the
 * daemon's job and is rendered by ExpansionPreview, not here.
 */

import { Plus, X } from 'lucide-react';
import type { ScopeKind } from '@/api/types';
import { ResourceCodeBadge } from '@/components';
import { JsonViewer } from './JsonViewer';
import {
  SELECTOR_KEYS,
  buildResourceSpec,
  buildSelectorSpec,
  type SelectorKey,
  type SelectorRow,
} from './scope';

export function ScopeEditor({
  kind,
  onKindChange,
  rows,
  onRowsChange,
  codes,
  onCodesChange,
  resourceOptions,
}: {
  kind: ScopeKind;
  onKindChange: (k: ScopeKind) => void;
  rows: SelectorRow[];
  onRowsChange: (rows: SelectorRow[]) => void;
  codes: string[];
  onCodesChange: (codes: string[]) => void;
  /** Resource codes available for the resource-mode multi-select. */
  resourceOptions: string[];
}) {
  function updateRow(index: number, patch: Partial<SelectorRow>) {
    onRowsChange(rows.map((r, i) => (i === index ? { ...r, ...patch } : r)));
  }
  function addRow() {
    onRowsChange([...rows, { key: 'host', value: '' }]);
  }
  function removeRow(index: number) {
    onRowsChange(rows.filter((_, i) => i !== index));
  }

  const availableCodes = resourceOptions.filter((c) => !codes.includes(c));

  return (
    <div className="flex flex-col gap-3">
      <fieldset>
        <legend className="mb-1 text-sm font-medium">Scope 类型 *</legend>
        <div className="flex gap-4 text-sm">
          {(['selector', 'resource'] as const).map((k) => (
            <label key={k} className="inline-flex items-center gap-1">
              <input
                type="radio"
                name="scope_kind"
                value={k}
                checked={kind === k}
                onChange={() => onKindChange(k)}
              />
              {k}
            </label>
          ))}
        </div>
      </fieldset>

      {kind === 'selector' ? (
        <div className="flex flex-col gap-2">
          <p className="text-xs text-text-muted">匹配标签（全部满足 all）</p>
          {rows.map((row, i) => (
            <div key={i} className="flex items-center gap-2">
              <select
                aria-label={`标签键 ${i + 1}`}
                value={row.key}
                onChange={(e) =>
                  updateRow(i, { key: e.target.value as SelectorKey })
                }
                className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
              >
                {SELECTOR_KEYS.map((k) => (
                  <option key={k} value={k}>
                    {k}
                  </option>
                ))}
              </select>
              <input
                aria-label={`标签值 ${i + 1}`}
                value={row.value}
                onChange={(e) => updateRow(i, { value: e.target.value })}
                className="flex-1 rounded-card border border-border bg-bg px-2 py-1 font-mono text-sm"
                placeholder="value"
              />
              <button
                type="button"
                onClick={() => removeRow(i)}
                aria-label={`删除标签 ${i + 1}`}
                className="text-text-muted hover:text-deny"
              >
                <X size={16} />
              </button>
            </div>
          ))}
          <button
            type="button"
            onClick={addRow}
            className="inline-flex w-fit items-center gap-1 rounded-card border border-border px-2 py-1 text-sm hover:bg-surface-2"
          >
            <Plus size={14} /> 增加一行 host:/env:/kind:
          </button>
          <div className="mt-1">
            <p className="mb-1 text-xs text-text-muted">规约预览（将提交的 JSON spec）</p>
            <JsonViewer value={buildSelectorSpec(rows)} label="selector spec 预览" />
          </div>
        </div>
      ) : (
        <div className="flex flex-col gap-2">
          <p className="text-xs text-text-muted">选择资源代号（多选）</p>
          <div className="flex flex-wrap gap-1">
            {codes.length === 0 && (
              <span className="text-xs text-text-muted">尚未选择资源</span>
            )}
            {codes.map((code) => (
              <span key={code} className="inline-flex items-center gap-0.5">
                <ResourceCodeBadge code={code} />
                <button
                  type="button"
                  onClick={() => onCodesChange(codes.filter((c) => c !== code))}
                  aria-label={`移除 ${code}`}
                  className="text-text-muted hover:text-deny"
                >
                  <X size={14} />
                </button>
              </span>
            ))}
          </div>
          {availableCodes.length > 0 && (
            <select
              aria-label="添加资源代号"
              value=""
              onChange={(e) => {
                if (e.target.value) onCodesChange([...codes, e.target.value]);
              }}
              className="w-fit rounded-card border border-border bg-bg px-2 py-1 text-sm"
            >
              <option value="">+ 添加资源…</option>
              {availableCodes.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          )}
          <div className="mt-1">
            <p className="mb-1 text-xs text-text-muted">将提交的 resource spec</p>
            <JsonViewer value={buildResourceSpec(codes) || '(空)'} label="resource spec 预览" />
          </div>
        </div>
      )}
    </div>
  );
}
