import { describe, expect, it } from 'vitest';
import { screen, fireEvent } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { renderWithQuery } from './testUtils';
import { SystemPage } from '../index';
import type { SettingRow } from '../../../api/types';

const BASE = '/v1';

const settings: SettingRow[] = [
  { key: 'approval.enabled', value: 'false', default: 'false', writable: true, version: 1, kind: 'bool' },
  { key: 'approval.on_timeout', value: 'deny', default: 'deny', writable: false, version: 1, kind: 'enum' },
  { key: 'audit.fsync', value: 'always', default: 'always', writable: true, version: 1, kind: 'enum' },
];

describe('SystemPage tab container', () => {
  it('exposes four tabs and switches the active panel', async () => {
    server.use(
      http.get(`${BASE}/settings`, () => HttpResponse.json(settings)),
      http.post(`${BASE}/approvals`, () =>
        HttpResponse.json({ items: [], page_no: 1, page_size: 20, total: 0 }),
      ),
    );
    renderWithQuery(<SystemPage />);

    const tabs = screen.getAllByRole('tab');
    expect(tabs).toHaveLength(4);
    // Default tab is Approvals.
    expect(screen.getByRole('tab', { name: '审批队列 Approvals' })).toHaveAttribute(
      'aria-selected',
      'true',
    );

    // Approvals panel is active: its section heading (not just the tab) shows.
    expect(
      screen.getByRole('heading', { name: '审批队列 Approvals' }),
    ).toBeInTheDocument();

    // Switch to Settings → its locked key renders, the approvals section heading
    // is gone (the tab button keeps its label, but the panel heading swaps).
    fireEvent.click(screen.getByRole('tab', { name: '设置 Settings' }));
    expect(await screen.findByText('approval.on_timeout')).toBeInTheDocument();
    expect(
      screen.queryByRole('heading', { name: '审批队列 Approvals' }),
    ).not.toBeInTheDocument();

    // Switch to Shutdown → danger control present.
    fireEvent.click(screen.getByRole('tab', { name: '关停 Shutdown' }));
    expect(screen.getByText('关停 daemon')).toBeInTheDocument();
  });
});
