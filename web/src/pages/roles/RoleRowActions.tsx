/**
 * RoleRowActions — inline action buttons (edit / delete) per row.
 */

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
  return (
    <div className="flex items-center justify-end gap-1">
      <button
        type="button"
        aria-label={`编辑角色 ${role.name}`}
        onClick={() => onEdit(role)}
        className="rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
      >
        编辑
      </button>
      <button
        type="button"
        aria-label={`删除角色 ${role.name}`}
        onClick={() => onDelete(role)}
        className="rounded-card border border-border px-2 py-1 text-xs text-deny hover:bg-surface-2"
      >
        删除
      </button>
    </div>
  );
}
