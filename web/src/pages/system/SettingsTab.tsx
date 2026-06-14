import { useMemo, useState } from 'react';
import { Lock } from 'lucide-react';
import {
  LoadingSkeleton,
  ErrorState,
  ConfirmDialog,
} from '../../components';
import { useSettings } from '../../api/hooks';
import { ConflictError } from '../../api/client';
import { useSaveSettings, type SettingWrite } from './hooks';
import type { SettingRow } from '../../api/types';

/**
 * Tab: Settings (设置). Fixed key set → grouped cards + row-level edit, NOT a
 * DataTable (no pagination). Edits accumulate in "保存改动 (n)" and submit as a
 * SINGLE POST /v1/settings carrying each changed key's `version` (optimistic
 * lock). Hard rules:
 *  - approval.on_timeout is rendered LOCKED read-only (恒 deny) — no control;
 *    UI embodiment of ESCALATE_FOLDS_TO_DENY.
 *  - Enabling approval.enabled is a danger action → ConfirmDialog (checkbox).
 *  - fail-closed: if GET errors (no version), NO write is possible (read-only).
 *  - If GET returns 0 keys it is an ERROR (config-plane anomaly), not "empty".
 *  - audit.retention_days clamps to [1, RETENTION_MAX] on the client.
 *  - 409 → "他人已改，请刷新重读 version 再改".
 */

const LOCKED_KEYS = new Set(['approval.on_timeout']);
const HIGH_RISK_KEYS = new Set(['approval.enabled']);
const RETENTION_MAX = 3650;

const FSYNC_OPTIONS = ['always', 'relaxed'] as const;

interface Group {
  title: string;
  keys: string[];
}

const GROUPS: Group[] = [
  { title: '审批 Approval', keys: ['approval.enabled', 'approval.on_timeout'] },
  {
    title: '审计 Audit',
    keys: ['audit.fsync', 'audit.retention_days', 'audit.exporter.otel.enabled'],
  },
];

export function SettingsTab() {
  const settings = useSettings();
  const save = useSaveSettings();

  // Pending edits keyed by setting key (string value, pre-submit).
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [confirming, setConfirming] = useState(false);
  const [riskAck, setRiskAck] = useState(false);

  const rows = settings.data;
  const byKey = useMemo(() => indexByKey(rows), [rows]);

  const dirty = useMemo(() => collectDirty(byKey, edits), [byKey, edits]);
  const dirtyCount = dirty.length;

  // fail-closed: read error OR a 0-key config-plane anomaly → read-only error.
  if (settings.isLoading) {
    return (
      <section aria-label="设置" className="flex flex-col gap-3">
        <h2 className="text-lg font-medium">设置 Settings</h2>
        <LoadingSkeleton rows={6} />
      </section>
    );
  }
  if (settings.isError) {
    return (
      <section aria-label="设置" className="flex flex-col gap-3">
        <h2 className="text-lg font-medium">设置 Settings</h2>
        <ErrorState
          title="设置读取失败"
          message={(settings.error as Error).message}
          onRetry={() => settings.refetch()}
        />
      </section>
    );
  }
  if (!rows || rows.length === 0) {
    return (
      <section aria-label="设置" className="flex flex-col gap-3">
        <h2 className="text-lg font-medium">设置 Settings</h2>
        <ErrorState
          title="暂无设置项"
          message="未读到任何可配置项，写入已禁用。"
          onRetry={() => settings.refetch()}
        />
      </section>
    );
  }

  function setEdit(key: string, value: string) {
    setEdits((prev) => ({ ...prev, [key]: value }));
  }

  function reset() {
    setEdits({});
    save.reset();
  }

  function requestSave() {
    const enablingApproval = dirty.some(
      (d) => HIGH_RISK_KEYS.has(d.key) && d.value === 'true',
    );
    if (enablingApproval) {
      setRiskAck(false);
      setConfirming(true);
      return;
    }
    doSave();
  }

  function doSave() {
    const writes: SettingWrite[] = dirty.map((d) => ({
      key: d.key,
      value: d.value,
      version: d.version,
    }));
    save.mutateAsync(writes).then(
      () => {
        setEdits({});
        setConfirming(false);
      },
      () => {
        setConfirming(false);
      },
    );
  }

  const conflict = save.error instanceof ConflictError;

  return (
    <section aria-label="设置" className="flex flex-col gap-3">
      <header className="flex items-center justify-between">
        <h2 className="text-lg font-medium">设置 Settings</h2>
        <div className="flex items-center gap-2">
          <button
            type="button"
            disabled={dirtyCount === 0 || save.isPending}
            onClick={requestSave}
            className="rounded-card bg-info px-3 py-1.5 text-sm text-white disabled:opacity-40"
          >
            保存改动 ({dirtyCount})
          </button>
          <button
            type="button"
            disabled={dirtyCount === 0}
            onClick={reset}
            className="rounded-card border border-border px-3 py-1.5 text-sm hover:enabled:bg-surface-2 disabled:opacity-40"
          >
            还原
          </button>
        </div>
      </header>

      {save.isSuccess && dirtyCount === 0 && (
        <p role="status" className="text-sm text-allow">
          设置已保存。
        </p>
      )}
      {conflict && (
        <p role="alert" className="text-sm text-deny">
          他人已改，请刷新重读 version 再改（未覆盖）。
        </p>
      )}
      {save.isError && !conflict && (
        <p role="alert" className="text-sm text-deny">
          {(save.error as Error).message}
        </p>
      )}

      {dirtyCount > 0 && (
        <div
          aria-label="改动摘要"
          className="rounded-card border border-border bg-surface-2 px-3 py-2 text-sm"
        >
          <div className="mb-1 font-medium">改动摘要</div>
          <ul className="flex flex-col gap-1">
            {dirty.map((d) => (
              <li key={d.key} className="font-mono text-xs">
                {d.key}: <span className="text-text-muted">{d.oldValue}</span> →{' '}
                <span className="text-text">{d.value}</span>
              </li>
            ))}
          </ul>
        </div>
      )}

      {GROUPS.map((group) => (
        <fieldset
          key={group.title}
          className="flex flex-col gap-2 rounded-card border border-border p-3"
        >
          <legend className="px-1 text-sm font-medium text-text-muted">
            {group.title}
          </legend>
          {group.keys
            .map((k) => byKey.get(k))
            .filter((r): r is SettingRow => Boolean(r))
            .map((row) => (
              <SettingRowView
                key={row.key}
                row={row}
                pending={edits[row.key]}
                onChange={(v) => setEdit(row.key, v)}
              />
            ))}
        </fieldset>
      ))}


      <ConfirmDialog
        open={confirming}
        title="确认：开启审批 approval.enabled"
        confirmLabel="开启"
        body={
          <label className="flex items-start gap-2">
            <input
              type="checkbox"
              checked={riskAck}
              onChange={(e) => setRiskAck(e.target.checked)}
            />
            <span>
              我理解：开启审批后，超时未裁决的请求将自动拒绝，不可超时放行。
            </span>
          </label>
        }
        onConfirm={() => {
          if (riskAck) doSave();
        }}
        onCancel={() => setConfirming(false)}
      />
    </section>
  );
}

function SettingRowView({
  row,
  pending,
  onChange,
}: {
  row: SettingRow;
  pending: string | undefined;
  onChange: (value: string) => void;
}) {
  const locked = LOCKED_KEYS.has(row.key) || !row.writable;
  const value = pending ?? row.value;
  const changed = pending !== undefined && pending !== row.value;

  return (
    <div className="grid grid-cols-[1fr_auto] items-center gap-3 border-b border-border py-2 last:border-0">
      <div className="flex flex-col gap-0.5">
        <span className="font-mono text-sm text-text">{row.key}</span>
        <span className="text-xs text-text-muted">
          默认 {row.default}
          {locked ? ' · 只读' : ' · 可写'}
          {row.key === 'audit.retention_days' && ` · 范围 [1, ${RETENTION_MAX}]`}
        </span>
      </div>
      <div className="flex items-center justify-end gap-2">
        {changed && <span className="text-xs text-warn">已改</span>}
        {locked ? (
          <span className="inline-flex items-center gap-1 rounded-badge border border-border px-2 py-0.5 font-mono text-xs text-text-muted">
            <Lock size={12} />
            {row.value}
          </span>
        ) : (
          <SettingControl row={row} value={value} onChange={onChange} />
        )}
      </div>
    </div>
  );
}

function SettingControl({
  row,
  value,
  onChange,
}: {
  row: SettingRow;
  value: string;
  onChange: (value: string) => void;
}) {
  const label = `设置 ${row.key}`;

  if (row.kind === 'bool') {
    return (
      <label className="inline-flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          aria-label={label}
          checked={value === 'true'}
          onChange={(e) => onChange(e.target.checked ? 'true' : 'false')}
        />
        {value === 'true' ? '开启' : '关闭'}
      </label>
    );
  }

  if (row.kind === 'enum' && row.key === 'audit.fsync') {
    return (
      <select
        aria-label={label}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="rounded-card border border-border bg-surface px-2 py-1 text-sm"
      >
        {FSYNC_OPTIONS.map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </select>
    );
  }

  if (row.kind === 'int') {
    return (
      <input
        type="number"
        aria-label={label}
        value={value}
        min={1}
        max={RETENTION_MAX}
        onChange={(e) => onChange(String(clampInt(e.target.value)))}
        className="w-24 rounded-card border border-border bg-surface px-2 py-1 text-right font-mono text-sm"
      />
    );
  }

  // Fallback: plain text input for any other kind.
  return (
    <input
      type="text"
      aria-label={label}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="w-40 rounded-card border border-border bg-surface px-2 py-1 font-mono text-sm"
    />
  );
}

/** Clamp the retention int into [1, RETENTION_MAX]; empty → 1 (safe floor). */
function clampInt(raw: string): number {
  const n = Math.trunc(Number(raw));
  if (!Number.isFinite(n)) return 1;
  return Math.min(RETENTION_MAX, Math.max(1, n));
}

function indexByKey(rows: SettingRow[] | undefined): Map<string, SettingRow> {
  const m = new Map<string, SettingRow>();
  for (const r of rows ?? []) m.set(r.key, r);
  return m;
}

interface DirtyEdit {
  key: string;
  value: string;
  oldValue: string;
  version: number;
}

function collectDirty(
  byKey: Map<string, SettingRow>,
  edits: Record<string, string>,
): DirtyEdit[] {
  const out: DirtyEdit[] = [];
  for (const [key, value] of Object.entries(edits)) {
    const row = byKey.get(key);
    if (!row) continue;
    if (LOCKED_KEYS.has(key) || !row.writable) continue;
    if (value === row.value) continue;
    out.push({ key, value, oldValue: row.value, version: row.version });
  }
  return out;
}
