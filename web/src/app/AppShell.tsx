import type { ReactNode } from 'react';
import { NavLink } from 'react-router-dom';
import { Moon, Shield, Sun } from 'lucide-react';
import { NAV, DASHBOARD } from './nav';
import { useTheme } from './theme';
import { HealthLight } from './HealthLight';
import { GlobalEmergencyBar, isFrozen } from './GlobalEmergencyBar';
import { useModeState } from '../api/hooks';
import { cn } from '../lib/cn';

/**
 * Global app skeleton (设计系统 §4 / §5): top bar (brand + global emergency area
 * + health light + theme toggle) + left nav (four groups) + content. When the
 * global mode is freeze a full-width red pulsing banner sits above everything
 * (highest visual priority, §3.1 --freeze).
 */
export function AppShell({ children }: { children: ReactNode }) {
  const { theme, toggle } = useTheme();
  const { data: modeRows } = useModeState();
  const frozen = isFrozen(modeRows);

  return (
    <div className="flex h-full flex-col">
      {frozen && (
        <div className="animate-freeze-pulse bg-freeze px-4 py-1.5 text-center text-sm font-medium text-white">
          ❄ 全局已冻结 · 所有动词被拒绝（应急态）
        </div>
      )}

      <header className="flex items-center gap-4 border-b border-border bg-surface px-4 py-2">
        <span className="flex items-center gap-2 font-semibold">
          <Shield size={18} className="text-info" />
          postern
        </span>
        <div className="ml-auto flex items-center gap-4">
          <GlobalEmergencyBar />
          <HealthLight />
          <button
            type="button"
            onClick={toggle}
            aria-label="切换主题"
            title={theme === 'dark' ? '切换到亮色' : '切换到暗色'}
            className="text-text-muted hover:text-text"
          >
            {theme === 'dark' ? <Sun size={16} /> : <Moon size={16} />}
          </button>
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        <nav className="w-56 shrink-0 overflow-y-auto border-r border-border bg-surface px-2 py-3">
          <NavItemLink path={DASHBOARD.path} label={DASHBOARD.label} />
          {NAV.map((group) => (
            <div key={group.group} className="mt-4">
              <div className="px-2 pb-1 text-xs uppercase tracking-wide text-text-muted">
                {group.group}
              </div>
              {group.items.map((item) => (
                <NavItemLink key={item.path} path={item.path} label={item.label} />
              ))}
            </div>
          ))}
        </nav>

        <main className="min-w-0 flex-1 overflow-y-auto p-6">{children}</main>
      </div>
    </div>
  );
}

function NavItemLink({ path, label }: { path: string; label: string }) {
  return (
    <NavLink
      to={path}
      end={path === '/'}
      className={({ isActive }) =>
        cn(
          'block rounded-card px-2 py-1.5 text-sm',
          isActive ? 'bg-surface-2 text-text' : 'text-text-muted hover:bg-surface-2 hover:text-text',
        )
      }
    >
      {label}
    </NavLink>
  );
}
