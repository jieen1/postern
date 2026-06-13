import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { StageChip, stageOrder } from './StageChip';
import { STAGES } from '../api/types';

describe('StageChip 只渲染 10 值闭集', () => {
  it('renders every one of the 10 closed stages verbatim', () => {
    expect(STAGES).toHaveLength(10);
    for (const stage of STAGES) {
      const { unmount } = render(<StageChip stage={stage} />);
      expect(screen.getByText(stage)).toBeInTheDocument();
      unmount();
    }
  });

  it('refuses a fabricated stage (e.g. "connect") — renders unknown, not the value', () => {
    render(<StageChip stage="connect" />);
    expect(screen.queryByText('connect')).not.toBeInTheDocument();
    expect(screen.getByText('unknown')).toBeInTheDocument();
  });

  it('stageOrder reflects the closed pipeline order', () => {
    expect(stageOrder('auth')).toBe(0);
    expect(stageOrder('discover')).toBe(9);
    expect(stageOrder('rbac')).toBe(2);
  });
});
