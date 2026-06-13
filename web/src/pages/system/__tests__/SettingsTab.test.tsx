import { describe, expect, it } from 'vitest';
import { screen, fireEvent, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { renderWithQuery } from './testUtils';
import { SettingsTab } from '../SettingsTab';
import type { SettingRow } from '../../../api/types';

const BASE = '/v1';

const settings: SettingRow[] = [
  { key: 'approval.enabled', value: 'false', default: 'false', writable: true, version: 5, kind: 'bool' },
  { key: 'approval.on_timeout', value: 'deny', default: 'deny', writable: false, version: 1, kind: 'enum' },
  { key: 'audit.fsync', value: 'always', default: 'always', writable: true, version: 7, kind: 'enum' },
  { key: 'audit.retention_days', value: '90', default: '90', writable: true, version: 2, kind: 'int' },
  { key: 'audit.exporter.otel.enabled', value: 'false', default: 'false', writable: true, version: 1, kind: 'bool' },
];

function settingsRead(rows: SettingRow[]) {
  return http.get(`${BASE}/settings`, () => HttpResponse.json(rows));
}

describe('SettingsTab', () => {
  it('renders approval.on_timeout as a LOCKED read-only deny (no control)', async () => {
    server.use(settingsRead(settings));
    renderWithQuery(<SettingsTab />);

    await screen.findByText('approval.on_timeout');
    // No editable control exists for the locked key.
    expect(screen.queryByLabelText('设置 approval.on_timeout')).not.toBeInTheDocument();
    // The on_timeout row states it is fixed and shows the deny value.
    expect(screen.getByText(/不可配（恒为 deny）/)).toBeInTheDocument();
  });

  it('accumulates dirty edits into "保存改动 (n)" with a summary preview', async () => {
    server.use(settingsRead(settings));
    renderWithQuery(<SettingsTab />);

    const fsync = await screen.findByLabelText('设置 audit.fsync');
    expect(screen.getByText('保存改动 (0)')).toBeDisabled();

    fireEvent.change(fsync, { target: { value: 'relaxed' } });
    expect(screen.getByText('保存改动 (1)')).toBeEnabled();
    // Summary preview shows old → new.
    const summary = screen.getByLabelText('改动摘要');
    expect(summary).toHaveTextContent('audit.fsync: always → relaxed');
  });

  it('submits a single POST carrying each changed key version (optimistic lock)', async () => {
    let posted: unknown = null;
    server.use(
      settingsRead(settings),
      http.post(`${BASE}/settings`, async ({ request }) => {
        posted = await request.json();
        return HttpResponse.json({ policy_rev: '4200' });
      }),
    );
    renderWithQuery(<SettingsTab />);

    fireEvent.change(await screen.findByLabelText('设置 audit.fsync'), {
      target: { value: 'relaxed' },
    });
    fireEvent.click(screen.getByText('保存改动 (1)'));

    await waitFor(() => expect(posted).not.toBeNull());
    expect(posted).toEqual({
      changes: [{ key: 'audit.fsync', value: 'relaxed', version: 7 }],
    });
    expect(await screen.findByText(/policy_rev → 4200/)).toBeInTheDocument();
  });

  it('clamps audit.retention_days into [1, 3650] on the client', async () => {
    let posted: { changes: { key: string; value: string }[] } | null = null;
    server.use(
      settingsRead(settings),
      http.post(`${BASE}/settings`, async ({ request }) => {
        posted = (await request.json()) as typeof posted;
        return HttpResponse.json({ policy_rev: '4201' });
      }),
    );
    renderWithQuery(<SettingsTab />);

    const ret = await screen.findByLabelText('设置 audit.retention_days');
    fireEvent.change(ret, { target: { value: '99999' } });
    expect((ret as HTMLInputElement).value).toBe('3650');

    fireEvent.click(screen.getByText('保存改动 (1)'));
    await waitFor(() => expect(posted).not.toBeNull());
    expect(posted!.changes[0]!.value).toBe('3650');
  });

  it('enabling approval.enabled needs the danger ConfirmDialog (checkbox) first', async () => {
    let posted: unknown = null;
    server.use(
      settingsRead(settings),
      http.post(`${BASE}/settings`, async ({ request }) => {
        posted = await request.json();
        return HttpResponse.json({ policy_rev: '4202' });
      }),
    );
    renderWithQuery(<SettingsTab />);

    fireEvent.click(await screen.findByLabelText('设置 approval.enabled'));
    fireEvent.click(screen.getByText('保存改动 (1)'));

    // ConfirmDialog appears; no request fired until acknowledged + confirmed.
    const dialog = await screen.findByRole('dialog', {
      name: '确认：开启审批 approval.enabled',
    });
    const confirmBtn = within(dialog).getByRole('button', { name: '开启' });
    fireEvent.click(confirmBtn);
    expect(posted).toBeNull(); // checkbox not ticked → blocked

    fireEvent.click(within(dialog).getByRole('checkbox', { name: /我理解/ }));
    fireEvent.click(confirmBtn);
    await waitFor(() => expect(posted).not.toBeNull());
    expect(posted).toEqual({
      changes: [{ key: 'approval.enabled', value: 'true', version: 5 }],
    });
  });

  it('surfaces a 409 as a refresh prompt and does not clear the edits', async () => {
    server.use(
      settingsRead(settings),
      http.post(`${BASE}/settings`, () =>
        HttpResponse.json({ error: { code: 'conflict', message: 'stale version' } }, { status: 409 }),
      ),
    );
    renderWithQuery(<SettingsTab />);

    fireEvent.change(await screen.findByLabelText('设置 audit.fsync'), {
      target: { value: 'relaxed' },
    });
    fireEvent.click(screen.getByText('保存改动 (1)'));

    expect(await screen.findByText(/他人已改，请刷新重读 version 再改/)).toBeInTheDocument();
    // Edits preserved (still 1 dirty) — no silent overwrite, no reset.
    expect(screen.getByText('保存改动 (1)')).toBeInTheDocument();
  });

  it('fail-closed: a read error renders an ErrorState and offers NO write', async () => {
    server.use(
      http.get(`${BASE}/settings`, () =>
        HttpResponse.json({ error: { code: 'io', message: '读取设置失败' } }, { status: 500 }),
      ),
    );
    renderWithQuery(<SettingsTab />);

    expect(await screen.findByRole('alert')).toHaveTextContent('读取设置失败');
    expect(screen.queryByText(/保存改动/)).not.toBeInTheDocument();
  });

  it('fail-closed: a 0-key response is treated as a config anomaly (error)', async () => {
    server.use(settingsRead([]));
    renderWithQuery(<SettingsTab />);

    expect(await screen.findByRole('alert')).toHaveTextContent('配置面异常');
    expect(screen.queryByText(/保存改动/)).not.toBeInTheDocument();
  });
});
