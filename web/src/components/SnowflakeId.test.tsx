import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { SnowflakeId } from './SnowflakeId';
import { truncateId } from '../lib/format';

describe('SnowflakeId 不丢精度', () => {
  // An id beyond 2^53: 7300000000000000123. Number(...) would round it.
  const BIG = '7300000000000000123';

  it('proves the precision trap: Number() would corrupt this id', () => {
    // Sanity: confirm the fixture actually exceeds the safe-integer range, so
    // this test is meaningful. (A numeric literal would itself be rounded by
    // the JS parser, so the trap is demonstrated via the string round-trip.)
    expect(Number(BIG) > Number.MAX_SAFE_INTEGER).toBe(true);
    // Round-tripping through Number loses the trailing digits → not the same string.
    expect(String(Number(BIG))).not.toBe(BIG);
    expect(String(Number(BIG))).toBe('7300000000000000000');
  });

  it('keeps the FULL id verbatim in the title (no number coercion)', () => {
    render(<SnowflakeId id={BIG} />);
    const el = screen.getByTitle(BIG);
    expect(el).toBeInTheDocument();
    // The displayed (truncated) text is head…tail, not the rounded number.
    expect(el).toHaveTextContent('7300…0123');
  });

  it('truncateId keeps head + tail, never reparses', () => {
    expect(truncateId(BIG)).toBe('7300…0123');
    // Short ids pass through unchanged.
    expect(truncateId('123')).toBe('123');
  });

  it('copies the exact full id string to the clipboard', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(<SnowflakeId id={BIG} />);
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: '复制完整 id' }));
    });
    expect(writeText).toHaveBeenCalledWith(BIG);
  });

  beforeEach(() => {
    vi.restoreAllMocks();
  });
});
