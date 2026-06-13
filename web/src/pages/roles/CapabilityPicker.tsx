/**
 * CapabilityPicker — the in-drawer verb selector (06-roles.md §五, page-local).
 *
 * Five checkboxes (observe/query/mutate/execute/manage); each ticked verb gets
 * an allow/escalate action radio. `destroy` renders DISABLED + struck-through
 * (UI parity with the model rule that destroy never enters a role). There is no
 * `admin` control anywhere — it is a structural absence, not a disabled toggle.
 */

import { CapabilityBadge } from '../../components';
import type { GrantAction } from '../../api/types';
import {
  DISABLED_CAPABILITY,
  GRANT_ACTIONS,
  SELECTABLE_CAPABILITIES,
  type FormCapability,
  type RoleVerb,
} from './lib';
import { cn } from '../../lib/cn';

export function CapabilityPicker({
  value,
  onChange,
}: {
  value: FormCapability[];
  onChange: (next: FormCapability[]) => void;
}) {
  const byCap = new Map(value.map((rc) => [rc.capability, rc.action] as const));

  function toggle(cap: RoleVerb, checked: boolean) {
    if (checked) {
      onChange([...value, { capability: cap, action: 'allow' }]);
    } else {
      onChange(value.filter((rc) => rc.capability !== cap));
    }
  }

  function setAction(cap: RoleVerb, action: GrantAction) {
    onChange(value.map((rc) => (rc.capability === cap ? { ...rc, action } : rc)));
  }

  return (
    <fieldset className="flex flex-col gap-2">
      <legend className="text-sm font-medium">动词集（至少勾 1）</legend>
      {SELECTABLE_CAPABILITIES.map((cap) => {
        const checked = byCap.has(cap);
        const action = byCap.get(cap) ?? 'allow';
        return (
          <div key={cap} className="flex items-center gap-3">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={checked}
                onChange={(e) => toggle(cap, e.target.checked)}
                aria-label={cap}
              />
              <CapabilityBadge capability={cap} />
            </label>
            {checked && (
              <span
                role="radiogroup"
                aria-label={`${cap} action`}
                className="flex items-center gap-3 text-xs"
              >
                {GRANT_ACTIONS.map((act) => (
                  <label key={act} className="flex items-center gap-1">
                    <input
                      type="radio"
                      name={`action-${cap}`}
                      checked={action === act}
                      onChange={() => setAction(cap, act)}
                      aria-label={`${cap} ${act}`}
                    />
                    <span className={act === 'escalate' ? 'text-warn' : 'text-allow'}>
                      {act}
                    </span>
                  </label>
                ))}
              </span>
            )}
          </div>
        );
      })}

      {/* destroy: disabled + struck-through — un-pickable by design. */}
      <div className="flex items-center gap-2 opacity-60">
        <input
          type="checkbox"
          checked={false}
          disabled
          aria-label={DISABLED_CAPABILITY}
          title="destroy 不进任何角色"
        />
        <span className={cn('line-through')}>
          <CapabilityBadge capability={DISABLED_CAPABILITY} />
        </span>
        <span className="text-xs text-text-muted">destroy 不可勾（不进角色）</span>
      </div>
    </fieldset>
  );
}
