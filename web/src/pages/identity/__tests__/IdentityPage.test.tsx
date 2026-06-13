import { describe, expect, it, beforeEach, vi } from 'vitest';
import { screen, fireEvent, waitFor, within, act } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { renderWithQuery } from './test-utils';
import { IdentityPage } from '../index';

const BASE = '/v1';

// Two principals beyond 2^53; one api_key credential for the agent.
const principals = [
  { id: '7300000000000000123', name: 'agent-order-bot', kind: 'agent', version: 3 },
  { id: '7300000000000000456', name: 'alice', kind: 'human', version: 1 },
];

const credentials = [
  {
    id: '7300000000000000789',
    principal: 'agent-order-bot',
    principal_id: '7300000000000000123',
    kind: 'api_key',
    trust_domain: 'mcp-local',
    expires_at: '2026-09-01T00:00:00Z',
    revoked_at: null,
    version: 2,
  },
];

function paged<T>(items: T[], url: URL) {
  const page_no = Math.max(1, Number(url.searchParams.get('page_no') ?? '1'));
  const page_size = Math.min(200, Math.max(1, Number(url.searchParams.get('page_size') ?? '20')));
  const start = (page_no - 1) * page_size;
  return { items: items.slice(start, start + page_size), page_no, page_size, total: items.length };
}

/** Default per-file handlers (overridable per test). */
function useDefaults() {
  server.use(
    http.get(`${BASE}/principals`, ({ request }) =>
      HttpResponse.json(paged(principals, new URL(request.url))),
    ),
    http.get(`${BASE}/credentials`, ({ request }) =>
      HttpResponse.json(paged(credentials, new URL(request.url))),
    ),
  );
}

beforeEach(() => {
  vi.restoreAllMocks();
  // jsdom lacks clipboard; provide a stub so copy buttons don't throw.
  Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
});

describe('IdentityPage — 渲染与左栏主体名册', () => {
  it('renders the title, primary action, and principal rows', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    expect(
      screen.getByRole('heading', { name: /主体与凭证 Principals \/ Credentials/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /新建主体/ })).toBeInTheDocument();
    expect(await screen.findByText('agent-order-bot')).toBeInTheDocument();
    expect(screen.getByText('alice')).toBeInTheDocument();
  });

  it('shows snowflake id as a string verbatim (no precision loss)', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    // The full id is in the title attribute, unrounded (Number() would corrupt it).
    expect(await screen.findByTitle('7300000000000000123')).toBeInTheDocument();
    // The rounded number must NOT appear.
    expect(screen.queryByTitle('7300000000000000000')).not.toBeInTheDocument();
  });

  it('shows the active credential count for the agent (and 0 for alice)', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    // agent-order-bot row has its active credential count = 1.
    const agentRow = (await screen.findByText('agent-order-bot')).closest('tr')!;
    expect(within(agentRow).getByText('1')).toBeInTheDocument();
    const aliceRow = screen.getByText('alice').closest('tr')!;
    expect(within(aliceRow).getByText('0')).toBeInTheDocument();
  });
});

describe('IdentityPage — 筛选与搜索', () => {
  it('filters principals by kind', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    await screen.findByText('agent-order-bot');
    fireEvent.change(screen.getByLabelText('按 kind 筛选'), { target: { value: 'human' } });
    expect(screen.queryByText('agent-order-bot')).not.toBeInTheDocument();
    expect(screen.getByText('alice')).toBeInTheDocument();
  });

  it('searches principals by name', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    await screen.findByText('agent-order-bot');
    fireEvent.change(screen.getByLabelText('按名搜索'), { target: { value: 'ali' } });
    expect(screen.queryByText('agent-order-bot')).not.toBeInTheDocument();
    expect(screen.getByText('alice')).toBeInTheDocument();
  });
});

describe('IdentityPage — 右栏 master–detail 联动', () => {
  it('starts with a neutral guide (not error) before a principal is selected', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    await screen.findByText('agent-order-bot');
    expect(screen.getByText(/选择左侧一个主体查看其网关凭证/)).toBeInTheDocument();
    // No error region while merely unselected.
    expect(screen.queryByText(/凭证加载失败/)).not.toBeInTheDocument();
  });

  it('shows the selected principal credentials with derived status', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    const agentRow = (await screen.findByText('agent-order-bot')).closest('tr')!;
    fireEvent.click(within(agentRow).getByRole('button', { name: '查看凭证' }));
    // Right panel now scoped to agent-order-bot.
    expect(
      await screen.findByRole('heading', { name: /凭证 Credentials · agent-order-bot/ }),
    ).toBeInTheDocument();
    // The api_key card + an "active"/生效 derived status badge.
    expect(screen.getByText('api_key')).toBeInTheDocument();
    expect(screen.getByText('生效')).toBeInTheDocument();
  });

  it('shows an empty state for a principal with no credentials', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    const aliceRow = (await screen.findByText('alice')).closest('tr')!;
    fireEvent.click(within(aliceRow).getByRole('button', { name: '查看凭证' }));
    expect(await screen.findByText(/该主体暂无网关凭证/)).toBeInTheDocument();
  });
});

describe('IdentityPage — 三态 fail-closed', () => {
  it('renders the principals error state without leaking fake rows', async () => {
    server.use(
      http.get(`${BASE}/principals`, () =>
        HttpResponse.json({ error: { code: 'internal', message: '主体加载失败' } }, { status: 500 }),
      ),
      http.get(`${BASE}/credentials`, ({ request }) =>
        HttpResponse.json(paged(credentials, new URL(request.url))),
      ),
    );
    renderWithQuery(<IdentityPage />);
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    // No principal names leaked through the error state.
    expect(screen.queryByText('agent-order-bot')).not.toBeInTheDocument();
    expect(screen.queryByText('alice')).not.toBeInTheDocument();
  });

  it('credential load failure: count shows — (not 0) and panel refuses to show active', async () => {
    server.use(
      http.get(`${BASE}/principals`, ({ request }) =>
        HttpResponse.json(paged(principals, new URL(request.url))),
      ),
      http.get(`${BASE}/credentials`, () =>
        HttpResponse.json({ error: { code: 'internal', message: '凭证加载失败' } }, { status: 500 }),
      ),
    );
    renderWithQuery(<IdentityPage />);
    const agentRow = (await screen.findByText('agent-order-bot')).closest('tr')!;
    // Count is "—" not "0" — fail-closed, does not pretend zero.
    await waitFor(() => expect(within(agentRow).getByText('—')).toBeInTheDocument());
    expect(within(agentRow).queryByText('1')).not.toBeInTheDocument();

    fireEvent.click(within(agentRow).getByRole('button', { name: '查看凭证' }));
    // Right panel error: explicitly states it cannot confirm validity/revocation.
    expect(
      await screen.findByText(/凭证加载失败，无法确认吊销\/有效状态/),
    ).toBeInTheDocument();
    // No credential rendered as 生效 in the error state.
    expect(screen.queryByText('生效')).not.toBeInTheDocument();
  });
});

describe('IdentityPage — 契约：分页钳制', () => {
  it('requests page_size clamped to ≤200 (DB_PAGINATION_MANDATORY)', async () => {
    let credPageSize = '';
    server.use(
      http.get(`${BASE}/principals`, ({ request }) =>
        HttpResponse.json(paged(principals, new URL(request.url))),
      ),
      http.get(`${BASE}/credentials`, ({ request }) => {
        const url = new URL(request.url);
        credPageSize = url.searchParams.get('page_size') ?? '';
        return HttpResponse.json(paged(credentials, url));
      }),
    );
    renderWithQuery(<IdentityPage />);
    await screen.findByText('agent-order-bot');
    // The page asks for the legal max (200), never an unbounded size.
    await waitFor(() => expect(credPageSize).toBe('200'));
    expect(Number(credPageSize)).toBeLessThanOrEqual(200);
  });
});

describe('IdentityPage — 写流程：新建主体', () => {
  it('opens the drawer, previews a summary, and on success shows policy_rev', async () => {
    useDefaults();
    let posted: unknown = null;
    server.use(
      http.post(`${BASE}/principals`, async ({ request }) => {
        posted = await request.json();
        return HttpResponse.json({ policy_rev: '5001' });
      }),
    );
    renderWithQuery(<IdentityPage />);
    await screen.findByText('agent-order-bot');
    fireEvent.click(screen.getByRole('button', { name: /新建主体/ }));

    const form = await screen.findByRole('form', { name: '新建主体表单' });
    fireEvent.change(within(form).getByLabelText('主体名'), { target: { value: 'svc-cron' } });
    // Summary preview reflects the typed name + axiom-one fact.
    expect(await within(form).findByText(/将登记主体 svc-cron/)).toBeInTheDocument();
    expect(within(form).getByText(/默认拒绝一切/)).toBeInTheDocument();

    await act(async () => {
      fireEvent.click(within(form).getByRole('button', { name: '登记主体' }));
    });
    // Success banner cites the advanced policy_rev.
    expect(await screen.findByText(/主体已登记，policy_rev → 5001/)).toBeInTheDocument();
    expect(posted).toMatchObject({ op: 'create', name: 'svc-cron', kind: 'agent' });
  });

  it('surfaces a 409 conflict in the form without closing it', async () => {
    useDefaults();
    server.use(
      http.post(`${BASE}/principals`, () =>
        HttpResponse.json({ error: { code: 'conflict', message: 'stale' } }, { status: 409 }),
      ),
    );
    renderWithQuery(<IdentityPage />);
    await screen.findByText('agent-order-bot');
    fireEvent.click(screen.getByRole('button', { name: /新建主体/ }));
    const form = await screen.findByRole('form', { name: '新建主体表单' });
    fireEvent.change(within(form).getByLabelText('主体名'), { target: { value: 'svc-cron' } });
    await act(async () => {
      fireEvent.click(within(form).getByRole('button', { name: '登记主体' }));
    });
    // 409 → refresh-and-retry prompt, drawer stays open (no silent overwrite).
    expect(await within(form).findByText(/他人已修改该记录，请刷新后重试/)).toBeInTheDocument();
    expect(screen.getByRole('form', { name: '新建主体表单' })).toBeInTheDocument();
  });
});

describe('IdentityPage — 写流程：新建凭证 + api_key 一次性展示', () => {
  it('creates an api_key and reveals the plaintext exactly once', async () => {
    useDefaults();
    server.use(
      http.post(`${BASE}/credentials`, () =>
        HttpResponse.json({ policy_rev: '5002', api_key: 'pk_live_ONESHOT_SECRET' }),
      ),
    );
    renderWithQuery(<IdentityPage />);
    const agentRow = (await screen.findByText('agent-order-bot')).closest('tr')!;
    fireEvent.click(within(agentRow).getByRole('button', { name: '查看凭证' }));
    fireEvent.click(await screen.findByRole('button', { name: /新建凭证/ }));

    const form = await screen.findByRole('form', { name: '新建凭证表单' });
    fireEvent.change(within(form).getByLabelText('可信域'), { target: { value: 'mcp-local' } });
    await act(async () => {
      fireEvent.click(within(form).getByRole('button', { name: '创建凭证' }));
    });

    // One-time reveal box shows the plaintext + the "仅显示一次" warning.
    const reveal = await screen.findByRole('dialog', { name: 'api_key 一次性展示' });
    expect(within(reveal).getByText('pk_live_ONESHOT_SECRET')).toBeInTheDocument();
    expect(within(reveal).getByText(/此值仅显示一次，关闭后不可再获取/)).toBeInTheDocument();

    // After acknowledging, the plaintext is gone — never re-rendered in the list.
    await act(async () => {
      fireEvent.click(within(reveal).getByRole('button', { name: /我已妥存/ }));
    });
    expect(screen.queryByText('pk_live_ONESHOT_SECRET')).not.toBeInTheDocument();
  });

  it('requires a token value when kind=token (明文录入纪律)', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    const agentRow = (await screen.findByText('agent-order-bot')).closest('tr')!;
    fireEvent.click(within(agentRow).getByRole('button', { name: '查看凭证' }));
    fireEvent.click(await screen.findByRole('button', { name: /新建凭证/ }));
    const form = await screen.findByRole('form', { name: '新建凭证表单' });
    // Switch to token kind — the secret field appears.
    fireEvent.click(within(form).getByRole('radio', { name: /token/ }));
    expect(within(form).getByLabelText('令牌值')).toBeInTheDocument();
    // The secret input is a password field (not echoed as plain text).
    expect(within(form).getByLabelText('令牌值')).toHaveAttribute('type', 'password');
  });
});

describe('IdentityPage — 写流程：吊销凭证（最高危·热生效·不可逆）', () => {
  it('requires explicit typed confirmation, then transitions to revoked', async () => {
    useDefaults();
    let revokeBody: unknown = null;
    server.use(
      http.post(`${BASE}/credentials`, async ({ request }) => {
        revokeBody = await request.json();
        return HttpResponse.json({ policy_rev: '5003' });
      }),
    );
    renderWithQuery(<IdentityPage />);
    const agentRow = (await screen.findByText('agent-order-bot')).closest('tr')!;
    fireEvent.click(within(agentRow).getByRole('button', { name: '查看凭证' }));
    // Open the credential row menu → 吊销凭证.
    fireEvent.click(await screen.findByRole('button', { name: '凭证操作' }));
    fireEvent.click(screen.getByRole('menuitem', { name: '吊销凭证' }));

    const dialog = await screen.findByRole('dialog', { name: /吊销凭证（热生效·不可逆）/ });
    // The danger copy states hot-effect, irreversibility, and ≠delete.
    expect(
      within(dialog).getByText(/热生效：吊销后该凭证一切认证即时被拒/),
    ).toBeInTheDocument();
    expect(within(dialog).getByText(/不可撤销/)).toBeInTheDocument();
    expect(within(dialog).getByText(/不删除凭证记录（区别于删除）/)).toBeInTheDocument();

    // Confirm is gated on typing the exact confirm word.
    const confirmBtn = within(dialog).getByRole('button', { name: '吊销' });
    expect(confirmBtn).toBeDisabled();
    fireEvent.change(within(dialog).getByRole('textbox'), { target: { value: '吊销' } });
    expect(confirmBtn).toBeEnabled();
    await act(async () => {
      fireEvent.click(confirmBtn);
    });

    // Revoke posts op=revoke + the credential's expected version (optimistic lock).
    await waitFor(() => expect(revokeBody).toMatchObject({ op: 'revoke', version: 2 }));
    expect(await screen.findByText(/凭证已吊销，热生效，policy_rev → 5003/)).toBeInTheDocument();
  });

  it('a revoked credential has no row actions (terminal, irreversible)', async () => {
    server.use(
      http.get(`${BASE}/principals`, ({ request }) =>
        HttpResponse.json(paged(principals, new URL(request.url))),
      ),
      http.get(`${BASE}/credentials`, ({ request }) =>
        HttpResponse.json(
          paged(
            [{ ...credentials[0], revoked_at: '2026-06-13T00:00:00Z' }],
            new URL(request.url),
          ),
        ),
      ),
    );
    renderWithQuery(<IdentityPage />);
    const agentRow = (await screen.findByText('agent-order-bot')).closest('tr')!;
    fireEvent.click(within(agentRow).getByRole('button', { name: '查看凭证' }));
    // Revoked badge present; no action menu (no revoke / un-revoke path).
    expect(await screen.findByText('已吊销')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '凭证操作' })).not.toBeInTheDocument();
  });
});

describe('IdentityPage — 契约：deny 不泄露 Scope 外存在性', () => {
  it('an out-of-scope principal id surfaces no rows and no存在性信号', async () => {
    // Empty scope (e.g. restricted operator) → empty register, not a probe-able error.
    server.use(
      http.get(`${BASE}/principals`, ({ request }) =>
        HttpResponse.json(paged([], new URL(request.url))),
      ),
      http.get(`${BASE}/credentials`, ({ request }) =>
        HttpResponse.json(paged([], new URL(request.url))),
      ),
    );
    renderWithQuery(<IdentityPage />);
    // Empty register guide — never an "id 不存在" or count for unseen principals.
    expect(await screen.findByText('暂无主体')).toBeInTheDocument();
    expect(screen.queryByText('agent-order-bot')).not.toBeInTheDocument();
    expect(screen.queryByText(/不存在/)).not.toBeInTheDocument();
  });
});

describe('IdentityPage — 写流程：删除主体（≠吊销）', () => {
  it('warns to revoke first when the principal still has active credentials', async () => {
    useDefaults();
    renderWithQuery(<IdentityPage />);
    const agentRow = (await screen.findByText('agent-order-bot')).closest('tr')!;
    // Wait for the active count to load so hasActiveCreds is true.
    await waitFor(() => expect(within(agentRow).getByText('1')).toBeInTheDocument());
    fireEvent.click(within(agentRow).getByRole('button', { name: /删除主体 agent-order-bot/ }));
    const dialog = await screen.findByRole('dialog', { name: /删除主体（逻辑删除）/ });
    expect(within(dialog).getByText(/不等于吊销其凭证/)).toBeInTheDocument();
    expect(within(dialog).getByText(/请先吊销再删除/)).toBeInTheDocument();
  });
});
