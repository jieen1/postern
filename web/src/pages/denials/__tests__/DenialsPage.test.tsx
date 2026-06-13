import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { screen, fireEvent, waitFor, within, act } from '@testing-library/react';

/** Wait until the ranking table has at least one data row rendered. */
async function waitForRanking() {
  await waitFor(() =>
    expect(document.querySelector('[data-group-key]')).toBeTruthy(),
  );
}
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import type { DenialSummaryRow, Page } from '../../../api/types';
import { DenialsPage } from '../index';
import { renderPage } from './renderPage';
import { ALERT_THRESHOLD } from '../lib';

const URL = '/v1/denials/summary';

// ── fixtures ──────────────────────────────────────────────────────────────────

// principal_id beyond 2^53 (Number() would round the trailing digits).
const BIG_ID = '7300000000000000123';

const ALERT_ROW: DenialSummaryRow = {
  principal: 'agent1',
  principal_id: BIG_ID,
  resource: 'db-main',
  stage: 'rbac',
  capability: 'mutate',
  count: 37,
  intent_digest: 'sha256:a3f1…9c',
  policy_rev: '1190',
};

const MID_ROW: DenialSummaryRow = {
  principal: 'agent3',
  principal_id: '7300000000000000777',
  resource: 'svc-order',
  stage: 'classify',
  capability: 'execute',
  count: 14,
  intent_digest: 'sha256:e8d0…1a',
  policy_rev: '1188',
};

const LOW_ROW: DenialSummaryRow = {
  principal: 'agent2',
  principal_id: '7300000000000000999',
  resource: 'db-main',
  stage: 'rbac',
  capability: 'mutate',
  count: 9,
  intent_digest: 'sha256:c41a…77',
  policy_rev: '1185',
};

function pageOf(items: DenialSummaryRow[], over?: Partial<Page<DenialSummaryRow>>): Page<DenialSummaryRow> {
  return {
    items,
    page_no: 1,
    page_size: 20,
    total: items.length,
    ...over,
  };
}

/** Override the denials endpoint for one test with a fixed JSON body. */
function mockDenials(body: Page<DenialSummaryRow>) {
  server.use(http.get(URL, () => HttpResponse.json(body)));
}

beforeEach(() => {
  vi.restoreAllMocks();
  // jsdom has no clipboard by default — provide a stub for copy actions.
  Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
  // scrollIntoView is not implemented in jsdom.
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  server.resetHandlers();
});

// ── 正常渲染 + 倒序 ──────────────────────────────────────────────────────────

describe('DenialsPage · 正常渲染', () => {
  it('默认 7d 加载后渲染标题、汇总条与按 count 倒序的聚合榜', async () => {
    // Provide rows out of count-order; the table must show them count-descending.
    mockDenials(pageOf([LOW_ROW, ALERT_ROW, MID_ROW], { total: 3 }));
    renderPage(<DenialsPage />);

    expect(
      screen.getByRole('heading', { name: '拒绝分析 Denials' }),
    ).toBeInTheDocument();

    await screen.findByText('svc-order');

    // 汇总条回显窗口标签与聚合组数 = total.
    const summary = screen.getByLabelText('窗口汇总');
    expect(within(summary).getByTestId('group-count')).toHaveTextContent('3');
    expect(within(summary).getByText('近 7 天')).toBeInTheDocument();

    // Reverse count order: the data rows (those with a count cell) are 37,14,9.
    const counts = screen
      .getAllByRole('row')
      .map((r) => within(r).queryByText(/^(37|14|9)$/)?.textContent)
      .filter(Boolean);
    expect(counts).toEqual(['37', '14', '9']);
  });

  it('resource 以代号徽章呈现，绝不显真实地址/连接串', async () => {
    mockDenials(pageOf([ALERT_ROW], { total: 1 }));
    renderPage(<DenialsPage />);
    await waitForRanking();
    // Only the codename appears — no host/port/URL leaks anywhere on the page.
    expect(document.body.textContent).not.toMatch(/postgres:\/\//);
    expect(document.body.textContent).not.toMatch(/\d+\.\d+\.\d+\.\d+/);
    expect(document.body.textContent).not.toMatch(/secret_hash/);
  });
});

// ── 三态 fail-closed ─────────────────────────────────────────────────────────

describe('DenialsPage · 三态 fail-closed', () => {
  it('加载中显示骨架（role=status），不显伪数据', async () => {
    // A never-resolving handler keeps the query in loading state.
    server.use(http.get(URL, () => new Promise(() => {})));
    renderPage(<DenialsPage />);
    expect(await screen.findByRole('status')).toBeInTheDocument();
    // No ranking rows / counts leaked while loading.
    expect(screen.queryByText('37')).not.toBeInTheDocument();
  });

  it('端点错误 → ErrorState（陈述事实、可重试），绝不显空榜冒充“无拒绝”', async () => {
    server.use(
      http.get(URL, () =>
        HttpResponse.json(
          { error: { code: 'unreachable', message: 'control 端点不可达' } },
          { status: 503 },
        ),
      ),
    );
    renderPage(<DenialsPage />);

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('无法读取拒绝聚合');
    expect(alert).toHaveTextContent('control 端点不可达');
    // fail-closed: the "no denials" EmptyState text must NOT appear on error.
    expect(screen.queryByText('该窗口内无被拒事件')).not.toBeInTheDocument();
    // A retry control is offered.
    expect(screen.getByRole('button', { name: '重试' })).toBeInTheDocument();
  });

  it('真实空窗口 → EmptyState（中性“好消息”），与错误态严格区分', async () => {
    mockDenials(pageOf([], { total: 0 }));
    renderPage(<DenialsPage />);

    expect(await screen.findByText('该窗口内无被拒事件')).toBeInTheDocument();
    // It is NOT an error: no alert role, no error title.
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByText('无法读取拒绝聚合')).not.toBeInTheDocument();
  });
});

// ── 告警带 ──────────────────────────────────────────────────────────────────

describe('DenialsPage · 告警带', () => {
  it('存在超阈值组时渲染告警带，[定位] 滚动到对应行并高亮', async () => {
    mockDenials(pageOf([ALERT_ROW, LOW_ROW], { total: 2 }));
    renderPage(<DenialsPage />);

    const band = await screen.findByRole('region', { name: '告警 Alerts' });
    expect(within(band).getByText(/告警 Alerts \(1\)/)).toBeInTheDocument();
    // The alert reflects the over-threshold group's count.
    expect(within(band).getByText(/37 次/)).toBeInTheDocument();

    const scrollSpy = vi
      .spyOn(Element.prototype, 'scrollIntoView')
      .mockImplementation(() => {});
    fireEvent.click(within(band).getByRole('button', { name: /定位/ }));
    expect(scrollSpy).toHaveBeenCalled();

    // The matching ranking row is highlighted (amber tint class applied).
    const groupRow = document.querySelector('[data-group-key]');
    expect(groupRow?.className).toMatch(/bg-warn/);
  });

  it('无超阈值组时整带隐藏（不显“0 告警”绿条）', async () => {
    mockDenials(pageOf([MID_ROW, LOW_ROW], { total: 2 }));
    renderPage(<DenialsPage />);
    await screen.findByText('svc-order');
    expect(screen.queryByRole('region', { name: '告警 Alerts' })).not.toBeInTheDocument();
    expect(screen.queryByText(/告警 Alerts/)).not.toBeInTheDocument();
  });

  it('阈值常量与设计一致（≥30）', () => {
    expect(ALERT_THRESHOLD).toBe(30);
  });
});

// ── 行展开细节 ───────────────────────────────────────────────────────────────

describe('DenialsPage · 行展开细节', () => {
  it('展开行显示落点 stage、intent_digest 全展、policy_rev、principal_id 雪花不丢精度', async () => {
    mockDenials(pageOf([ALERT_ROW], { total: 1 }));
    renderPage(<DenialsPage />);
    await waitForRanking();

    fireEvent.click(screen.getByRole('button', { name: '展开细节' }));

    const panel = screen.getByRole('region', { name: '聚合组细节' });
    // Deny landing stage shown; full intent_digest + policy_rev within the panel.
    expect(within(panel).getByText(/在 rbac 阶段被拒/)).toBeInTheDocument();
    expect(within(panel).getByText('sha256:a3f1…9c')).toBeInTheDocument();
    expect(within(panel).getByText(/policy_rev/)).toBeInTheDocument();
    expect(within(panel).getByText('1190')).toBeInTheDocument();

    // principal_id rendered as a string snowflake (full value in title), never
    // a coerced number.
    const idEl = within(panel).getByTitle(BIG_ID);
    expect(idEl).toBeInTheDocument();
    expect(idEl).toHaveTextContent('7300…0123');
    // Proof the trap is real: Number() would have corrupted it.
    expect(String(Number(BIG_ID))).not.toBe(BIG_ID);
  });

  it('缺失字段（intent_digest 为空）显占位“—”，不臆造', async () => {
    const noDigest: DenialSummaryRow = { ...ALERT_ROW, intent_digest: '' };
    mockDenials(pageOf([noDigest], { total: 1 }));
    renderPage(<DenialsPage />);
    await waitForRanking();
    fireEvent.click(screen.getByRole('button', { name: '展开细节' }));
    // The digest line shows a dash placeholder, not a fabricated value.
    expect(screen.getAllByText('—').length).toBeGreaterThan(0);
    expect(screen.queryByText(/sha256/)).not.toBeInTheDocument();
  });

  it('展开区提供 elevate 机械模板（占位 TTL）且非放行按钮', async () => {
    mockDenials(pageOf([ALERT_ROW], { total: 1 }));
    renderPage(<DenialsPage />);
    await waitForRanking();
    fireEvent.click(screen.getByRole('button', { name: '展开细节' }));

    expect(
      screen.getByText('postern elevate agent1 --cap db-main:mutate --ttl <填>'),
    ).toBeInTheDocument();
    // E7: there is NO allow/grant/放行 button anywhere on the page.
    expect(screen.queryByRole('button', { name: /放行/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /allow/i })).not.toBeInTheDocument();
  });
});

// ── 行操作（只读跳转，无放行）─────────────────────────────────────────────────

describe('DenialsPage · 行操作跳转', () => {
  it('行菜单“跳 Grants 这一格”路由到 Grants 并预填 principal+resource', async () => {
    mockDenials(pageOf([ALERT_ROW], { total: 1 }));
    renderPage(<DenialsPage />);
    await waitForRanking();

    fireEvent.click(screen.getByRole('button', { name: '行操作' }));
    const menu = screen.getByRole('menu');
    fireEvent.click(within(menu).getByRole('menuitem', { name: '跳 Grants 这一格' }));

    const probe = await screen.findByTestId('location');
    await waitFor(() =>
      expect(probe.getAttribute('data-pathname')).toBe('/grants'),
    );
    const params = new URLSearchParams(probe.getAttribute('data-search') ?? '');
    expect(params.get('principal')).toBe('agent1');
    expect(params.get('resource')).toBe('db-main');
  });

  it('复制 elevate 模板写入剪贴板（机械命令，非放行）', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    mockDenials(pageOf([ALERT_ROW], { total: 1 }));
    renderPage(<DenialsPage />);
    await waitForRanking();

    fireEvent.click(screen.getByRole('button', { name: '行操作' }));
    await act(async () => {
      fireEvent.click(
        within(screen.getByRole('menu')).getByRole('menuitem', {
          name: '复制 elevate 模板',
        }),
      );
    });
    expect(writeText).toHaveBeenCalledWith(
      'postern elevate agent1 --cap db-main:mutate --ttl <填>',
    );
  });
});

// ── 窗口切换 / 刷新 / 分页契约 ─────────────────────────────────────────────────

describe('DenialsPage · 窗口/刷新/分页契约', () => {
  it('切换窗口以新 window 重新请求并回显标签', async () => {
    const seen: string[] = [];
    server.use(
      http.get(URL, ({ request }) => {
        const w = new globalThis.URL(request.url).searchParams.get('window');
        seen.push(w ?? '');
        return HttpResponse.json(pageOf([ALERT_ROW], { total: 1 }));
      }),
    );
    renderPage(<DenialsPage />);
    await waitForRanking();
    expect(seen).toContain('7d');

    fireEvent.change(screen.getByLabelText('窗口'), { target: { value: '30d' } });

    await waitFor(() => expect(seen).toContain('30d'));
    // Summary bar echoes the new window label.
    expect(await screen.findByText('近 30 天')).toBeInTheDocument();
  });

  it('刷新按钮触发对当前窗口的再次请求', async () => {
    let calls = 0;
    server.use(
      http.get(URL, () => {
        calls += 1;
        return HttpResponse.json(pageOf([ALERT_ROW], { total: 1 }));
      }),
    );
    renderPage(<DenialsPage />);
    await waitForRanking();
    const initial = calls;

    fireEvent.click(screen.getByRole('button', { name: /刷新/ }));
    await waitFor(() => expect(calls).toBeGreaterThan(initial));
  });

  it('分页：page_no/page_size 缺省 20、上限钳 200，翻页携带新 page_no 再请求', async () => {
    const seenSizes: string[] = [];
    let lastPageNo = '';
    server.use(
      http.get(URL, ({ request }) => {
        const sp = new globalThis.URL(request.url).searchParams;
        seenSizes.push(sp.get('page_size') ?? '');
        lastPageNo = sp.get('page_no') ?? '';
        // total > page_size so "下一页" is enabled.
        return HttpResponse.json(pageOf([ALERT_ROW], { total: 45 }));
      }),
    );
    renderPage(<DenialsPage />);
    await waitForRanking();

    // Default page_size is 20 (DB_PAGINATION_MANDATORY).
    expect(seenSizes[0]).toBe('20');

    // Next page → new page_no=2 carried to the server (server-driven paging).
    fireEvent.click(screen.getByRole('button', { name: '下一页' }));
    await waitFor(() => expect(lastPageNo).toBe('2'));
  });

  it('改页大小为 200 时请求携带钳到 200 的 page_size', async () => {
    const seenSizes: string[] = [];
    server.use(
      http.get(URL, ({ request }) => {
        const sp = new globalThis.URL(request.url).searchParams;
        seenSizes.push(sp.get('page_size') ?? '');
        return HttpResponse.json(pageOf([ALERT_ROW], { total: 300 }));
      }),
    );
    renderPage(<DenialsPage />);
    await waitForRanking();

    fireEvent.change(screen.getByLabelText('每页组数'), { target: { value: '200' } });
    await waitFor(() => expect(seenSizes).toContain('200'));
  });
});

// ── 客户端排序（仅当前页内）──────────────────────────────────────────────────

describe('DenialsPage · 列排序（页内）', () => {
  it('点击“次数”列在当前页内切换升/降序', async () => {
    mockDenials(pageOf([ALERT_ROW, MID_ROW, LOW_ROW], { total: 3 }));
    renderPage(<DenialsPage />);
    await screen.findByText('svc-order');

    // Default desc → click count header toggles to ascending (9,14,37).
    fireEvent.click(screen.getByRole('button', { name: /次数/ }));
    const counts = screen
      .getAllByRole('row')
      .map((r) => within(r).queryByText(/^(37|14|9)$/)?.textContent)
      .filter(Boolean);
    expect(counts).toEqual(['9', '14', '37']);
  });
});
