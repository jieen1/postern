import { describe, expect, it } from 'vitest';
import { screen, fireEvent, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { renderWithQuery } from './testUtils';
import { ApprovalsTab } from '../ApprovalsTab';
import type { ApprovalItem, SettingRow } from '../../../api/types';

const BASE = '/v1';

const enabledSettings: SettingRow[] = [
  { key: 'approval.enabled', value: 'true', default: 'false', writable: true, version: 3, kind: 'bool' },
  { key: 'approval.on_timeout', value: 'deny', default: 'deny', writable: false, version: 1, kind: 'enum' },
];
const disabledSettings: SettingRow[] = [
  { key: 'approval.enabled', value: 'false', default: 'false', writable: true, version: 1, kind: 'bool' },
];

// A snowflake id well beyond 2^53 — must survive as a string, no precision loss.
const BIG_ID = '7300000000000000123';

const pendingItem: ApprovalItem = {
  id: BIG_ID,
  principal: 'agent3',
  resource: 'db-main',
  capability: 'mutate',
  status: 'escalate→deny',
  policy_rev: '4190',
  expires_at: null,
};

function approvalsList(items: ApprovalItem[]) {
  return http.post(`${BASE}/approvals`, async ({ request }) => {
    const body = (await request.json()) as { op?: string };
    if (body.op === 'adjudicate') return HttpResponse.json({ policy_rev: '4191' });
    return HttpResponse.json({ items, page_no: 1, page_size: 20, total: items.length });
  });
}

describe('ApprovalsTab', () => {
  it('shows the EmptyState when approval is disabled and queue is empty', async () => {
    server.use(
      http.get(`${BASE}/settings`, () => HttpResponse.json(disabledSettings)),
      approvalsList([]),
    );
    renderWithQuery(<ApprovalsTab />);

    expect(await screen.findByText('暂无挂起的审批请求')).toBeInTheDocument();
    // No adjudication control exists when disabled.
    expect(screen.queryByText('裁决')).not.toBeInTheDocument();
  });

  it('renders a fail-closed ErrorState (no fake rows) when the queue read fails', async () => {
    server.use(
      http.get(`${BASE}/settings`, () => HttpResponse.json(disabledSettings)),
      http.post(`${BASE}/approvals`, () =>
        HttpResponse.json({ error: { code: 'unreachable', message: 'daemon 不可达' } }, { status: 503 }),
      ),
    );
    renderWithQuery(<ApprovalsTab />);

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('daemon 不可达');
    // No leaked rows / adjudicate controls.
    expect(screen.queryByText('agent3')).not.toBeInTheDocument();
    expect(screen.queryByText('裁决')).not.toBeInTheDocument();
  });

  it('shows the [裁决] row action only when approval.enabled=true', async () => {
    server.use(
      http.get(`${BASE}/settings`, () => HttpResponse.json(enabledSettings)),
      approvalsList([pendingItem]),
    );
    renderWithQuery(<ApprovalsTab />);

    expect(await screen.findByText('agent3')).toBeInTheDocument();
    expect(screen.getByText('裁决')).toBeInTheDocument();
  });

  it('keeps the snowflake id as a string (no precision loss) in the table and drawer', async () => {
    server.use(
      http.get(`${BASE}/settings`, () => HttpResponse.json(enabledSettings)),
      approvalsList([pendingItem]),
    );
    renderWithQuery(<ApprovalsTab />);
    await screen.findByText('agent3');

    // The full id is exposed verbatim via the SnowflakeId title attribute.
    const idEls = screen.getAllByTitle(BIG_ID);
    expect(idEls.length).toBeGreaterThan(0);
    // Round-trip through Number would corrupt the tail → assert it is intact.
    expect(String(Number(BIG_ID))).not.toBe(BIG_ID);
  });

  it('adjudicating "本次允许" requires a danger ConfirmDialog before submit', async () => {
    let adjudicated: unknown = null;
    server.use(
      http.get(`${BASE}/settings`, () => HttpResponse.json(enabledSettings)),
      http.post(`${BASE}/approvals`, async ({ request }) => {
        const body = (await request.json()) as { op?: string; decision?: string };
        if (body.op === 'adjudicate') {
          adjudicated = body;
          return HttpResponse.json({ policy_rev: '4191' });
        }
        return HttpResponse.json({ items: [pendingItem], page_no: 1, page_size: 20, total: 1 });
      }),
    );
    renderWithQuery(<ApprovalsTab />);
    fireEvent.click(await screen.findByText('裁决'));

    // Choose allow_once then submit → ConfirmDialog appears, no request yet.
    fireEvent.click(screen.getByLabelText(/本次允许/));
    fireEvent.click(screen.getByText('提交裁决'));
    expect(await screen.findByRole('dialog', { name: '确认：本次允许' })).toBeInTheDocument();
    expect(adjudicated).toBeNull();

    // Confirm → request fires with decision allow_once.
    fireEvent.click(screen.getByText('确认'));
    await waitFor(() => expect(adjudicated).not.toBeNull());
    const body = adjudicated as { decision: string; version: unknown };
    expect(body.decision).toBe('allow_once');
    // The optimistic-lock anchor is the item's policy_rev, sent verbatim as a
    // string (no Number round-trip even for this small rev).
    expect(body.version).toBe(pendingItem.policy_rev);
    expect(typeof body.version).toBe('string');
  });

  it('sends policy_rev > 2^53 as the version anchor verbatim (no Number precision loss)', async () => {
    // A policy_rev beyond JS safe-integer range — Number() would silently
    // corrupt the tail and poison the optimistic-lock anchor.
    const BIG_REV = '7300000000000000999';
    expect(String(Number(BIG_REV))).not.toBe(BIG_REV); // the trap is real

    const bigRevItem: ApprovalItem = { ...pendingItem, policy_rev: BIG_REV };
    let adjudicated: { version?: unknown; decision?: string } | null = null;
    server.use(
      http.get(`${BASE}/settings`, () => HttpResponse.json(enabledSettings)),
      http.post(`${BASE}/approvals`, async ({ request }) => {
        const reqBody = (await request.json()) as { op?: string; version?: unknown; decision?: string };
        if (reqBody.op === 'adjudicate') {
          adjudicated = reqBody;
          return HttpResponse.json({ policy_rev: '4191' });
        }
        return HttpResponse.json({ items: [bigRevItem], page_no: 1, page_size: 20, total: 1 });
      }),
    );
    renderWithQuery(<ApprovalsTab />);
    fireEvent.click(await screen.findByText('裁决'));
    // Default decision is deny → submit directly (no confirm dialog).
    fireEvent.click(screen.getByText('提交裁决'));

    await waitFor(() => expect(adjudicated).not.toBeNull());
    // Byte-for-byte the original string — not Number(BIG_REV) round-tripped.
    expect(adjudicated!.version).toBe(BIG_REV);
    expect(typeof adjudicated!.version).toBe('string');
    expect(adjudicated!.version).not.toBe(Number(BIG_REV));
    expect(adjudicated!.decision).toBe('deny');
  });

  it('surfaces a 409 conflict as a refresh prompt and does not close the drawer', async () => {
    server.use(
      http.get(`${BASE}/settings`, () => HttpResponse.json(enabledSettings)),
      http.post(`${BASE}/approvals`, async ({ request }) => {
        const body = (await request.json()) as { op?: string };
        if (body.op === 'adjudicate') {
          return HttpResponse.json(
            { error: { code: 'conflict', message: 'version stale' } },
            { status: 409 },
          );
        }
        return HttpResponse.json({ items: [pendingItem], page_no: 1, page_size: 20, total: 1 });
      }),
    );
    renderWithQuery(<ApprovalsTab />);
    fireEvent.click(await screen.findByText('裁决'));
    // Default decision is deny → submit directly (no confirm dialog).
    fireEvent.click(screen.getByText('提交裁决'));

    expect(await screen.findByText('他人已改，请刷新重读后再裁决。')).toBeInTheDocument();
    // Drawer stays open (facts still visible) — no silent success.
    expect(screen.getByRole('dialog', { name: '审批裁决' })).toBeInTheDocument();
  });
});
