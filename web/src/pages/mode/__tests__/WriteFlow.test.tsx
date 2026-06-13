import { describe, expect, it, beforeEach } from 'vitest';
import { screen, within, fireEvent, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { renderWithClient } from './render';
import { ModePage } from '../index';
import type { ModeStateRow } from '../../../api/types';

const BASE = '/v1';

beforeEach(() => {
  Object.assign(navigator, { clipboard: { writeText: () => Promise.resolve() } });
});

function rows(extra: Partial<ModeStateRow>[] = []): ModeStateRow[] {
  const global: ModeStateRow = {
    scope: null,
    mode: 'normal',
    effective_mode: 'normal',
    expires_at: null,
    version: 7,
    updated_at: '2026-06-14T03:11:00Z',
    updated_by: 'admin',
    policy_rev: '4187',
  };
  const overrides = extra.map((o, i) => ({
    scope: `res-${i}`,
    mode: 'maintain' as const,
    effective_mode: 'maintain' as const,
    expires_at: null,
    version: 2,
    updated_at: '2026-06-14T02:00:00Z',
    updated_by: 'admin',
    policy_rev: '4180',
    ...o,
  })) as ModeStateRow[];
  return [global, ...overrides];
}

/** Capture the POST body the page sends on a mode-set write. */
function captureSet(state: ModeStateRow[], onSet: (body: unknown) => void) {
  server.use(
    http.post(`${BASE}/mode`, async ({ request }) => {
      const body = (await request.json().catch(() => ({}))) as { op?: string };
      if (body.op === 'set') {
        onSet(body);
        return HttpResponse.json({ rows: state, policy_rev: '4200' });
      }
      return HttpResponse.json(state);
    }),
  );
}

/** Open the global switch drawer AFTER the board has loaded (so version is read). */
async function openGlobalDrawer() {
  // Wait for the loaded global card so global.version is resolved (not 0).
  await screen.findByRole('region', { name: '全局辖区' });
  fireEvent.click(screen.getByRole('button', { name: '切换模式' }));
  return screen.findByRole('dialog', { name: '切换模式' });
}

describe('ModePage 写流程 — 摘要预览 + 标准确认', () => {
  it('observe switch: opens drawer, previews 旧→新, confirms, sends version + ttl', async () => {
    let sent: unknown = null;
    captureSet(rows(), (b) => (sent = b));
    renderWithClient(<ModePage />);

    const drawer = await openGlobalDrawer();
    // pick observe
    fireEvent.click(within(drawer).getByRole('radio', { name: /observe/i }));
    // set a TTL
    fireEvent.change(within(drawer).getByPlaceholderText(/留空=长期/), {
      target: { value: '30' },
    });
    // summary preview shows expected version (the global row version = 7)
    expect(within(drawer).getByText(/期望 version/)).toBeInTheDocument();
    expect(within(drawer).getByTitle('7')).toBeInTheDocument();
    expect(within(drawer).getByText(/30 分钟/)).toBeInTheDocument();

    // confirm
    fireEvent.click(within(drawer).getByRole('button', { name: '确认切换' }));
    const confirm = await screen.findByRole('dialog', { name: /切换模式 — GLOBAL/ });
    fireEvent.click(within(confirm).getByRole('button', { name: '确认切换' }));

    await waitFor(() => expect(sent).not.toBeNull());
    expect(sent).toMatchObject({
      op: 'set',
      scope: null,
      mode: 'observe',
      version: 7,
      ttl_ms: 30 * 60000,
    });
  });
});

describe('ModePage 写流程 — freeze 最高危防误触', () => {
  it('freeze requires typing the jurisdiction identifier (GLOBAL), not the word "freeze"', async () => {
    let sent: unknown = null;
    captureSet(rows(), (b) => (sent = b));
    renderWithClient(<ModePage />);

    const drawer = await openGlobalDrawer();
    fireEvent.click(within(drawer).getByRole('radio', { name: /freeze/i }));

    // summary + narrowing text both state freeze rejects all verbs incl read-only.
    expect(within(drawer).getAllByText(/拒绝一切动词/).length).toBeGreaterThan(0);

    fireEvent.click(within(drawer).getByRole('button', { name: '确认切换' }));
    const confirm = await screen.findByRole('dialog', {
      name: /切到 FREEZE — GLOBAL/,
    });
    const confirmBtn = within(confirm).getByRole('button', { name: '确认冻结' });
    // disabled until the right word is typed
    expect(confirmBtn).toBeDisabled();

    // wrong word stays disabled
    fireEvent.change(within(confirm).getByRole('textbox'), {
      target: { value: 'freeze' },
    });
    expect(confirmBtn).toBeDisabled();

    // correct jurisdiction identifier unlocks
    fireEvent.change(within(confirm).getByRole('textbox'), {
      target: { value: 'GLOBAL' },
    });
    expect(confirmBtn).toBeEnabled();
    fireEvent.click(confirmBtn);

    await waitFor(() => expect(sent).not.toBeNull());
    expect(sent).toMatchObject({ op: 'set', scope: null, mode: 'freeze', version: 7 });
  });

  it('per-resource freeze confirm word is the resource code', async () => {
    let sent: unknown = null;
    captureSet(rows([{ scope: 'db-main', mode: 'maintain', effective_mode: 'maintain', version: 5 }]), (b) => (sent = b));
    renderWithClient(<ModePage />);

    fireEvent.click(await screen.findByRole('button', { name: '切换此资源' }));
    const drawer = await screen.findByRole('dialog', { name: '切换模式' });
    // scope badge prefilled with the resource code (also echoed in the summary).
    expect(within(drawer).getAllByText('db-main').length).toBeGreaterThan(0);
    fireEvent.click(within(drawer).getByRole('radio', { name: /freeze/i }));
    fireEvent.click(within(drawer).getByRole('button', { name: '确认切换' }));

    const confirm = await screen.findByRole('dialog', { name: /切到 FREEZE — db-main/ });
    const btn = within(confirm).getByRole('button', { name: '确认冻结' });
    fireEvent.change(within(confirm).getByRole('textbox'), { target: { value: 'db-main' } });
    expect(btn).toBeEnabled();
    fireEvent.click(btn);

    await waitFor(() => expect(sent).not.toBeNull());
    expect(sent).toMatchObject({ scope: 'db-main', mode: 'freeze', version: 5 });
  });
});

describe('ModePage 写流程 — 回落 normal 留痕', () => {
  it('the row "回落继承" preselects normal and previews 放宽至 normal', async () => {
    captureSet(rows([{ scope: 'db-main', mode: 'maintain', effective_mode: 'maintain' }]), () => {});
    renderWithClient(<ModePage />);

    fireEvent.click(await screen.findByRole('button', { name: '回落继承' }));
    const drawer = await screen.findByRole('dialog', { name: '切换模式' });
    // normal is the selected radio (aria-checked) and the fallback note shows.
    expect(within(drawer).getByRole('radio', { name: /normal/i })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(within(drawer).getByText(/将放宽至 normal/)).toBeInTheDocument();
  });
});

describe('ModePage 写流程 — 乐观锁 409 原样呈现，不静默重试', () => {
  it('surfaces a 409 conflict in the drawer and keeps the local view unchanged', async () => {
    // read returns global normal; set returns 409.
    server.use(
      http.post(`${BASE}/mode`, async ({ request }) => {
        const body = (await request.json().catch(() => ({}))) as { op?: string };
        if (body.op === 'set') {
          return HttpResponse.json(
            { error: { code: 'version_conflict', message: '他人已改' } },
            { status: 409 },
          );
        }
        return HttpResponse.json(rows());
      }),
    );
    renderWithClient(<ModePage />);

    const drawer = await openGlobalDrawer();
    fireEvent.click(within(drawer).getByRole('radio', { name: /observe/i }));
    fireEvent.click(within(drawer).getByRole('button', { name: '确认切换' }));
    const confirm = await screen.findByRole('dialog', { name: /切换模式 — GLOBAL/ });
    fireEvent.click(within(confirm).getByRole('button', { name: '确认切换' }));

    // 409 message rendered in the drawer footer; drawer stays open.
    expect(await screen.findByText(/乐观锁冲突/)).toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: '切换模式' })).toBeInTheDocument();
  });

  it('surfaces a non-409 write error verbatim without changing the view', async () => {
    server.use(
      http.post(`${BASE}/mode`, async ({ request }) => {
        const body = (await request.json().catch(() => ({}))) as { op?: string };
        if (body.op === 'set') {
          return HttpResponse.json(
            { error: { code: 'forbidden', message: '权限不足：越权写控制面' } },
            { status: 403 },
          );
        }
        return HttpResponse.json(rows());
      }),
    );
    renderWithClient(<ModePage />);

    const drawer = await openGlobalDrawer();
    fireEvent.click(within(drawer).getByRole('radio', { name: /observe/i }));
    fireEvent.click(within(drawer).getByRole('button', { name: '确认切换' }));
    const confirm = await screen.findByRole('dialog', { name: /切换模式 — GLOBAL/ });
    fireEvent.click(within(confirm).getByRole('button', { name: '确认切换' }));

    expect(await screen.findByText('权限不足：越权写控制面')).toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: '切换模式' })).toBeInTheDocument();
  });
});
