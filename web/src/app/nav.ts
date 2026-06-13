/**
 * Navigation model (设计系统 §5): four groups, twelve pages. Paths are the
 * single source of truth shared by the router and the sidebar.
 */
export interface NavItem {
  path: string;
  label: string;
  /** API the page primarily reads (for the placeholder + future wiring). */
  api: string;
}

export interface NavGroup {
  group: string;
  items: NavItem[];
}

export const DASHBOARD: NavItem = {
  path: '/',
  label: '总览 Dashboard',
  api: 'health, denials/summary, grants, mode',
};

export const NAV: NavGroup[] = [
  {
    group: '观测 Observability',
    items: [
      { path: '/audit', label: '审计 Audit', api: 'GET /v1/audit' },
      { path: '/denials', label: '拒绝分析 Denials', api: 'GET /v1/denials/summary' },
      { path: '/verify', label: '红队自检 Verify', api: 'POST /v1/verify' },
    ],
  },
  {
    group: '授权 Authorization',
    items: [
      { path: '/grants', label: '授权矩阵 Grants', api: 'GET /v1/grants' },
      { path: '/roles', label: '角色 Roles', api: 'GET/POST /v1/roles' },
      { path: '/bindings', label: '绑定 Bindings', api: 'GET/POST /v1/bindings' },
      { path: '/constraints', label: '细则与条件 Constraints', api: 'constraints/conditions/deny-notes' },
    ],
  },
  {
    group: '接入 Resources & Identity',
    items: [
      { path: '/resources', label: '资源 Resources', api: 'GET/POST /v1/resources' },
      { path: '/principals', label: '主体与凭证 Principals', api: 'principals, credentials' },
    ],
  },
  {
    group: '系统 System',
    items: [
      { path: '/mode', label: '模式 Mode', api: 'POST /v1/mode' },
      { path: '/system', label: '系统 System', api: 'approvals, settings, import/export' },
    ],
  },
];

/** Flat list of every navigable item (dashboard + 11 inner pages = 12). */
export const ALL_ITEMS: NavItem[] = [DASHBOARD, ...NAV.flatMap((g) => g.items)];
