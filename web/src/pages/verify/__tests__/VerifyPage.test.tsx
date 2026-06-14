import { describe, expect, it, beforeEach } from 'vitest';
import {
  render,
  screen,
  fireEvent,
  waitFor,
  within,
  cleanup,
} from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import * as fx from '../../../mocks/fixtures';
import type { VerifyReport } from '../../../api/types';
import { VerifyPage } from '../index';

/** Fresh client per render; retries off so error states resolve immediately. */
function makeClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
}

/** Surfaces the current location so navigation assertions can read it. */
function LocationProbe() {
  const loc = useLocation();
  return (
    <div data-testid="location">
      {loc.pathname}
      {loc.search}
    </div>
  );
}

function renderPage() {
  const client = makeClient();
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={['/verify']}>
        <Routes>
          <Route path="/verify" element={<VerifyPage />} />
          <Route path="/audit" element={<LocationProbe />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** Open the confirm dialog, tick the ack checkbox, click 运行. */
async function runVerifyViaDialog() {
  fireEvent.click(screen.getAllByText('运行自检')[0]!);
  const dialog = await screen.findByRole('dialog', { name: '运行红队自检？' });
  fireEvent.click(within(dialog).getByRole('checkbox'));
  fireEvent.click(within(dialog).getByRole('button', { name: '运行' }));
}

/** A report flipping one probe to FAIL with a verbatim, existence-leak gap_note. */
function failReport(): VerifyReport {
  const items = fx.verifyReport.items.map((i) => ({ ...i }));
  const idx = items.findIndex((i) => i.name === 'default_deny_unknown_resource');
  items[idx] = {
    name: 'default_deny_unknown_resource',
    pass: false,
    gap_note:
      "拒绝响应泄露了资源 'nonexistent-probe-target' 的存在性(your_grants 含被探测代号)",
  };
  return { all_pass: false, items };
}

describe('VerifyPage', () => {
  beforeEach(() => cleanup());

  it('starts in the empty (never-run) state — no fake nine, idle verdict', async () => {
    renderPage();
    expect(await screen.findByText('尚未运行红队自检')).toBeInTheDocument();
    // Overall verdict is idle, NOT green.
    expect(screen.getByLabelText(/整体判定：尚未运行/)).toBeInTheDocument();
    expect(screen.queryByText('ALL PASS')).not.toBeInTheDocument();
    // No probe rows rendered before a run.
    expect(screen.queryByText('scope_out_mutate')).not.toBeInTheDocument();
  });

  it('running requires explicit checkbox ack; 运行 disabled until ticked', async () => {
    renderPage();
    await screen.findByText('尚未运行红队自检');
    fireEvent.click(screen.getAllByText('运行自检')[0]!);
    const dialog = await screen.findByRole('dialog', { name: '运行红队自检？' });
    const runBtn = within(dialog).getByRole('button', { name: '运行' });
    // Gated until the acknowledgment checkbox is ticked.
    expect(runBtn).toBeDisabled();
    fireEvent.click(within(dialog).getByRole('checkbox'));
    expect(runBtn).toBeEnabled();
  });

  it('confirm summary previews the action (not a policy diff) + current policy_rev', async () => {
    renderPage();
    await screen.findByText('尚未运行红队自检');
    fireEvent.click(screen.getAllByText('运行自检')[0]!);
    const dialog = await screen.findByRole('dialog', { name: '运行红队自检？' });
    expect(within(dialog).getByText(/自发 9 条应被拒探针/)).toBeInTheDocument();
    expect(
      within(dialog).getByText('不改任何策略（无 policy_rev 前进）'),
    ).toBeInTheDocument();
    // policy_rev from health fixture (4187) is shown as a string, not parsed.
    expect(within(dialog).getByText(fx.health.policy_rev)).toBeInTheDocument();
  });

  it('ALL PASS: complete all-pass report → green verdict (9/9), nine PASS rows', async () => {
    renderPage();
    await screen.findByText('尚未运行红队自检');
    await runVerifyViaDialog();

    expect(await screen.findByText('ALL PASS')).toBeInTheDocument();
    expect(screen.getByText('(9/9)')).toBeInTheDocument();
    // All nine probe names render, in catalog order.
    expect(screen.getAllByText('PASS')).toHaveLength(9);
    expect(screen.getByText('scope_out_mutate')).toBeInTheDocument();
    expect(screen.getByText('redaction_probe')).toBeInTheDocument();
    // PASS rows carry no gap_note.
    expect(screen.queryByText(/泄露了资源/)).not.toBeInTheDocument();
  });

  it('VERIFY FAILED: a FAIL probe shows verbatim gap_note (existence leak), auto-expanded', async () => {
    server.use(http.post('/v1/verify', () => HttpResponse.json(failReport())));
    renderPage();
    await screen.findByText('尚未运行红队自检');
    await runVerifyViaDialog();

    expect(await screen.findByText('VERIFY FAILED')).toBeInTheDocument();
    expect(screen.getByText('(8/9)')).toBeInTheDocument();
    expect(screen.getByText('FAIL')).toBeInTheDocument();
    // gap_note rendered VERBATIM — not reworded, not summarized.
    expect(
      screen.getByText(
        "拒绝响应泄露了资源 'nonexistent-probe-target' 的存在性(your_grants 含被探测代号)",
      ),
    ).toBeInTheDocument();
    // FAIL row auto-expands → its PASS-criterion detail is visible.
    expect(screen.getByText(/不泄露该资源存在性/)).toBeInTheDocument();
  });

  it('error (500): fail-closed — ErrorState, verdict 未知, no fake nine, no fake green', async () => {
    server.use(
      http.post('/v1/verify', () =>
        HttpResponse.json({ error: { code: 'unavailable', message: 'down' } }, { status: 500 }),
      ),
    );
    renderPage();
    await screen.findByText('尚未运行红队自检');
    await runVerifyViaDialog();

    expect(await screen.findByRole('alert')).toBeInTheDocument();
    // Overall verdict is the warn "未知" state, NOT green and NOT a fabricated FAIL.
    expect(screen.getByLabelText(/整体判定：自检未能运行/)).toBeInTheDocument();
    expect(screen.queryByText('ALL PASS')).not.toBeInTheDocument();
    expect(screen.queryByText('ALL PASS')).not.toBeInTheDocument();
    // No probe rows leaked through the error.
    expect(screen.queryByText('scope_out_mutate')).not.toBeInTheDocument();
  });

  it('permission error (403): distinct "无权运行控制面动作" message, no results', async () => {
    server.use(
      http.post('/v1/verify', () =>
        HttpResponse.json({ error: { code: 'forbidden', message: 'no' } }, { status: 403 }),
      ),
    );
    renderPage();
    await screen.findByText('尚未运行红队自检');
    await runVerifyViaDialog();

    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText(/无权运行控制面动作/)).toBeInTheDocument();
    expect(screen.queryByText('PASS')).not.toBeInTheDocument();
  });

  it('incomplete report (≠9 items): fail-closed — treated as 未知, never counted as PASS', async () => {
    // Only 8 items returned — a missing probe must NOT silently pass.
    const truncated: VerifyReport = {
      all_pass: true,
      items: fx.verifyReport.items.slice(0, 8),
    };
    server.use(http.post('/v1/verify', () => HttpResponse.json(truncated)));
    renderPage();
    await screen.findByText('尚未运行红队自检');
    await runVerifyViaDialog();

    expect(await screen.findByRole('alert')).toBeInTheDocument();
    expect(screen.getByText(/报告不完整，判定无效/)).toBeInTheDocument();
    expect(screen.getByLabelText(/整体判定：自检未能运行/)).toBeInTheDocument();
    // The 8 returned items are NOT rendered as a partial-green result.
    expect(screen.queryByText('ALL PASS')).not.toBeInTheDocument();
    expect(screen.queryAllByText('PASS')).toHaveLength(0);
  });

  it('inconsistent report (all_pass=true but an item failed): fail-closed 未知', async () => {
    const lying: VerifyReport = {
      all_pass: true, // claims green …
      items: fx.verifyReport.items.map((i, n) =>
        n === 0 ? { ...i, pass: false, gap_note: 'x' } : { ...i }, // … but item 0 failed.
      ),
    };
    server.use(http.post('/v1/verify', () => HttpResponse.json(lying)));
    renderPage();
    await screen.findByText('尚未运行红队自检');
    await runVerifyViaDialog();

    expect(await screen.findByRole('alert')).toBeInTheDocument();
    expect(screen.getByText(/all_pass 与逐项结果不一致/)).toBeInTheDocument();
    expect(screen.queryByText('ALL PASS')).not.toBeInTheDocument();
  });

  it('PASS row is collapsed by default and toggles open to reveal static catalog', async () => {
    renderPage();
    await screen.findByText('尚未运行红队自检');
    await runVerifyViaDialog();
    await screen.findByText('ALL PASS');

    // Probe ①'s intent prose is hidden until its row is expanded.
    const row = screen.getByText('scope_out_mutate').closest('div');
    expect(row).not.toBeNull();
    expect(screen.queryByText(/对其 Scope 外资源发起 mutate/)).not.toBeInTheDocument();

    const toggle = screen.getByText('scope_out_mutate').closest('button')!;
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText(/对其 Scope 外资源发起 mutate/)).toBeInTheDocument();
  });

  it('audit deep-link carries the verify principal + since window (no policy_rev write)', async () => {
    renderPage();
    await screen.findByText('尚未运行红队自检');
    await runVerifyViaDialog();
    await screen.findByText('ALL PASS');

    fireEvent.click(screen.getByText('查看探针审计留痕'));
    const loc = await screen.findByTestId('location');
    expect(loc).toHaveTextContent('/audit');
    expect(loc.textContent).toMatch(/principal=verify-probe/);
    expect(loc.textContent).toMatch(/since=/);
  });

  it('policy_rev renders as a precise STRING (snowflake discipline, no Number coercion)', async () => {
    // A >2^53 policy_rev must survive verbatim (precision trap).
    const bigRev = '9007199254740993007';
    server.use(http.get('/v1/health', () => HttpResponse.json({ ...fx.health, policy_rev: bigRev })));
    renderPage();
    await screen.findByText('尚未运行红队自检');
    // SnowflakeId truncates the middle but keeps the exact value in the title.
    const node = await screen.findByTitle(bigRev);
    expect(node).toBeInTheDocument();
    // The truncation never round-trips through Number (which would lose the tail).
    expect(node.title).toBe(bigRev);
    expect(Number(bigRev).toString()).not.toBe(bigRev); // proves the precision trap is real
  });

  it('run button is disabled while a run is in flight (no concurrent re-trigger)', async () => {
    // Delay the response so the pending state is observable.
    let resolve!: () => void;
    const gate = new Promise<void>((r) => {
      resolve = r;
    });
    server.use(
      http.post('/v1/verify', async () => {
        await gate;
        return HttpResponse.json(fx.verifyReport);
      }),
    );
    renderPage();
    await screen.findByText('尚未运行红队自检');
    await runVerifyViaDialog();

    // While pending, the trigger shows 运行中… and is disabled. The banner also
    // says 运行中…, so locate the one inside a <button>.
    await waitFor(() => expect(screen.getAllByText('运行中…').length).toBeGreaterThan(0));
    const inButton = screen
      .getAllByText('运行中…')
      .map((el) => el.closest('button'))
      .find((b): b is HTMLButtonElement => b !== null);
    expect(inButton).toBeTruthy();
    expect(inButton).toBeDisabled();

    resolve();
    expect(await screen.findByText('ALL PASS')).toBeInTheDocument();
  });

  it('cancelling the confirm dialog does not trigger a run', async () => {
    renderPage();
    await screen.findByText('尚未运行红队自检');
    fireEvent.click(screen.getAllByText('运行自检')[0]!);
    const dialog = await screen.findByRole('dialog', { name: '运行红队自检？' });
    fireEvent.click(within(dialog).getByRole('button', { name: '取消' }));
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: '运行红队自检？' })).not.toBeInTheDocument(),
    );
    // Still in the never-run state — no probes, no verdict change.
    expect(screen.getByText('尚未运行红队自检')).toBeInTheDocument();
    expect(screen.queryByText('ALL PASS')).not.toBeInTheDocument();
  });
});
