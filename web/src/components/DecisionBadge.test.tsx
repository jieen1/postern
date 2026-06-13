import { describe, expect, it } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { DecisionBadge } from './DecisionBadge';

describe('DecisionBadge', () => {
  it('renders allow / deny / escalate labels', () => {
    const { unmount } = render(<DecisionBadge decision="allow" />);
    expect(screen.getByText('allow')).toBeInTheDocument();
    unmount();

    const r2 = render(<DecisionBadge decision="deny" />);
    expect(screen.getByText('deny')).toBeInTheDocument();
    r2.unmount();

    render(<DecisionBadge decision="escalate" />);
    expect(screen.getByText('escalate')).toBeInTheDocument();
  });

  it('folds escalate_denied to a deny presentation (no pending state)', () => {
    render(<DecisionBadge decision="escalate_denied" />);
    expect(screen.getByText('deny')).toBeInTheDocument();
    expect(screen.queryByText('escalate')).not.toBeInTheDocument();
  });

  it('expands a deny to reveal its stage chip and reason', () => {
    render(
      <DecisionBadge decision="deny" stage="rbac" reason="denied at rbac: no grant cell" />,
    );
    const toggle = screen.getByRole('button');
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('rbac')).toBeInTheDocument();
    expect(screen.getByText(/no grant cell/)).toBeInTheDocument();
  });

  it('an allow is not expandable and renders no interactive button (no nested button)', () => {
    render(<DecisionBadge decision="allow" />);
    // A non-expandable badge is an inert <span>, never a (disabled) <button>:
    // this is what lets it live inside another interactive element (the audit
    // row toggle) without producing illegal nested buttons.
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.getByText('allow')).toBeInTheDocument();
  });

  it('renders no button when expandable=false even for an expandable-shaped deny', () => {
    // The audit row passes expandable={false} so the whole row owns the toggle;
    // the badge must NOT be interactive (would nest a button inside the row).
    render(
      <DecisionBadge
        decision="deny"
        stage="rbac"
        reason="denied at rbac: no grant cell"
        expandable={false}
      />,
    );
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('a deny with a stage/reason is still an interactive button (ApprovalsTab usage)', () => {
    // ApprovalsTab renders a standalone, expandable escalate_denied badge; that
    // path must keep its real, clickable button (no regression from bug-1 fix).
    render(<DecisionBadge decision="escalate_denied" reason="approval timed out" />);
    const toggle = screen.getByRole('button');
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText(/approval timed out/)).toBeInTheDocument();
  });
});
