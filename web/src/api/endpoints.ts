/**
 * The 28 control-plane endpoints (CONTROL_ROUTES) as typed functions.
 *
 * One function per `(method, path)` in postern-daemon control/router.rs. No
 * route is invented (notably no `GET /v1/mode`, no `GET /v1/approvals`); mode
 * and approvals are read via their POST same-source projection. Pagination is
 * forced on every collection GET.
 */

import { buildQuery, http } from './client';
import type {
  ApprovalItem,
  AuditEvent,
  AuditQuery,
  Binding,
  CapabilitySurface,
  ConditionRow,
  ConstraintRow,
  CredentialRow,
  DenialSummaryRow,
  DenialWindow,
  DenyNoteRow,
  ElevateRequest,
  GrantsView,
  Health,
  ModeSetRequest,
  ModeStateRow,
  Page,
  PageQuery,
  PrincipalRow,
  ResourceRow,
  RevokeRequest,
  Role,
  SettingRow,
  VerifyReport,
  WriteAck,
} from './types';

// ── 主体 / 凭据 / 角色 / 绑定 ──────────────────────────────────────────────────

export const getPrincipals = (page: Partial<PageQuery> = {}) =>
  http.get<Page<PrincipalRow>>(`/principals?${buildQuery(page)}`);
export const postPrincipal = (body: unknown) =>
  http.post<WriteAck>('/principals', body);

export const getCredentials = (
  page: Partial<PageQuery> = {},
  principal?: string,
) => http.get<Page<CredentialRow>>(`/credentials?${buildQuery(page, { principal })}`);
export const postCredential = (body: unknown) =>
  http.post<WriteAck>('/credentials', body);

// Normalize the deferred read-model projections (effective/direct/inherits_from)
// to arrays so the page renders even before the backend computes them. Forward-
// compatible: real projected values, when the backend lands them, override the
// `??` defaults. (Backend projection is a tracked follow-up.)
export const getRoles = (page: Partial<PageQuery> = {}) =>
  http.get<Page<Role>>(`/roles?${buildQuery(page)}`).then((res) => ({
    ...res,
    items: res.items.map((r) => ({
      ...r,
      effective: r.effective ?? [],
      direct: r.direct ?? [],
      inherits_from: r.inherits_from ?? [],
    })),
  }));
export const postRole = (body: unknown) => http.post<WriteAck>('/roles', body);

export const getBindings = (page: Partial<PageQuery> = {}) =>
  http.get<Page<Binding>>(`/bindings?${buildQuery(page)}`);
export const postBinding = (body: unknown) =>
  http.post<WriteAck>('/bindings', body);

// ── 资源（含 discover 子动作）─────────────────────────────────────────────────

export const getResources = (page: Partial<PageQuery> = {}) =>
  http.get<Page<ResourceRow>>(`/resources?${buildQuery(page)}`).then((res) => ({
    ...res,
    // tiers/labels are deferred backend projections — default to arrays so the
    // page renders; real values override when the backend projects them.
    items: res.items.map((r) => ({ ...r, tiers: r.tiers ?? [], labels: r.labels ?? [] })),
  }));
export const postResource = (body: unknown) =>
  http.post<WriteAck>('/resources', body);
export const discoverResource = (code: string) =>
  http.post<CapabilitySurface>(`/resources/${encodeURIComponent(code)}/discover`);

// ── 细则 / 条件 / 拒绝备注 ────────────────────────────────────────────────────

export const getConstraints = (page: Partial<PageQuery> = {}) =>
  http.get<Page<ConstraintRow>>(`/constraints?${buildQuery(page)}`);
export const postConstraint = (body: unknown) =>
  http.post<WriteAck>('/constraints', body);

export const getConditions = (page: Partial<PageQuery> = {}) =>
  http.get<Page<ConditionRow>>(`/conditions?${buildQuery(page)}`);
export const postCondition = (body: unknown) =>
  http.post<WriteAck>('/conditions', body);

export const getDenyNotes = (page: Partial<PageQuery> = {}) =>
  http.get<Page<DenyNoteRow>>(`/deny-notes?${buildQuery(page)}`);
export const postDenyNote = (body: unknown) =>
  http.post<WriteAck>('/deny-notes', body);

// ── 设置 ──────────────────────────────────────────────────────────────────────

export const getSettings = () => http.get<SettingRow[]>('/settings');
export const postSettings = (body: unknown) =>
  http.post<WriteAck>('/settings', body);

// ── 临时授权 / 模式 / 授权视图 ────────────────────────────────────────────────

export const elevateGrant = (body: ElevateRequest) =>
  http.post<WriteAck>('/grants/temp/elevate', body);
export const revokeGrant = (body: RevokeRequest) =>
  http.post<WriteAck>('/grants/temp/revoke', body);

/**
 * POST /v1/mode is the only mode write AND its same-source read: posting with
 * no change (or the SPA's mode-state projection request) returns the current
 * `mode_state` rows. There is deliberately no GET /v1/mode.
 */
export const getModeState = () =>
  http.post<ModeStateRow[]>('/mode', { op: 'read' });
export const setMode = (body: ModeSetRequest) =>
  http.post<{ rows: ModeStateRow[] } & WriteAck>('/mode', { op: 'set', ...body });

export const getGrants = (principal?: string, page: Partial<PageQuery> = {}) =>
  http.get<GrantsView>(`/grants?${buildQuery(page, { principal })}`);

// ── 审计 / 拒绝摘要 / 审批 ────────────────────────────────────────────────────

export const getAudit = (query: AuditQuery = {}) => {
  const { page_no, page_size, ...filters } = query;
  return http.get<Page<AuditEvent>>(
    `/audit?${buildQuery({ page_no, page_size }, filters as Record<string, string | undefined>)}`,
  );
};

export const getDenialsSummary = (
  window: DenialWindow = '7d',
  page: Partial<PageQuery> = {},
) => http.get<Page<DenialSummaryRow>>(`/denials/summary?${buildQuery(page, { window })}`);

/** POST /v1/approvals — query (and adjudicate). Default disabled ⇒ usually empty. */
export const getApprovals = (page: Partial<PageQuery> = {}) =>
  http.post<Page<ApprovalItem>>('/approvals', { op: 'list', ...buildPageBody(page) });
export const adjudicateApproval = (body: unknown) =>
  http.post<WriteAck>('/approvals', { op: 'adjudicate', ...(body as object) });

function buildPageBody(page: Partial<PageQuery>): PageQuery {
  return {
    page_no: Math.max(1, page.page_no ?? 1),
    page_size: Math.min(200, Math.max(1, page.page_size ?? 20)),
  };
}

// ── 导出 / 导入 / 校验 ────────────────────────────────────────────────────────

export const exportPolicy = () => http.post<{ toml: string }>('/export', {});
export const importPolicy = (body: unknown) =>
  http.post<{ added: number; changed: number; deleted: number; applied: boolean }>(
    '/import',
    body,
  );

export const runVerify = () => http.post<VerifyReport>('/verify');

// ── 健康 / 关停 ──────────────────────────────────────────────────────────────

export const getHealth = () => http.get<Health>('/health');
export const shutdown = () => http.post<WriteAck>('/shutdown', { confirm: 'shutdown' });
