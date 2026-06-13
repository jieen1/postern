/**
 * CapabilityActionBadge — a CapabilityBadge plus its allow/escalate action
 * micro-tag (06-roles.md §三: 每徽章带 action 微角标).
 *
 * The shared `CapabilityBadge` is read-only and carries only the verb color; it
 * has no slot for the per-verb action. Rather than mutate the shared component
 * we compose it here with a tiny adjacent tag. The action is conveyed by text
 * (not color alone) for a11y: `escalate` reads as `↑` + the warn token.
 */

import { CapabilityBadge } from '../../components';
import type { Capability, GrantAction } from '../../api/types';
import { cn } from '../../lib/cn';

export function CapabilityActionBadge({
  capability,
  action,
}: {
  capability: Capability;
  action: GrantAction;
}) {
  return (
    <span className="inline-flex items-center gap-0.5">
      <CapabilityBadge capability={capability} />
      <span
        aria-label={`action ${action}`}
        title={action}
        className={cn(
          'rounded-badge px-1 text-[10px] font-medium leading-none',
          action === 'escalate' ? 'text-warn' : 'text-allow',
        )}
      >
        {action === 'escalate' ? '↑esc' : 'allow'}
      </span>
    </span>
  );
}
