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

  it('an allow is not expandable', () => {
    render(<DecisionBadge decision="allow" />);
    expect(screen.getByRole('button')).toBeDisabled();
  });
});
