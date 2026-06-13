/**
 * RoleRowActions — the per-row `[⋯]` action menu (06-roles.md §二/§四).
 * Edit verbs / edit inheritance both open the same edit drawer; delete opens the
 * danger ConfirmDialog. A tiny disclosure menu, keyboard-reachable.
 */

import { useState } from 'react';
import { MoreHorizontal } from 'lucide-react';
import type { Role } from '../../api/types';

export function RoleRowActions({
  role,
  onEdit,
  onDelete,
}: {
  role: Role;
  onEdit: (role: Role) => void;
  onDelete: (role: Role) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="relative inline-block text-left">
      <button
        type="button"
        aria-label={`角色 ${role.name} 操作`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="rounded-card border border-border px-2 py-1 text-text-muted hover:bg-surface-2 hover:text-text"
      >
        <MoreHorizontal size={14} />
      </button>
      {open && (
        <div
          role="menu"
          className="absolute right-0 z-10 mt-1 flex w-40 flex-col rounded-card border border-border bg-surface py-1 text-sm shadow-lg"
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onEdit(role);
            }}
            className="px-3 py-1.5 text-left hover:bg-surface-2"
          >
            编辑动词集 / 继承
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onDelete(role);
            }}
            className="px-3 py-1.5 text-left text-deny hover:bg-surface-2"
          >
            删除（逻辑删除）
          </button>
        </div>
      )}
    </div>
  );
}
