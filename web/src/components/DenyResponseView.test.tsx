import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { DenyResponseView } from './DenyResponseView';
import type { DenyResponse } from '../api/types';

const base: DenyResponse = {
  decision: 'deny',
  denied: { resource: 'db-main', capability: 'mutate', objects: ['table:orders'] },
  reason: 'denied at rbac: no grant cell',
  your_grants: { 'db-main': ['observe', 'query'] },
  request_hint: 'postern elevate db-main mutate',
  operator_note: '请走变更单据。',
};

describe('DenyResponseView (逐字段 / operator_note 原样)', () => {
  it('renders the operator_note verbatim when present', () => {
    render(<DenyResponseView deny={base} />);
    expect(screen.getByText('请走变更单据。')).toBeInTheDocument();
  });

  it('omits operator_note entirely when the field is absent', () => {
    const noNote: DenyResponse = { ...base };
    delete (noNote as { operator_note?: string }).operator_note;
    render(<DenyResponseView deny={noNote} />);
    expect(screen.queryByText('operator_note')).not.toBeInTheDocument();
  });

  it('renders request_hint as null when the capability is ungrantable', () => {
    render(<DenyResponseView deny={{ ...base, request_hint: null }} />);
    expect(screen.getByText('null')).toBeInTheDocument();
  });
});
