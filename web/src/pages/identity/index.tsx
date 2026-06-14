import { useMemo, useState, type ReactNode } from 'react';
import { Plus, Search } from 'lucide-react';
import {
  Badge,
  DataTable,
  FormDrawer,
  ConfirmDialog,
  SnowflakeId,
  type Column,
} from '../../components';
import { ConflictError } from '../../api/client';
import {
  PAGE_DEFAULT_SIZE,
  PAGE_MAX_SIZE,
  type CredentialRow,
  type PageQuery,
  type PrincipalKind,
  type PrincipalRow,
} from '../../api/types';
import { useCredentials, usePrincipals } from '../../api/hooks';
import { CredentialPanel } from './CredentialPanel';
import { PrincipalForm } from './PrincipalForm';
import { CredentialForm } from './CredentialForm';
import { ApiKeyRevealDialog } from './ApiKeyRevealDialog';
import {
  PRINCIPAL_KINDS,
  deleteCredentialSummary,
  deletePrincipalSummary,
  revokeSummary,
  tallyCredentials,
  type CredentialFormValues,
  type PrincipalFormValues,
} from './schema';
import {
  useCreateCredential,
  useCreatePrincipal,
  useDeleteCredential,
  useDeletePrincipal,
  useRevokeCredential,
} from './mutations';

/**
 * 主体与凭证 Principals / Credentials（10-principals-credentials.md）。
 *
 * 左右双栏 master–detail：左 DataTable 主体名册（kind 筛选 / name 搜 / 强制分页 /
 * 三态 / 行操作），右 CredentialPanel 聚焦选中主体的网关凭证。全部写操作走基座
 * 统一流程（RHF+Zod → 摘要预览 →（危险则）ConfirmDialog → 失效刷新 → 成功/失败/
 * 409）。凭证永不显 secret_hash/明文；api_key 明文仅创建时一次性显示。
 *
 * 凭证数据：单次拉取（不带 principal 过滤）供左栏"凭证数"聚合与右栏过滤共用，
 * 避免双查询；凭证加载失败时左栏凭证数显"—"（不确定不显 0），右栏错误态不显
 * 任何"生效"凭证（§6.2 fail-closed）。
 */

const CONFIRM_WORD = '吊销';

const KIND_LABEL: Record<PrincipalKind, string> = {
  agent: 'agent',
  program: 'program',
  human: 'human',
};

type Banner = { tone: 'success' | 'error'; text: string } | null;

type DangerAction =
  | { type: 'revoke'; cred: CredentialRow; principalName: string }
  | { type: 'deleteCredential'; cred: CredentialRow; principalName: string }
  | { type: 'deletePrincipal'; principal: PrincipalRow; hasActiveCreds: boolean }
  | null;

export function IdentityPage() {
  const [page, setPage] = useState<PageQuery>({
    page_no: 1,
    page_size: PAGE_DEFAULT_SIZE,
  });
  const [kindFilter, setKindFilter] = useState<PrincipalKind | ''>('');
  const [nameQuery, setNameQuery] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [banner, setBanner] = useState<Banner>(null);

  // Drawers / dialogs.
  const [principalDrawer, setPrincipalDrawer] = useState(false);
  const [credentialDrawer, setCredentialDrawer] = useState(false);
  const [danger, setDanger] = useState<DangerAction>(null);
  const [revealKey, setRevealKey] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);

  const principalsQuery = usePrincipals(page);
  // 凭证：单次拉取（满页上限）供左栏聚合 + 右栏过滤共用。
  const credentialsQuery = useCredentials({ page_no: 1, page_size: PAGE_MAX_SIZE });

  const createPrincipal = useCreatePrincipal();
  const deletePrincipal = useDeletePrincipal();
  const createCredential = useCreateCredential();
  const revokeCredential = useRevokeCredential();
  const deleteCredential = useDeleteCredential();

  const allPrincipals = useMemo(
    () => principalsQuery.data?.items ?? [],
    [principalsQuery.data],
  );
  const total = principalsQuery.data?.total ?? 0;

  // 客户端筛选（kind 单选 / name 模糊）仅作用于当前页（服务器拥有分页）。
  const visiblePrincipals = useMemo(
    () =>
      allPrincipals.filter((p) => {
        if (kindFilter && p.kind !== kindFilter) return false;
        if (nameQuery.trim() && !p.name.toLowerCase().includes(nameQuery.trim().toLowerCase()))
          return false;
        return true;
      }),
    [allPrincipals, kindFilter, nameQuery],
  );

  const credsLoadFailed = credentialsQuery.isError;
  const allCreds = useMemo(
    () => credentialsQuery.data?.items ?? [],
    [credentialsQuery.data],
  );

  const credsByPrincipal = useMemo(() => {
    const map = new Map<string, CredentialRow[]>();
    for (const c of allCreds) {
      const list = map.get(c.principal_id) ?? [];
      list.push(c);
      map.set(c.principal_id, list);
    }
    return map;
  }, [allCreds]);

  const selectedPrincipal =
    allPrincipals.find((p) => p.id === selectedId) ?? null;
  const selectedCreds = selectedPrincipal
    ? credsByPrincipal.get(selectedPrincipal.id) ?? []
    : [];

  // ── 左栏列定义 ──────────────────────────────────────────────────────────────
  const columns: Column<PrincipalRow>[] = [
    {
      key: 'name',
      header: 'name',
      render: (p) => <span className="font-medium text-text">{p.name}</span>,
      sortValue: (p) => p.name,
    },
    {
      key: 'kind',
      header: 'kind',
      render: (p) => <Badge className="border-border text-text-muted">{KIND_LABEL[p.kind]}</Badge>,
      sortValue: (p) => p.kind,
    },
    {
      key: 'creds',
      header: '凭证#',
      render: (p) => {
        // 凭证加载失败：显"—"而非 0（§6.2，不确定不冒充）。
        if (credsLoadFailed) {
          return <span title="凭证加载失败，无法统计" className="text-text-muted">—</span>;
        }
        const list = credsByPrincipal.get(p.id) ?? [];
        const t = tallyCredentials(list);
        return (
          <span
            title={`生效 ${t.active} · 吊销 ${t.revoked} · 过期 ${t.expired}`}
            className="font-mono"
          >
            {t.active}
          </span>
        );
      },
    },
    {
      key: 'id',
      header: 'id',
      render: (p) => <SnowflakeId id={p.id} />,
      className: 'font-mono',
    },
  ];

  // ── 写流程回调 ──────────────────────────────────────────────────────────────
  function describeWriteError(err: unknown): string {
    if (err instanceof ConflictError) {
      return '他人已修改该记录，请刷新后重试（不会静默覆盖）。';
    }
    if (err instanceof Error) return err.message;
    return '操作失败';
  }

  function submitPrincipal(values: PrincipalFormValues) {
    setFormError(null);
    createPrincipal.mutate(
      { op: 'create', name: values.name.trim(), kind: values.kind },
      {
        onSuccess: () => {
          setPrincipalDrawer(false);
          setBanner({ tone: 'success', text: '主体已登记。' });
        },
        onError: (err) => setFormError(describeWriteError(err)),
      },
    );
  }

  function submitCredential(values: CredentialFormValues) {
    if (!selectedPrincipal) return;
    setFormError(null);
    const expires_at =
      values.ttl_days.trim() === ''
        ? null
        : new Date(Date.now() + Number(values.ttl_days.trim()) * 86_400_000).toISOString();
    const body = {
      op: 'create' as const,
      principal_id: selectedPrincipal.id,
      kind: values.kind,
      trust_domain: values.trust_domain.trim(),
      expires_at,
      ...(values.kind === 'token' ? { secret: values.secret } : {}),
    };
    createCredential.mutate(body, {
      onSuccess: (ack) => {
        setCredentialDrawer(false);
        // api_key 特例：一次性展示明文。
        if (values.kind === 'api_key' && ack.api_key) {
          setRevealKey(ack.api_key);
        }
        setBanner({ tone: 'success', text: '凭证已创建。' });
      },
      onError: (err) => setFormError(describeWriteError(err)),
    });
  }

  function confirmDanger() {
    if (!danger) return;
    if (danger.type === 'revoke') {
      revokeCredential.mutate(
        { op: 'revoke', id: danger.cred.id, version: danger.cred.version },
        {
          onSuccess: () => {
            setDanger(null);
            setBanner({ tone: 'success', text: '凭证已吊销，即时生效。' });
          },
          onError: (err) => {
            setDanger(null);
            setBanner({ tone: 'error', text: describeWriteError(err) });
          },
        },
      );
    } else if (danger.type === 'deleteCredential') {
      deleteCredential.mutate(
        { op: 'delete', id: danger.cred.id, version: danger.cred.version },
        {
          onSuccess: () => {
            setDanger(null);
            setBanner({ tone: 'success', text: '凭证已删除。' });
          },
          onError: (err) => {
            setDanger(null);
            setBanner({ tone: 'error', text: describeWriteError(err) });
          },
        },
      );
    } else {
      deletePrincipal.mutate(
        { op: 'delete', id: danger.principal.id, version: danger.principal.version },
        {
          onSuccess: () => {
            setDanger(null);
            if (selectedId === danger.principal.id) setSelectedId(null);
            setBanner({ tone: 'success', text: '主体已删除。' });
          },
          onError: (err) => {
            setDanger(null);
            setBanner({ tone: 'error', text: describeWriteError(err) });
          },
        },
      );
    }
  }

  const dangerCopy = useMemo(() => buildDangerCopy(danger), [danger]);

  return (
    <section className="flex flex-col gap-4">
      <header className="flex items-center justify-between">
        <h1 className="text-2xl font-medium">主体与凭证 Principals / Credentials</h1>
        <button
          type="button"
          onClick={() => {
            setFormError(null);
            setPrincipalDrawer(true);
          }}
          className="inline-flex items-center gap-1 rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
        >
          <Plus size={14} /> 新建主体
        </button>
      </header>

      {banner && (
        <div
          role={banner.tone === 'error' ? 'alert' : 'status'}
          className={
            banner.tone === 'error'
              ? 'rounded-card border border-deny/40 bg-deny/5 px-3 py-2 text-sm text-deny'
              : 'rounded-card border border-allow/40 bg-allow/5 px-3 py-2 text-sm text-allow'
          }
        >
          {banner.text}
        </div>
      )}

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        {/* 左栏：主体名册 */}
        <section aria-label="主体名册" className="flex flex-col gap-3">
          <h2 className="text-lg font-medium">主体名册 Principals</h2>
          <div className="flex flex-wrap items-center gap-2">
            <label className="flex items-center gap-1 text-sm">
              <span className="text-text-muted">kind</span>
              <select
                aria-label="按 kind 筛选"
                value={kindFilter}
                onChange={(e) => setKindFilter(e.target.value as PrincipalKind | '')}
                className="rounded-card border border-border bg-surface px-2 py-1 text-sm"
              >
                <option value="">全部</option>
                {PRINCIPAL_KINDS.map((k) => (
                  <option key={k} value={k}>
                    {k}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex items-center gap-1 text-sm">
              <Search size={14} className="text-text-muted" />
              <input
                aria-label="按名搜索"
                value={nameQuery}
                onChange={(e) => setNameQuery(e.target.value)}
                placeholder="搜名"
                className="rounded-card border border-border bg-bg px-2 py-1 text-sm"
              />
            </label>
          </div>

          <DataTable
            columns={columns}
            rows={visiblePrincipals}
            total={total}
            page={page}
            onPageChange={setPage}
            rowKey={(p) => p.id}
            loading={principalsQuery.isLoading}
            error={principalsQuery.isError ? { message: principalsQuery.error?.message } : null}
            onRetry={() => void principalsQuery.refetch()}
            emptyTitle="暂无主体"
            emptyAction={
              <button
                type="button"
                onClick={() => setPrincipalDrawer(true)}
                className="rounded-card bg-info px-3 py-1.5 text-sm text-white hover:brightness-110"
              >
                新建主体
              </button>
            }
            rowActions={(p) => (
              <div className="flex items-center justify-end gap-2">
                <button
                  type="button"
                  onClick={() => setSelectedId(p.id)}
                  aria-pressed={selectedId === p.id}
                  className={
                    selectedId === p.id
                      ? 'rounded-card border border-info px-2 py-1 text-xs text-info'
                      : 'rounded-card border border-border px-2 py-1 text-xs hover:bg-surface-2'
                  }
                >
                  查看凭证
                </button>
                <button
                  type="button"
                  aria-label={`删除主体 ${p.name}`}
                  onClick={() => {
                    const list = credsByPrincipal.get(p.id) ?? [];
                    const hasActiveCreds =
                      !credsLoadFailed && tallyCredentials(list).active > 0;
                    setDanger({ type: 'deletePrincipal', principal: p, hasActiveCreds });
                  }}
                  className="rounded-card border border-border px-2 py-1 text-xs text-deny hover:bg-surface-2"
                >
                  删除
                </button>
              </div>
            )}
          />
        </section>

        {/* 右栏：凭证 */}
        <CredentialPanel
          principal={selectedPrincipal}
          creds={selectedCreds}
          loading={Boolean(selectedPrincipal) && credentialsQuery.isLoading}
          error={
            selectedPrincipal && credsLoadFailed
              ? { message: credentialsQuery.error?.message }
              : null
          }
          onRetry={() => void credentialsQuery.refetch()}
          onCreate={() => {
            setFormError(null);
            setCredentialDrawer(true);
          }}
          onRevoke={(cred) =>
            setDanger({
              type: 'revoke',
              cred,
              principalName: selectedPrincipal?.name ?? cred.principal,
            })
          }
          onDelete={(cred) =>
            setDanger({
              type: 'deleteCredential',
              cred,
              principalName: selectedPrincipal?.name ?? cred.principal,
            })
          }
        />
      </div>

      {/* 新建主体抽屉 */}
      <FormDrawer
        open={principalDrawer}
        title="新建主体"
        onClose={() => setPrincipalDrawer(false)}
      >
        <PrincipalForm
          submitting={createPrincipal.isPending}
          submitError={formError}
          onSubmit={submitPrincipal}
          onCancel={() => setPrincipalDrawer(false)}
        />
      </FormDrawer>

      {/* 新建凭证抽屉 */}
      <FormDrawer
        open={credentialDrawer && Boolean(selectedPrincipal)}
        title="新建凭证"
        onClose={() => setCredentialDrawer(false)}
      >
        {selectedPrincipal && (
          <CredentialForm
            principal={selectedPrincipal}
            submitting={createCredential.isPending}
            submitError={formError}
            onSubmit={submitCredential}
            onCancel={() => setCredentialDrawer(false)}
          />
        )}
      </FormDrawer>

      {/* 危险确认：吊销 / 删除凭证 / 删除主体 */}
      <ConfirmDialog
        open={danger !== null}
        title={dangerCopy.title}
        body={dangerCopy.body}
        confirmWord={dangerCopy.confirmWord}
        confirmLabel={dangerCopy.confirmLabel}
        onConfirm={confirmDanger}
        onCancel={() => setDanger(null)}
      />

      {/* api_key 一次性明文展示 */}
      {revealKey !== null && (
        <ApiKeyRevealDialog apiKey={revealKey} onClose={() => setRevealKey(null)} />
      )}
    </section>
  );
}

function buildDangerCopy(danger: DangerAction): {
  title: string;
  body: ReactNode;
  confirmWord?: string;
  confirmLabel: string;
} {
  if (!danger) return { title: '', body: null, confirmLabel: '确认' };
  if (danger.type === 'revoke') {
    return {
      title: '吊销凭证（热生效·不可逆）',
      confirmWord: CONFIRM_WORD,
      confirmLabel: '吊销',
      body: (
        <div className="flex flex-col gap-2">
          <div>{revokeSummary(danger.principalName, danger.cred)}</div>
          <div className="text-deny">
            即时生效，不可撤销。吊销不删除凭证记录，仅将其置为吊销态。
          </div>
        </div>
      ),
    };
  }
  if (danger.type === 'deleteCredential') {
    return {
      title: '删除凭证（逻辑删除·≠吊销）',
      confirmLabel: '删除',
      body: <div>{deleteCredentialSummary(danger.principalName, danger.cred)}</div>,
    };
  }
  return {
    title: '删除主体（逻辑删除）',
    confirmLabel: '删除',
    body: <div>{deletePrincipalSummary(danger.principal.name, danger.hasActiveCreds)}</div>,
  };
}

export default IdentityPage;
