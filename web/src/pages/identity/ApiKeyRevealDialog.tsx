import { useState } from 'react';
import { Check, Copy, KeyRound } from 'lucide-react';

/**
 * api_key 创建后的一次性明文展示框（10-principals-credentials.md §五 / §4.2-5）。
 *
 * 仅展示创建响应一次性回传的明文，不读列表、不读 secret_hash。关闭后即不可再得；
 * 单按钮"我已妥存（关闭后不可再得）"。醒目提示，但非危险确认（信息提示）。
 */
export function ApiKeyRevealDialog({
  apiKey,
  onClose,
}: {
  apiKey: string;
  onClose: () => void;
}) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(apiKey);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // 剪贴板不可用时静默（明文仍可手动选择复制）。
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="api_key 一次性展示"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-md rounded-card border border-warn/50 bg-surface p-5 shadow-lg">
        <div className="mb-3 flex items-center gap-2">
          <KeyRound className="text-warn" size={18} />
          <h2 className="text-lg font-medium">api_key 已生成</h2>
        </div>
        <p className="mb-3 text-sm text-warn">
          此值仅显示一次，关闭后不可再获取，请立即妥存。
        </p>
        <div className="mb-4 flex items-center gap-2 rounded-card border border-border bg-bg p-2">
          <code className="flex-1 break-all font-mono text-xs text-text">{apiKey}</code>
          <button
            type="button"
            onClick={copy}
            aria-label="复制 api_key"
            title="复制 api_key"
            className="shrink-0 text-text-muted hover:text-text"
          >
            {copied ? <Check size={14} className="text-allow" /> : <Copy size={14} />}
          </button>
        </div>
        <div className="flex justify-end">
          <button
            type="button"
            onClick={onClose}
            className="rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
          >
            我已妥存（关闭后不可再得）
          </button>
        </div>
      </div>
    </div>
  );
}
