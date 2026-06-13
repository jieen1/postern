import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { screen, fireEvent, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { renderWithQuery } from './testUtils';
import { ImportExportTab } from '../ImportExportTab';

const BASE = '/v1';

describe('ImportExportTab', () => {
  beforeEach(() => {
    // jsdom lacks URL.createObjectURL/revokeObjectURL — patch only those methods
    // on the real URL (NOT the whole global, which MSW needs as a constructor).
    (URL as unknown as { createObjectURL: () => string }).createObjectURL = () =>
      'blob:mock';
    (URL as unknown as { revokeObjectURL: () => void }).revokeObjectURL = () => {};
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('exports declarative TOML via a download (read action, no confirm)', async () => {
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
    server.use(
      http.post(`${BASE}/export`, () =>
        HttpResponse.json({ toml: '# postern policy export\npolicy_rev = 4187\n' }),
      ),
    );
    renderWithQuery(<ImportExportTab />);

    fireEvent.click(screen.getByText('导出 TOML'));
    await waitFor(() => expect(clickSpy).toHaveBeenCalled());
    // No confirm dialog interposed for a read export.
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    clickSpy.mockRestore();
  });

  it('export failure surfaces a red error and downloads no half file', async () => {
    server.use(
      http.post(`${BASE}/export`, () =>
        HttpResponse.json({ error: { code: 'io', message: '导出失败' } }, { status: 500 }),
      ),
    );
    renderWithQuery(<ImportExportTab />);
    fireEvent.click(screen.getByText('导出 TOML'));
    expect(await screen.findByRole('alert')).toHaveTextContent('未下载半截文件');
  });

  it('validate (dry-run) shows the diff summary and only then enables 应用导入', async () => {
    server.use(
      http.post(`${BASE}/import`, () =>
        HttpResponse.json({ added: 2, changed: 1, deleted: 0, applied: false }),
      ),
    );
    renderWithQuery(<ImportExportTab />);

    // Apply is disabled until a dry-run validates.
    expect(screen.getByText('应用导入')).toBeDisabled();

    fireEvent.change(screen.getByLabelText('粘贴 TOML'), {
      target: { value: '[role.observer]\n' },
    });
    fireEvent.click(screen.getByText('校验'));

    const diff = await screen.findByLabelText('diff 摘要');
    expect(diff).toHaveTextContent('新增 2');
    expect(diff).toHaveTextContent('变更 1');
    expect(diff).toHaveTextContent('删除 0');
    expect(screen.getByText('应用导入')).toBeEnabled();
  });

  it('illegal import (e.g. admin role) is a whole-reject with no partial apply', async () => {
    server.use(
      http.post(`${BASE}/import`, () =>
        HttpResponse.json(
          { error: { code: 'invalid_admin', message: 'name="admin" 入口对称硬拒' } },
          { status: 422 },
        ),
      ),
    );
    renderWithQuery(<ImportExportTab />);

    fireEvent.change(screen.getByLabelText('粘贴 TOML'), {
      target: { value: '[role.admin]\n' },
    });
    fireEvent.click(screen.getByText('校验'));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('整体拒绝（无部分 apply，库未改）');
    expect(alert).toHaveTextContent('入口对称硬拒');
    // No diff summary, apply stays disabled.
    expect(screen.queryByLabelText('diff 摘要')).not.toBeInTheDocument();
    expect(screen.getByText('应用导入')).toBeDisabled();
  });

  it('overwrite apply requires a typed confirm word and shows the delete count', async () => {
    const seen: { dry_run: boolean; mode: string }[] = [];
    server.use(
      http.post(`${BASE}/import`, async ({ request }) => {
        const body = (await request.json()) as { dry_run: boolean; mode: string };
        seen.push(body);
        return HttpResponse.json({
          added: 1,
          changed: 0,
          deleted: 3,
          applied: !body.dry_run,
        });
      }),
    );
    renderWithQuery(<ImportExportTab />);

    fireEvent.click(screen.getByLabelText('覆盖（高危）'));
    fireEvent.change(screen.getByLabelText('粘贴 TOML'), {
      target: { value: '[role.observer]\n' },
    });
    fireEvent.click(screen.getByText('校验'));
    await screen.findByLabelText('diff 摘要');
    // Overwrite delete warning carries the count.
    expect(screen.getByText(/覆盖模式将删除 3 个实体/)).toBeInTheDocument();

    // Apply → ConfirmDialog with confirm word; apply request not yet sent.
    fireEvent.click(screen.getByText('应用导入'));
    const dialog = await screen.findByRole('dialog', { name: '确认：覆盖导入' });
    expect(dialog).toHaveTextContent('将删除 3 个实体');
    expect(seen.filter((s) => !s.dry_run)).toHaveLength(0);

    // Confirm button disabled until the word is typed.
    const confirmBtn = screen.getByText('覆盖应用');
    expect(confirmBtn).toBeDisabled();
    // The dialog's confirm-word input is the textbox inside the dialog.
    const wordInput = within(dialog).getByRole('textbox');
    fireEvent.change(wordInput, { target: { value: 'overwrite' } });
    expect(confirmBtn).toBeEnabled();

    fireEvent.click(confirmBtn);
    // Now the apply (dry_run=false, overwrite) request fires.
    await waitFor(() =>
      expect(seen.some((s) => !s.dry_run && s.mode === 'overwrite')).toBe(true),
    );
    expect(await screen.findByText(/已应用 \(\+1 ~0 -3\)/)).toBeInTheDocument();
  });

  it('merge apply needs no confirm dialog and reports the applied counts', async () => {
    server.use(
      http.post(`${BASE}/import`, async ({ request }) => {
        const body = (await request.json()) as { dry_run: boolean };
        return HttpResponse.json({ added: 2, changed: 1, deleted: 0, applied: !body.dry_run });
      }),
    );
    renderWithQuery(<ImportExportTab />);

    fireEvent.change(screen.getByLabelText('粘贴 TOML'), {
      target: { value: '[role.observer]\n' },
    });
    fireEvent.click(screen.getByText('校验'));
    await screen.findByLabelText('diff 摘要');

    fireEvent.click(screen.getByText('应用导入'));
    // No confirm dialog for merge.
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(await screen.findByText(/已应用 \(\+2 ~1 -0\)/)).toBeInTheDocument();
  });
});
