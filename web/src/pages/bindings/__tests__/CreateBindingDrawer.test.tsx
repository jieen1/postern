import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { screen, fireEvent, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '@/mocks/server';
import type { PrincipalRow, Role } from '@/api/types';
import { renderWithProviders } from './renderPage';
import { CreateBindingDrawer } from '../CreateBindingDrawer';
import type { ExpansionPreview } from '../api';

const BASE = '/v1';

const PRINCIPALS: PrincipalRow[] = [
  { id: '7300000000000000123', name: 'agent2', kind: 'agent', version: 7 },
];
const ROLES: Role[] = [
  {
    id: '7300000000000001001',
    name: 'maintainer',
    effective: [],
    direct: [],
    inherits_from: [],
    version: 1,
    updated_at: null,
    updated_by: null,
  },
];
const RESOURCE_CODES = ['db-main', 'redis-main', 'docker-A'];

function mockPreview(data: ExpansionPreview) {
  server.use(http.post(`${BASE}/bindings/preview`, () => HttpResponse.json(data)));
}

function setup(overrides: Partial<Parameters<typeof CreateBindingDrawer>[0]> = {}) {
  const onCreated = vi.fn();
  const onClose = vi.fn();
  renderWithProviders(
    <CreateBindingDrawer
      open
      onClose={onClose}
      principals={PRINCIPALS}
      roles={ROLES}
      resourceOptions={RESOURCE_CODES}
      onCreated={onCreated}
      {...overrides}
    />,
  );
  return { onCreated, onClose };
}

/** Fill principal + role + one selector label so a preview/submit can run. */
function fillBasics() {
  fireEvent.change(screen.getByLabelText(/Principal/i, { selector: 'select' }), {
    target: { value: 'agent2' },
  });
  fireEvent.change(screen.getByLabelText(/Role/i, { selector: 'select' }), {
    target: { value: 'maintainer' },
  });
  fireEvent.change(screen.getByLabelText('标签值 1'), { target: { value: 'A' } });
}

beforeEach(() => {
  vi.restoreAllMocks();
});
afterEach(() => server.resetHandlers());

describe('CreateBindingDrawer — selector spec is what-you-see-is-what-you-send', () => {
  it('shows the {all:[...]} JSON spec preview reflecting the rows', async () => {
    mockPreview({ expanded_resources: [], grants: [], parse_error: null });
    setup();
    fillBasics();
    fireEvent.change(screen.getByLabelText('标签键 1'), { target: { value: 'host' } });

    const preview = screen.getByLabelText('selector spec 预览');
    // The exact JSON that will be submitted is shown verbatim.
    expect(JSON.parse(preview.textContent ?? '')).toEqual({
      all: [{ key: 'host', value: 'A' }],
    });
  });
});

describe('CreateBindingDrawer — live expansion preview (fail-closed)', () => {
  it('renders the daemon-reported resource set + count', async () => {
    mockPreview({
      expanded_resources: ['docker-A'],
      grants: [{ resource: 'docker-A', capability: 'observe', action: 'allow', tier: 'logs' }],
      parse_error: null,
    });
    setup();
    fillBasics();

    expect(await screen.findByTestId('expansion-count')).toHaveTextContent('1');
    // docker-A appears in the resource set (and again in the collapsed grant matrix).
    expect(screen.getAllByText('docker-A').length).toBeGreaterThan(0);
  });

  it('empty set ⇒ amber 无匹配 fact, not an error (异常 B)', async () => {
    mockPreview({ expanded_resources: [], grants: [], parse_error: null });
    setup();
    fillBasics();

    expect(await screen.findByTestId('expansion-empty')).toHaveTextContent(
      '展开为 0 个资源（无匹配标签）',
    );
  });

  it('unparseable selector ⇒ red fail-closed "将不授予任何资源" (异常 C)', async () => {
    mockPreview({
      expanded_resources: [],
      grants: [],
      parse_error: 'bad selector token at col 3',
    });
    setup();
    fillBasics();

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('选择器语法不可解析——将不授予任何资源');
  });

  it('probe unreachable ⇒ "按未授权对待", never falls back to all resources', async () => {
    server.use(
      http.post(`${BASE}/bindings/preview`, () =>
        HttpResponse.json({ error: { code: 'down', message: 'x' } }, { status: 503 }),
      ),
    );
    setup();
    fillBasics();

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('无法计算展开——按未授权对待');
    // Fail-closed: it must NOT show any optimistic resource set.
    expect(screen.queryByText('docker-A')).not.toBeInTheDocument();
    expect(screen.queryByText('db-main')).not.toBeInTheDocument();
  });
});

describe('CreateBindingDrawer — summary preview before create', () => {
  it('blocks the summary until principal/role/scope are valid', () => {
    mockPreview({ expanded_resources: [], grants: [], parse_error: null });
    setup();
    // Nothing filled yet ⇒ the summary trigger is disabled.
    expect(screen.getByRole('button', { name: '预览摘要并创建' })).toBeDisabled();
  });

  it('opens a summary dialog echoing principal/role/scope/expansion', async () => {
    mockPreview({
      expanded_resources: ['docker-A'],
      grants: [{ resource: 'docker-A', capability: 'manage', action: 'allow', tier: 'admin' }],
      parse_error: null,
    });
    setup();
    fillBasics();
    await screen.findByTestId('expansion-count');

    fireEvent.click(screen.getByRole('button', { name: '预览摘要并创建' }));
    const dialog = await screen.findByRole('dialog', { name: '确认创建绑定' });
    expect(within(dialog).getByText('agent2')).toBeInTheDocument();
    expect(within(dialog).getByText('maintainer')).toBeInTheDocument();
    expect(within(dialog).getByTestId('summary-expansion')).toHaveTextContent('[docker-A]');
  });
});

describe('CreateBindingDrawer — create write flow', () => {
  it('creates carrying the principal read-time version, reports policy_rev↑', async () => {
    mockPreview({ expanded_resources: ['docker-A'], grants: [], parse_error: null });
    let body: Record<string, unknown> = {};
    server.use(
      http.post(`${BASE}/bindings`, async ({ request }) => {
        body = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json({ policy_rev: '5000' });
      }),
    );
    const { onCreated } = setup();
    fillBasics();
    await screen.findByTestId('expansion-count');

    fireEvent.click(screen.getByRole('button', { name: '预览摘要并创建' }));
    const dialog = await screen.findByRole('dialog', { name: '确认创建绑定' });
    fireEvent.click(within(dialog).getByRole('button', { name: '确认创建' }));

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith('5000'));
    // Optimistic-lock version is the principal's read-time version (7), front-end never invents it.
    expect(body.version).toBe(7);
    expect(body.principal).toBe('agent2');
    expect(body.scope_kind).toBe('selector');
    expect(JSON.parse(body.scope_spec as string)).toEqual({
      all: [{ key: 'host', value: 'A' }],
    });
  });

  it('on 409 conflict, prompts refresh and does NOT call onCreated (异常 F)', async () => {
    mockPreview({ expanded_resources: ['docker-A'], grants: [], parse_error: null });
    server.use(
      http.post(`${BASE}/bindings`, () =>
        HttpResponse.json(
          { error: { code: 'conflict', message: 'stale version' } },
          { status: 409 },
        ),
      ),
    );
    const { onCreated } = setup();
    fillBasics();
    await screen.findByTestId('expansion-count');

    fireEvent.click(screen.getByRole('button', { name: '预览摘要并创建' }));
    const dialog = await screen.findByRole('dialog', { name: '确认创建绑定' });
    fireEvent.click(within(dialog).getByRole('button', { name: '确认创建' }));

    expect(await screen.findByText(/他人已改、请刷新重试/)).toBeInTheDocument();
    expect(onCreated).not.toHaveBeenCalled();
  });

  it('on 422 (主体/角色不存在), shows red error and keeps inputs (异常 H)', async () => {
    mockPreview({ expanded_resources: ['docker-A'], grants: [], parse_error: null });
    server.use(
      http.post(`${BASE}/bindings`, () =>
        HttpResponse.json(
          { error: { code: 'unprocessable', message: '主体不存在' } },
          { status: 422 },
        ),
      ),
    );
    const { onCreated } = setup();
    fillBasics();
    await screen.findByTestId('expansion-count');

    fireEvent.click(screen.getByRole('button', { name: '预览摘要并创建' }));
    const dialog = await screen.findByRole('dialog', { name: '确认创建绑定' });
    fireEvent.click(within(dialog).getByRole('button', { name: '确认创建' }));

    expect(await screen.findByText('主体不存在')).toBeInTheDocument();
    expect(onCreated).not.toHaveBeenCalled();
    // Inputs are retained (drawer stays open, principal still selected).
    expect(screen.getByLabelText(/Principal/i, { selector: 'select' })).toHaveValue('agent2');
  });
});
