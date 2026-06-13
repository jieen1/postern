import { useState } from 'react';
import { Check, ChevronDown, ChevronRight, ShieldAlert, X } from 'lucide-react';
import { Badge } from './Badge';
import { StageChip } from './StageChip';
import type { Decision, Stage } from '../api/types';
import { cn } from '../lib/cn';

/**
 * allow / deny / escalate decision badge (设计系统 §4). Fixed semantic color +
 * icon (not color alone). A deny can be expanded to show its stage + reason.
 * An escalation that folded to a deny renders as deny-flavored (escalate is
 * never a standalone "pending" state — approval is closed).
 */

function normalize(decision: Decision): 'allow' | 'deny' | 'escalate' {
  if (decision === 'allow') return 'allow';
  if (decision === 'escalate') return 'escalate';
  return 'deny'; // 'deny' and 'escalate_denied' both render as deny.
}

export function DecisionBadge({
  decision,
  stage,
  reason,
  expandable = true,
}: {
  decision: Decision;
  stage?: Stage | null;
  reason?: string;
  expandable?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const kind = normalize(decision);
  const canExpand = expandable && kind === 'deny' && (Boolean(stage) || Boolean(reason));

  const visual = {
    allow: { cls: 'border-allow/50 text-allow', icon: <Check size={12} />, label: 'allow' },
    deny: { cls: 'border-deny/50 text-deny', icon: <X size={12} />, label: 'deny' },
    escalate: { cls: 'border-warn/50 text-warn', icon: <ShieldAlert size={12} />, label: 'escalate' },
  }[kind];

  return (
    <span className="inline-flex flex-col items-start gap-1">
      <button
        type="button"
        disabled={!canExpand}
        onClick={() => canExpand && setOpen((v) => !v)}
        className={cn('inline-flex', canExpand ? 'cursor-pointer' : 'cursor-default')}
        aria-expanded={canExpand ? open : undefined}
      >
        <Badge className={visual.cls}>
          {visual.icon}
          {visual.label}
          {canExpand &&
            (open ? <ChevronDown size={12} /> : <ChevronRight size={12} />)}
        </Badge>
      </button>
      {canExpand && open && (
        <div className="flex flex-col gap-1 rounded-badge border border-border bg-surface-2 px-2 py-1 text-xs">
          {stage && (
            <div className="flex items-center gap-1">
              <span className="text-text-muted">stage:</span>
              <StageChip stage={stage} />
            </div>
          )}
          {reason && <div className="font-mono text-text-muted">{reason}</div>}
        </div>
      )}
    </span>
  );
}
