import { useState } from 'react';
import { AlertTriangle, Power } from 'lucide-react';
import { ConfirmDialog } from '../../components';
import { useShutdown } from './hooks';

/**
 * Tab: Shutdown (关停). Single danger action. Confirm via a typed word
 * `shutdown` (ConfirmDialog). On success the daemon enters graceful shutdown
 * (the rest of the UI turns "unreachable"); on FAILURE we surface a red error
 * and state explicitly "未关停" — never silently assume it stopped (fail-closed,
 * the safer side stays RUNNING).
 */

const SHUTDOWN_WORD = 'shutdown';

export function ShutdownTab() {
  const shutdown = useShutdown();
  const [confirming, setConfirming] = useState(false);

  function doShutdown() {
    setConfirming(false);
    shutdown.mutate();
  }

  return (
    <section aria-label="关停" className="flex flex-col gap-3">
      <h2 className="text-lg font-medium">关停 Shutdown</h2>

      <div className="flex flex-col gap-3 rounded-card border border-deny/40 bg-deny/5 p-4">
        <div className="flex items-start gap-2 text-sm">
          <AlertTriangle size={18} className="mt-0.5 shrink-0 text-deny" />
          <div className="flex flex-col gap-1">
            <p>关停 daemon 将停止所有服务并断开所有连接。策略状态在重启后原样恢复。</p>
          </div>
        </div>

        <button
          type="button"
          onClick={() => setConfirming(true)}
          disabled={shutdown.isPending || shutdown.isSuccess}
          className="inline-flex w-fit items-center gap-1 rounded-card bg-deny px-3 py-1.5 text-sm text-white hover:enabled:brightness-110 disabled:opacity-40"
        >
          <Power size={14} />
          关停 daemon
        </button>

        {shutdown.isSuccess && (
          <p role="status" className="text-sm text-warn">
            daemon 正在优雅关停，控制面即将不可达。
          </p>
        )}
        {shutdown.isError && (
          <p role="alert" className="text-sm text-deny">
            关停失败，daemon 未关停（仍在运行）：{(shutdown.error as Error).message}
          </p>
        )}
      </div>

      <ConfirmDialog
        open={confirming}
        title="确认：关停 daemon"
        confirmWord={SHUTDOWN_WORD}
        confirmLabel="关停"
        body="关停后所有服务停止，所有连接断开。此操作不可在 UI 内撤销。"
        onConfirm={doShutdown}
        onCancel={() => setConfirming(false)}
      />
    </section>
  );
}
