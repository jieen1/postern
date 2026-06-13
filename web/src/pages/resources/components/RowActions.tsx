import { useState } from 'react';
import { MoreVertical } from 'lucide-react';
import type { ResourceRow } from '../../../api/types';

/**
 * Row action menu (设计 §2.1 ⋮): discover / edit / disable·enable. Each item is
 * a real <button> for keyboard reachability; the menu closes on selection.
 */
export function RowActions({
  row,
  onDiscover,
  onEdit,
  onToggleEnable,
}: {
  row: ResourceRow;
  onDiscover: (row: ResourceRow) => void;
  onEdit: (row: ResourceRow) => void;
  onToggleEnable: (row: ResourceRow) => void;
}) {
  const [open, setOpen] = useState(false);

  function pick(fn: (row: ResourceRow) => void) {
    setOpen(false);
    fn(row);
  }

  return (
    <div className="relative inline-block text-left">
      <button
        type="button"
        aria-label={`资源 ${row.code} 行操作`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="rounded-card border border-border px-1.5 py-1 text-text-muted hover:bg-surface-2 hover:text-text"
      >
        <MoreVertical size={16} />
      </button>
      {open && (
        <div
          role="menu"
          className="absolute right-0 z-20 mt-1 w-40 rounded-card border border-border bg-surface py-1 text-sm shadow-lg"
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => pick(onDiscover)}
            className="block w-full px-3 py-1.5 text-left hover:bg-surface-2"
          >
            探测 discover
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => pick(onEdit)}
            className="block w-full px-3 py-1.5 text-left hover:bg-surface-2"
          >
            编辑
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => pick(onToggleEnable)}
            className="block w-full px-3 py-1.5 text-left text-warn hover:bg-surface-2"
          >
            {row.enable_flag ? '停用' : '启用'}
          </button>
        </div>
      )}
    </div>
  );
}
