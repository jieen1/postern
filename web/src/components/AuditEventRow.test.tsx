import { describe, expect, it } from 'vitest';
import { render, screen, fireEvent, within } from '@testing-library/react';
import { AuditEventRow, type AuditPair } from './AuditEventRow';
import type { AuditEvent } from '../api/types';

function ev(over: Partial<AuditEvent>): AuditEvent {
  return {
    v: 1,
    kind: 'request',
    entry: 'mcp',
    origin: 'unix:uid=1000',
    principal: 'agent-order-bot',
    resource: 'db-main',
    capability: 'mutate',
    objects: [],
    decision: 'deny',
    stage: 'rbac',
    reason: 'denied at rbac: no grant cell (db-main, mutate) for binding observer',
    policy_rev: '4187',
    id: '7300000000000004003',
    ts: '2026-06-14T03:21:40Z',
    ...over,
  };
}

const denyPair: AuditPair = { intent: ev({}) };

describe('AuditEventRow — row expand + no nested interactive (bug-1 regression)', () => {
  it('renders the deny DecisionBadge as a non-interactive element (no nested button)', () => {
    render(<AuditEventRow pair={denyPair} />);

    // The row toggle is the only interactive control in the collapsed header;
    // the badge must NOT add a second (nested) button.
    const toggle = screen.getByRole('button');
    expect(within(toggle).queryByRole('button')).not.toBeInTheDocument();
    // The deny badge text is present, just inert.
    expect(within(toggle).getByText('deny')).toBeInTheDocument();
  });

  it('clicking the decision badge region toggles the row open (badge does not swallow the click)', () => {
    render(<AuditEventRow pair={denyPair} />);
    expect(screen.queryByText('reason')).not.toBeInTheDocument();

    // Click directly on the decision badge label — with the bug, this hit a
    // nested badge button and the row stayed collapsed.
    fireEvent.click(screen.getByText('deny'));

    // The row's expanded panel is now open.
    expect(screen.getByText('reason')).toBeInTheDocument();
  });

  it('shows the deny stage in the expanded panel (info preserved after badge lost its expander)', () => {
    render(<AuditEventRow pair={denyPair} />);
    fireEvent.click(screen.getByText('deny'));

    // stage label + the StageChip value live in the row panel now.
    expect(screen.getByText('stage')).toBeInTheDocument();
    // 'rbac' appears both on the inert badge area is NOT rendered there; the
    // StageChip in the panel carries it.
    expect(screen.getByText('rbac')).toBeInTheDocument();
  });
});
