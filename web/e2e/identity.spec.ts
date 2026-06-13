import { test, expect, type Page } from '@playwright/test';

/**
 * 主体与凭证 Principals / Credentials — 真实浏览器端到端（10-principals-credentials.md）。
 *
 * 运行于共享的 Vite + 浏览器内 MSW dev server（:5173）。MSW 是**静态 handlers**，
 * 无法 per-test 改返回，故本 spec 只覆盖默认 fixtures 下可可靠观测的真实集成：
 * happy path、跨区联动、以及 DOM 级安全契约不变量。异常/错误态/分页边界由 vitest
 * 单测（server.use）充分覆盖，此处不重复造。
 *
 * 默认 fixtures（src/mocks/fixtures.ts）实际渲染的数据：
 *  - 主体：agent-order-bot（agent, id 7300000000000000123）、alice（human, id …456）。
 *  - 凭证：agent-order-bot 有 1 条 api_key（trust_domain=mcp-local, id …789, 未吊销）。
 *  - POST /v1/credentials 静态返回仅 { policy_rev }（**不含 api_key 明文**），故一次性
 *    展示框（ApiKeyRevealDialog）无法经真实创建触发——该行为由 vitest 覆盖；本 spec
 *    在浏览器里断言其**反向契约**：创建后明文/secret_hash 永不在列表出现。
 */

const AGENT = 'agent-order-bot';
const AGENT_ID = '7300000000000000123';
const AGENT_ID_ROUNDED = '7300000000000000000'; // Number(AGENT_ID) 的丢精度形，绝不应出现
const CRED_ID = '7300000000000000789';

/** 进入主体凭证页并等左栏从 fixtures 渲染出真实主体（非骨架/空态）。 */
async function gotoIdentity(page: Page) {
  await page.goto('/');
  await page.getByRole('link', { name: /主体与凭证 Principals/ }).click();
  await expect(
    page.getByRole('heading', { name: /主体与凭证 Principals \/ Credentials/ }),
  ).toBeVisible();
  // 真实数据落地：fixtures 里的主体名出现。
  await expect(page.getByText(AGENT)).toBeVisible();
}

/** 选中 agent-order-bot 行，右栏聚焦其凭证。 */
async function selectAgent(page: Page) {
  const agentRow = page.locator('tr', { hasText: AGENT });
  await agentRow.getByRole('button', { name: '查看凭证' }).click();
  await expect(
    page.getByRole('heading', { name: new RegExp(`凭证 Credentials · ${AGENT}`) }),
  ).toBeVisible();
}

test.describe('主体与凭证 — 真实浏览器集成', () => {
  test('左栏主体名册从 fixtures 渲染真实数据 + 雪花 id 全值不丢精度', async ({ page }) => {
    await gotoIdentity(page);

    // 两个 fixtures 主体都渲染出真实值（不是骨架/空态）。
    await expect(page.getByText(AGENT)).toBeVisible();
    await expect(page.getByText('alice')).toBeVisible();

    // kind 徽章渲染（限定在表内，避开 kind 筛选 <option>）。
    const table = page.getByRole('table');
    await expect(table.getByText('agent', { exact: true })).toBeVisible();
    await expect(table.getByText('human', { exact: true })).toBeVisible();

    // 雪花 id 契约：全值原样 string 在 title 上，绝不当 number 解析（丢精度）。
    await expect(page.getByTitle(AGENT_ID)).toBeVisible();
    await expect(page.getByTitle(AGENT_ID_ROUNDED)).toHaveCount(0);

    // 生效凭证数：agent-order-bot 行 = 1，alice 行 = 0。
    const agentRow = page.locator('tr', { hasText: AGENT });
    await expect(agentRow.getByText('1', { exact: true })).toBeVisible();
    const aliceRow = page.locator('tr', { hasText: 'alice' });
    await expect(aliceRow.getByText('0', { exact: true })).toBeVisible();
  });

  test('kind 筛选与按名搜索改变渲染结果', async ({ page }) => {
    await gotoIdentity(page);

    // 按 kind=human 筛 → agent-order-bot 消失，alice 仍在。
    await page.getByLabel('按 kind 筛选').selectOption('human');
    await expect(page.getByText(AGENT)).toHaveCount(0);
    await expect(page.getByText('alice')).toBeVisible();

    // 还原后按名搜 "order" → 只剩 agent-order-bot。
    await page.getByLabel('按 kind 筛选').selectOption('');
    await page.getByLabel('按名搜索').fill('order');
    await expect(page.getByText('alice')).toHaveCount(0);
    await expect(page.getByText(AGENT)).toBeVisible();
  });

  test('master–detail 联动：未选引导态 → 选中后渲染该主体凭证（生效状态）', async ({
    page,
  }) => {
    await gotoIdentity(page);

    // 未选主体：右栏中性引导态（非错误）。
    await expect(page.getByText(/选择左侧一个主体查看其网关凭证/)).toBeVisible();
    await expect(page.getByText(/凭证加载失败/)).toHaveCount(0);

    await selectAgent(page);

    // 右栏渲染该主体的 api_key 凭证 + 派生"生效"状态徽章 + 真实可信域。
    // exact:true 命中状态徽章本身（避开摘要行 "生效 1"）。
    await expect(page.getByText('api_key')).toBeVisible();
    await expect(page.getByText('生效', { exact: true })).toBeVisible();
    await expect(page.getByText('域: mcp-local')).toBeVisible();

    // alice（无凭证）→ 右栏空态（如实陈述后果，非错误）。
    const aliceRow = page.locator('tr', { hasText: 'alice' });
    await aliceRow.getByRole('button', { name: '查看凭证' }).click();
    await expect(page.getByText(/该主体暂无网关凭证/)).toBeVisible();
  });

  test('新建主体：抽屉 → 摘要预览 → 提交成功 toast 带 policy_rev', async ({ page }) => {
    await gotoIdentity(page);

    await page.getByRole('button', { name: /新建主体/ }).click();
    const form = page.getByRole('form', { name: '新建主体表单' });
    await expect(form).toBeVisible();

    await form.getByLabel('主体名').fill('svc-cron');
    // 摘要预览反映键入名 + 公理一事实陈述。
    await expect(form.getByText(/将登记主体 svc-cron/)).toBeVisible();
    await expect(form.getByText(/默认拒绝一切/)).toBeVisible();

    await form.getByRole('button', { name: '登记主体' }).click();
    // 成功：drawer 关 + 成功 banner（status 角色）含 policy_rev 前进。
    await expect(page.getByRole('form', { name: '新建主体表单' })).toHaveCount(0);
    const banner = page.getByRole('status');
    await expect(banner).toContainText('主体已登记');
    await expect(banner).toContainText(/policy_rev → \d+/);
  });

  test('新建凭证（api_key）：成功 toast 落 credential_event；明文与 secret_hash 永不入列表', async ({
    page,
  }) => {
    await gotoIdentity(page);
    await selectAgent(page);

    await page.getByRole('button', { name: /新建凭证/ }).click();
    const form = page.getByRole('form', { name: '新建凭证表单' });
    await expect(form).toBeVisible();

    // 默认 kind=api_key：表单明示不录入明文（基座原则六）。
    await expect(form.getByText(/api_key 由 daemon 生成，表单不录入明文/)).toBeVisible();
    await form.getByLabel('可信域').fill('mcp-local');
    // 摘要预览（事实陈述）。
    await expect(form.getByText(/为主体 agent-order-bot 新建 api_key 凭证/)).toBeVisible();

    await form.getByRole('button', { name: '创建凭证' }).click();

    // 成功：drawer 关 + 成功 banner 指向 credential_event（不记 secret）。
    await expect(page.getByRole('form', { name: '新建凭证表单' })).toHaveCount(0);
    const banner = page.getByRole('status');
    await expect(banner).toContainText('凭证已创建');
    await expect(banner).toContainText('credential_event');

    // DOM 契约：创建后回到列表，secret_hash / 明文密钥从不在页面任何处出现。
    const body = page.locator('body');
    await expect(body).not.toContainText('secret_hash');
    await expect(body).not.toContainText(/pk_live_/);
    await expect(body).not.toContainText(/sk_live_/);
  });

  test('token 凭证录入：明文字段为 password 类型（不明文回显）', async ({ page }) => {
    await gotoIdentity(page);
    await selectAgent(page);

    await page.getByRole('button', { name: /新建凭证/ }).click();
    const form = page.getByRole('form', { name: '新建凭证表单' });
    await expect(form).toBeVisible();

    // 切到 kind=token → 出现"令牌值"字段，且为 password 类型。
    await form.getByRole('radio', { name: /token/ }).check();
    const secret = form.getByLabel('令牌值');
    await expect(secret).toBeVisible();
    await expect(secret).toHaveAttribute('type', 'password');
    // 明文零接触纪律：表单声明本地不留存。
    await expect(form.getByText(/本地不留存、提交后即清/)).toBeVisible();
  });

  test('吊销凭证（最高危·热生效·不可逆）：危险确认须显式键入确认词', async ({ page }) => {
    await gotoIdentity(page);
    await selectAgent(page);

    // 凭证行操作 ⋮ → 吊销凭证。
    await page.getByRole('button', { name: '凭证操作' }).click();
    await page.getByRole('menuitem', { name: '吊销凭证' }).click();

    const dialog = page.getByRole('dialog', { name: /吊销凭证（热生效·不可逆）/ });
    await expect(dialog).toBeVisible();
    // 后果直述：热生效 / 不可撤销 / ≠删除。
    await expect(dialog.getByText(/热生效：吊销后该凭证一切认证即时被拒/)).toBeVisible();
    await expect(dialog.getByText(/不可撤销/)).toBeVisible();
    await expect(dialog.getByText(/不删除凭证记录（区别于删除）/)).toBeVisible();

    // 确认按钮门控于键入精确确认词"吊销"。
    const confirm = dialog.getByRole('button', { name: '吊销' });
    await expect(confirm).toBeDisabled();
    await dialog.getByRole('textbox').fill('吊销');
    await expect(confirm).toBeEnabled();

    await confirm.click();
    // 成功：banner 明示热生效 + policy_rev 前进 + 落 credential_event。
    const banner = page.getByRole('status');
    await expect(banner).toContainText('凭证已吊销');
    await expect(banner).toContainText('热生效');
    await expect(banner).toContainText('credential_event');
  });

  test('删除主体（≠吊销）：有生效凭证时确认框提示先吊销', async ({ page }) => {
    await gotoIdentity(page);

    const agentRow = page.locator('tr', { hasText: AGENT });
    // 等生效凭证数加载（=1）后再删，使 hasActiveCreds 为真。
    await expect(agentRow.getByText('1', { exact: true })).toBeVisible();
    await agentRow.getByRole('button', { name: `删除主体 ${AGENT}` }).click();

    const dialog = page.getByRole('dialog', { name: /删除主体（逻辑删除）/ });
    await expect(dialog).toBeVisible();
    // 明示删除≠吊销 + 有生效凭证须先吊销（避免"已删主体仍可认证"悖态）。
    await expect(dialog.getByText(/不等于吊销其凭证/)).toBeVisible();
    await expect(dialog.getByText(/请先吊销再删除/)).toBeVisible();
  });

  test('DOM 安全契约：凭证区无 secret_hash/明文、无真实地址连接串、id 全值在 title', async ({
    page,
  }) => {
    await gotoIdentity(page);
    await selectAgent(page);

    // 凭证卡片渲染（含真实 id），但绝不出现 secret_hash 或明文密钥前缀。
    await expect(page.getByText('api_key')).toBeVisible();
    const panel = page.getByRole('region', { name: '凭证' }).first();
    await expect(panel).not.toContainText('secret_hash');
    await expect(panel).not.toContainText(/pk_live_|sk_live_|password/);

    // 凭证雪花 id 全值在 title（string，不丢精度），截断形不暴露 number 化。
    await expect(page.getByTitle(CRED_ID)).toBeVisible();

    // 全页绝无真实地址/连接串（postgres:// 或裸 IP 形）——本页是网关身份面，不碰资源地址。
    const body = page.locator('body');
    await expect(body).not.toContainText(/postgres:\/\//);
    await expect(body).not.toContainText(/\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/);
  });
});
