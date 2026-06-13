import { useState } from 'react';
import { cn } from '../../lib/cn';
import { ApprovalsTab } from './ApprovalsTab';
import { SettingsTab } from './SettingsTab';
import { ImportExportTab } from './ImportExportTab';
import { ShutdownTab } from './ShutdownTab';

/**
 * System page (设计文档 12-system.md) — a lightweight Tab container over the
 * four small non-mode system blocks: Approvals / Settings / Import-Export /
 * Shutdown. The container holds NO security logic; it only switches the content
 * area. Each tab independently follows the base skeleton (list page / form
 * page) and the unified write flow.
 */

type TabId = 'approvals' | 'settings' | 'import-export' | 'shutdown';

const TABS: { id: TabId; label: string }[] = [
  { id: 'approvals', label: '审批队列 Approvals' },
  { id: 'settings', label: '设置 Settings' },
  { id: 'import-export', label: '导入导出' },
  { id: 'shutdown', label: '关停 Shutdown' },
];

export function SystemPage() {
  const [tab, setTab] = useState<TabId>('approvals');

  return (
    <div className="flex flex-col gap-4">
      <h1 className="text-xl font-medium">系统 System</h1>

      <div role="tablist" aria-label="系统子页" className="flex flex-wrap gap-1 border-b border-border">
        {TABS.map((t) => {
          const active = t.id === tab;
          return (
            <button
              key={t.id}
              type="button"
              role="tab"
              id={`system-tab-${t.id}`}
              aria-selected={active}
              aria-controls={`system-panel-${t.id}`}
              onClick={() => setTab(t.id)}
              className={cn(
                '-mb-px rounded-t-card border-b-2 px-3 py-2 text-sm',
                active
                  ? 'border-info text-text'
                  : 'border-transparent text-text-muted hover:text-text',
              )}
            >
              {t.label}
            </button>
          );
        })}
      </div>

      <div
        role="tabpanel"
        id={`system-panel-${tab}`}
        aria-labelledby={`system-tab-${tab}`}
      >
        {tab === 'approvals' && <ApprovalsTab />}
        {tab === 'settings' && <SettingsTab />}
        {tab === 'import-export' && <ImportExportTab />}
        {tab === 'shutdown' && <ShutdownTab />}
      </div>
    </div>
  );
}

export default SystemPage;
