import type { ResourceRow } from '../../../api/types';

/**
 * Row action buttons (inline, directly visible — no dropdown).
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
  return (
    <div className="flex items-center justify-end gap-1">
      <button
        type="button"
        onClick={() => onDiscover(row)}
        className="rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
      >
        探测
      </button>
      <button
        type="button"
        onClick={() => onEdit(row)}
        className="rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2"
      >
        编辑
      </button>
      <button
        type="button"
        onClick={() => onToggleEnable(row)}
        className={
          row.enable_flag
            ? 'rounded-card border border-border px-2 py-1 text-xs text-warn hover:bg-surface-2'
            : 'rounded-card border border-border px-2 py-1 text-xs text-allow hover:bg-surface-2'
        }
      >
        {row.enable_flag ? '停用' : '启用'}
      </button>
    </div>
  );
}
