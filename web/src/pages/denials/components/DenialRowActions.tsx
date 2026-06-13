import { useEffect, useId, useRef, useState } from 'react';
import { MoreHorizontal } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import type { DenialSummaryRow } from '../../../api/types';
import { elevateTemplate } from '../lib';

/**
 * Row action menu (`[⋯]`) — read-only routing + a copy-template action. There
 * is NO allow/grant control here: every "adjustment" is a NAVIGATION to the
 * rule editor that owns the write (Grants/Constraints) or the audit trail. This
 * is the E7 UI landing: the ranking is a signal, never a one-click allow.
 */
export function DenialRowActions({ row }: { row: DenialSummaryRow }) {
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();
  const ref = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const principal = row.principal ?? row.principal_id ?? '';

  useEffect(() => {
    if (!open) return;
    function onDoc(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener('mousedown', onDoc);
    return () => document.removeEventListener('mousedown', onDoc);
  }, [open]);

  function go(to: string) {
    setOpen(false);
    navigate(to);
  }

  async function copyTemplate() {
    setOpen(false);
    try {
      await navigator.clipboard.writeText(elevateTemplate(row));
    } catch {
      // Clipboard unavailable — no-op (the template is also in the detail panel).
    }
  }

  return (
    <div ref={ref} className="relative inline-block text-left">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="行操作"
        className="rounded-card border border-border px-1.5 py-1 text-text-muted hover:bg-surface-2 hover:text-text"
      >
        <MoreHorizontal size={14} aria-hidden />
      </button>
      {open && (
        <div
          id={menuId}
          role="menu"
          className="absolute right-0 z-10 mt-1 w-64 rounded-card border border-border bg-surface py-1 text-left text-sm shadow-card"
        >
          <MenuItem
            onClick={() =>
              go(
                `/grants?principal=${encodeURIComponent(principal)}&resource=${encodeURIComponent(row.resource)}`,
              )
            }
          >
            跳 Grants 这一格
          </MenuItem>
          <MenuItem
            onClick={() => go(`/constraints?resource=${encodeURIComponent(row.resource)}`)}
          >
            跳 Constraints
          </MenuItem>
          <MenuItem
            onClick={() =>
              go(
                `/audit?principal=${encodeURIComponent(principal)}&decision=deny`,
              )
            }
          >
            查 deny 流水（跳 Audit）
          </MenuItem>
          <MenuItem onClick={copyTemplate}>复制 elevate 模板</MenuItem>
        </div>
      )}
    </div>
  );
}

function MenuItem({
  children,
  onClick,
}: {
  children: React.ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      className="block w-full px-3 py-1.5 text-left text-text hover:bg-surface-2"
    >
      {children}
    </button>
  );
}
