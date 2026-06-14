import { useState } from 'react';
import { Download } from 'lucide-react';
import { ConfirmDialog } from '../../components';
import { ApiError } from '../../api/client';
import { MonoTextView } from './MonoTextView';
import {
  useExportPolicy,
  useImportPolicy,
  type ImportMode,
} from './hooks';

/**
 * Tab: Import / Export (导入导出).
 *  - Export: POST /v1/export → TOML text → browser download. Read action, no
 *    confirm. Declarative policy ONLY — never credentials/secret_hash/real
 *    addresses/transient state.
 *  - Import: paste TOML → choose merge/overwrite → 校验 (dry-run) → diff summary
 *    (added/changed/deleted). Apply enabled ONLY when the dry-run is legal (no
 *    partial apply). Overwrite is a danger action → ConfirmDialog with a typed
 *    confirm word + the delete count. Illegal/admin-role import → whole reject.
 */

const IMPORT_CONFIRM_WORD = 'overwrite';

interface DryRunResult {
  added: number;
  changed: number;
  deleted: number;
  applied: boolean;
}

export function ImportExportTab() {
  const exportMut = useExportPolicy();
  const importMut = useImportPolicy();

  const [toml, setToml] = useState('');
  const [mode, setMode] = useState<ImportMode>('merge');
  const [dryRun, setDryRun] = useState<DryRunResult | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [applied, setApplied] = useState<string | null>(null);

  // Any edit to the source or mode invalidates a prior validation result.
  function onSourceChange(next: string) {
    setToml(next);
    setDryRun(null);
    setApplied(null);
    importMut.reset();
  }
  function onModeChange(next: ImportMode) {
    setMode(next);
    setDryRun(null);
    setApplied(null);
    importMut.reset();
  }

  function doExport() {
    exportMut.mutateAsync().then(
      (res) => downloadToml(res.toml),
      () => {
        /* error surfaced via exportMut.error */
      },
    );
  }

  function validate() {
    setApplied(null);
    importMut
      .mutateAsync({ toml, mode, dry_run: true })
      .then(
        (res) => setDryRun(res),
        () => setDryRun(null),
      );
  }

  function requestApply() {
    if (mode === 'overwrite') {
      setConfirming(true);
      return;
    }
    doApply();
  }

  function doApply() {
    setConfirming(false);
    importMut.mutateAsync({ toml, mode, dry_run: false }).then(
      (res) => {
        setApplied(`已应用 (+${res.added} ~${res.changed} -${res.deleted})`);
        setDryRun(null);
      },
      () => {
        /* error surfaced via importMut.error */
      },
    );
  }

  const importError = importMut.error;
  const isRejection = importError instanceof ApiError; // 422/4xx whole-reject
  const canApply = dryRun !== null && !importMut.isPending;

  return (
    <section aria-label="导入导出" className="flex flex-col gap-4">
      <h2 className="text-lg font-medium">导入导出 Import / Export</h2>

      <div className="grid gap-4 md:grid-cols-2">
        {/* ── Export ── */}
        <div className="flex flex-col gap-3 rounded-card border border-border p-4">
          <h3 className="font-medium">导出 Export</h3>
          <p className="text-sm text-text-muted">
            把当前声明式策略导出为 TOML（不含凭证明文或瞬时状态）。
          </p>
          <button
            type="button"
            onClick={doExport}
            disabled={exportMut.isPending}
            className="inline-flex w-fit items-center gap-1 rounded-card border border-border px-3 py-1.5 text-sm hover:enabled:bg-surface-2 disabled:opacity-40"
          >
            <Download size={14} />
            导出 TOML
          </button>
          {exportMut.isError && (
            <p role="alert" className="text-xs text-deny">
              导出失败，未下载半截文件：{(exportMut.error as Error).message}
            </p>
          )}
        </div>

        {/* ── Import ── */}
        <div className="flex flex-col gap-3 rounded-card border border-border p-4">
          <h3 className="font-medium">导入 Import</h3>
          <p className="text-sm text-text-muted">粘贴声明式策略 TOML，先校验后应用。</p>

          <fieldset className="flex items-center gap-4 text-sm">
            <legend className="sr-only">导入模式</legend>
            <label className="flex items-center gap-1">
              <input
                type="radio"
                name="import-mode"
                checked={mode === 'merge'}
                onChange={() => onModeChange('merge')}
              />
              合并
            </label>
            <label className="flex items-center gap-1">
              <input
                type="radio"
                name="import-mode"
                checked={mode === 'overwrite'}
                onChange={() => onModeChange('overwrite')}
              />
              覆盖（高危）
            </label>
          </fieldset>

          <label className="flex flex-col gap-1 text-sm">
            <span className="text-text-muted">粘贴 TOML</span>
            <textarea
              aria-label="粘贴 TOML"
              value={toml}
              onChange={(e) => onSourceChange(e.target.value)}
              rows={6}
              className="rounded-card border border-border bg-surface px-2 py-1 font-mono text-xs"
              placeholder="# 粘贴声明式策略 TOML…"
            />
          </label>

          <MonoTextView text={toml} label="预览" emptyHint="未粘贴内容" />

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={validate}
              disabled={toml.trim().length === 0 || importMut.isPending}
              className="rounded-card border border-border px-3 py-1.5 text-sm hover:enabled:bg-surface-2 disabled:opacity-40"
            >
              校验
            </button>
            <button
              type="button"
              onClick={requestApply}
              disabled={!canApply}
              className="rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40"
            >
              应用导入
            </button>
          </div>

          {dryRun && (
            <div aria-label="diff 摘要" className="rounded-card border border-border bg-surface-2 px-3 py-2 text-sm">
              <div className="mb-1 font-medium">校验通过 · diff 摘要</div>
              <div className="flex gap-4 font-mono text-xs">
                <span className="text-allow">新增 {dryRun.added}</span>
                <span className="text-warn">变更 {dryRun.changed}</span>
                <span className="text-deny">删除 {dryRun.deleted}</span>
              </div>
              {mode === 'overwrite' && dryRun.deleted > 0 && (
                <p className="mt-1 text-xs text-deny">
                  覆盖模式将删除 {dryRun.deleted} 个实体，应用需输入确认词。
                </p>
              )}
            </div>
          )}

          {importError && isRejection && (
            <p role="alert" className="text-xs text-deny">
              整体拒绝（无部分 apply，库未改）：{(importError as ApiError).message}
            </p>
          )}
          {importError && !isRejection && (
            <p role="alert" className="text-xs text-deny">
              {(importError as Error).message}
            </p>
          )}
          {applied && (
            <p role="status" className="text-xs text-allow">
              {applied}
            </p>
          )}
        </div>
      </div>

      <ConfirmDialog
        open={confirming}
        title="确认：覆盖导入"
        confirmWord={IMPORT_CONFIRM_WORD}
        confirmLabel="覆盖应用"
        body={
          <span>
            覆盖模式将删除 {dryRun?.deleted ?? 0} 个实体且不可部分回退。输入确认词后应用。
          </span>
        }
        onConfirm={doApply}
        onCancel={() => setConfirming(false)}
      />
    </section>
  );
}

function downloadToml(toml: string) {
  const blob = new Blob([toml], { type: 'application/toml' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = 'postern-policy.toml';
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}
