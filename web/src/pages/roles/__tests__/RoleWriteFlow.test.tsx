import { describe, expect, it, beforeEach, vi } from 'vitest';
import { screen, fireEvent, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import type { Page, Role } from '../../../api/types';
import { renderRoles, findRoleRow } from './renderRoles';

const ROLES: Role[] = [
  {
    id: '7300000000000001001',
    name: 'observer',
    effective: [
      { capability: 'observe', action: 'allow' },
      { capability: 'query', action: 'allow' },
    ],
    direct: [
      { capability: 'observe', action: 'allow' },
      { capability: 'query', action: 'allow' },
    ],
    inherits_from: [],
    version: 4,
    updated_at: '2026-06-10T00:00:00Z',
    updated_by: 'admin',
  },
];

function rolesGet(items: Role[] = ROLES) {
  return http.get('/v1/roles', () =>
    HttpResponse.json<Page<Role>>({ items, page_no: 1, page_size: 20, total: items.length }),
  );
}

beforeEach(() => {
  server.use(rolesGet());
  Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
});

describe('Roles write flow — create (06-roles.md §四)', () => {
  it('open → fill → preview summary → submit → success banner with policy_rev', async () => {
    let posted: unknown = null;
    server.use(
      http.post('/v1/roles', async ({ request }) => {
        posted = await request.json();
        return HttpResponse.json({ policy_rev: '4242' });
      }),
    );
    renderRoles();
    await findRoleRow('observer');

    fireEvent.click(screen.getByRole('button', { name: /新建角色/ }));
    const drawer = await screen.findByRole('dialog', { name: '新建角色' });
    const d = within(drawer);

    fireEvent.change(d.getByLabelText('名称'), { target: { value: 'analyst' } });
    fireEvent.click(d.getByLabelText('observe'));
    fireEvent.click(d.getByLabelText('query'));

    // Local effective preview reflects self set; labelled "daemon 为准".
    const preview = d.getByLabelText('有效动词集预览');
    expect(within(preview).getByText('observe')).toBeInTheDocument();
    expect(within(preview).getByText('query')).toBeInTheDocument();
    expect(d.getByText(/最终以 daemon 为准/)).toBeInTheDocument();

    fireEvent.click(d.getByRole('button', { name: /预览摘要/ }));
    // Summary view shows what will be written.
    expect(await d.findByLabelText('写入摘要')).toBeInTheDocument();
    expect(d.getByText('analyst')).toBeInTheDocument();

    fireEvent.click(d.getByRole('button', { name: '提交' }));

    await waitFor(() =>
      expect(screen.getByText(/角色已保存/)).toBeInTheDocument(),
    );
    // The body carried the right name + direct capabilities, no version (create).
    expect(posted).toMatchObject({
      name: 'analyst',
      capabilities: [
        { capability: 'observe', action: 'allow' },
        { capability: 'query', action: 'allow' },
      ],
      inherits_from: [],
    });
    expect((posted as { version?: number }).version).toBeUndefined();
  });

  it('per-verb escalate action is captured and sent', async () => {
    let posted: { capabilities?: { capability: string; action: string }[] } = {};
    server.use(
      http.post('/v1/roles', async ({ request }) => {
        posted = (await request.json()) as typeof posted;
        return HttpResponse.json({ policy_rev: '4243' });
      }),
    );
    renderRoles();
    await findRoleRow('observer');
    fireEvent.click(screen.getByRole('button', { name: /新建角色/ }));
    const d = within(await screen.findByRole('dialog', { name: '新建角色' }));
    fireEvent.change(d.getByLabelText('名称'), { target: { value: 'escal' } });
    fireEvent.click(d.getByLabelText('mutate'));
    // switch mutate's action to escalate
    fireEvent.click(d.getByLabelText('mutate escalate'));
    fireEvent.click(d.getByRole('button', { name: /预览摘要/ }));
    fireEvent.click(await d.findByRole('button', { name: '提交' }));
    await waitFor(() => expect(posted.capabilities).toEqual([{ capability: 'mutate', action: 'escalate' }]));
  });
});

describe('Roles write flow — admin hard-block (§六-A, SEC_ADMIN_NOT_GRANTABLE)', () => {
  it('disables submit and shows the admin notice for case/whitespace variants', async () => {
    renderRoles();
    await findRoleRow('observer');
    fireEvent.click(screen.getByRole('button', { name: /新建角色/ }));
    const d = within(await screen.findByRole('dialog', { name: '新建角色' }));

    for (const variant of ['admin', 'Admin', '  admin  ']) {
      fireEvent.change(d.getByLabelText('名称'), { target: { value: variant } });
      expect(d.getByText(/admin 不可作为可授予角色/)).toBeInTheDocument();
      expect(d.getByRole('button', { name: /预览摘要/ })).toBeDisabled();
    }
  });

  it('has NO admin control anywhere in the picker (structural absence)', async () => {
    renderRoles();
    await findRoleRow('observer');
    fireEvent.click(screen.getByRole('button', { name: /新建角色/ }));
    const d = within(await screen.findByRole('dialog', { name: '新建角色' }));
    // destroy is present but disabled; admin has no checkbox at all.
    expect(d.queryByLabelText('admin')).toBeNull();
    const destroy = d.getByLabelText('destroy');
    expect(destroy).toBeDisabled();
  });
});

describe('Roles write flow — 409 optimistic lock (§六-F)', () => {
  it('edit submit that 409s shows a refresh prompt and does NOT mutate the view', async () => {
    server.use(
      http.post('/v1/roles', () =>
        HttpResponse.json({ error: { code: 'version_conflict', message: 'stale version' } }, { status: 409 }),
      ),
    );
    renderRoles();
    await findRoleRow('observer');

    // Inline 编辑 button is directly visible (no dropdown needed).
    fireEvent.click(screen.getByRole('button', { name: /编辑角色 observer/ }));
    const d = within(await screen.findByRole('dialog', { name: '编辑角色' }));
    // edit pre-fills; go straight to summary and submit.
    fireEvent.click(d.getByRole('button', { name: /预览摘要/ }));
    // summary shows the carried version (optimistic-lock token).
    const summary = within(await d.findByLabelText('写入摘要'));
    expect(summary.getByText('乐观锁 version（编辑携带）')).toBeInTheDocument();
    expect(summary.getByText('4')).toBeInTheDocument();
    fireEvent.click(d.getByRole('button', { name: '提交' }));

    expect(await d.findByText(/乐观锁冲突 409/)).toBeInTheDocument();
    // The drawer is still open (view unchanged); no success banner.
    expect(screen.queryByText(/policy_rev 前进/)).not.toBeInTheDocument();
  });
});

describe('Roles write flow — generic write error (§七 错误信封原样转述)', () => {
  it('relays the daemon message verbatim and keeps the drawer open', async () => {
    server.use(
      http.post('/v1/roles', () =>
        HttpResponse.json({ error: { code: 'invalid_capability', message: 'frobnicate is not a verb' } }, { status: 422 }),
      ),
    );
    renderRoles();
    await findRoleRow('observer');
    fireEvent.click(screen.getByRole('button', { name: /新建角色/ }));
    const d = within(await screen.findByRole('dialog', { name: '新建角色' }));
    fireEvent.change(d.getByLabelText('名称'), { target: { value: 'x' } });
    fireEvent.click(d.getByLabelText('observe'));
    fireEvent.click(d.getByRole('button', { name: /预览摘要/ }));
    fireEvent.click(await d.findByRole('button', { name: '提交' }));
    expect(await d.findByText('frobnicate is not a verb')).toBeInTheDocument();
  });
});

describe('Roles delete — danger ConfirmDialog (§4.2)', () => {
  it('requires the explicit ack checkbox before delete fires, then posts delete_flag=1 + version', async () => {
    let posted: { delete_flag?: number; version?: number } = {};
    server.use(
      http.post('/v1/roles', async ({ request }) => {
        posted = (await request.json()) as typeof posted;
        return HttpResponse.json({ policy_rev: '4250' });
      }),
    );
    renderRoles();
    await findRoleRow('observer');

    // Inline 删除 button is directly visible (no dropdown needed).
    fireEvent.click(screen.getByRole('button', { name: /删除角色 observer/ }));
    const dialog = await screen.findByRole('dialog', { name: /删除角色/ });
    const c = within(dialog);
    expect(c.getByText(/observer.*逻辑删除/, { selector: 'p' })).toBeInTheDocument();

    // Confirm without acking → no network write.
    fireEvent.click(c.getByRole('button', { name: '删除' }));
    expect(posted.delete_flag).toBeUndefined();

    // Ack then confirm → posts delete_flag=1 with the optimistic-lock version.
    fireEvent.click(c.getByLabelText('我已知晓影响'));
    fireEvent.click(c.getByRole('button', { name: '删除' }));
    await waitFor(() => expect(posted.delete_flag).toBe(1));
    expect(posted.version).toBe(4);
    expect(await screen.findByText(/角色已保存/)).toBeInTheDocument();
  });
});
