/**
 * Mock fixtures in REAL wire shape: snowflake-string ids, paged envelopes,
 * structured deny, audit events, the nine verify probes, mode state, health.
 * These let the SPA (and tests) run with no daemon. Ids are deliberately
 * >2^53 to catch any accidental Number coercion.
 */

import type {
  ApprovalItem,
  AuditEvent,
  Binding,
  ConditionRow,
  ConstraintRow,
  CredentialRow,
  DenialSummaryRow,
  DenyNoteRow,
  DenyResponse,
  GrantsView,
  Health,
  ModeStateRow,
  PrincipalRow,
  ResourceRow,
  Role,
  SettingRow,
  VerifyReport,
} from '../api/types';

// Snowflake ids well beyond 2^53 (precision trap on purpose).
export const ID = {
  principalAgent: '7300000000000000123',
  principalHuman: '7300000000000000456',
  credApiKey: '7300000000000000789',
  roleObserver: '7300000000000001001',
  bindingA: '7300000000000001234',
  resourceDb: '7300000000000002001',
  tempGrant: '7300000000000003001',
  auditIntent: '7300000000000004001',
  auditOutcome: '7300000000000004002',
  auditDeny: '7300000000000004003',
} as const;

export const health: Health = {
  status: 'up',
  audit_writable: true,
  audit_watermark: 0.12,
  policy_rev: '4187',
  uptime_ms: 9_432_000,
};

export const modeState: ModeStateRow[] = [
  {
    scope: null,
    mode: 'normal',
    effective_mode: 'normal',
    expires_at: null,
    version: 7,
    updated_at: '2026-06-14T03:11:00Z',
    updated_by: 'admin',
    policy_rev: '4187',
  },
  {
    scope: 'db-main',
    mode: 'maintain',
    effective_mode: 'maintain',
    expires_at: '2026-06-14T06:00:00Z',
    version: 2,
    updated_at: '2026-06-14T02:50:00Z',
    updated_by: 'admin',
    policy_rev: '4180',
  },
];

export const auditEvents: AuditEvent[] = [
  {
    v: 1,
    kind: 'request',
    entry: 'mcp',
    origin: 'unix:uid=1000',
    principal: 'agent-order-bot',
    resource: 'db-main',
    capability: 'query',
    objects: ['table:orders'],
    decision: 'allow',
    stage: null,
    reason: '',
    policy_rev: '4187',
    id: ID.auditOutcome,
    request_id: 'req-aa01',
    ts: '2026-06-14T03:20:11Z',
    principal_id: ID.principalAgent,
    intent_digest: 'sha256:1f3a…9c',
    response_digest: 'sha256:88be…41',
    tier: 'readonly',
    duration_ms: 23,
  },
  {
    v: 1,
    kind: 'request',
    entry: 'mcp',
    origin: 'unix:uid=1000',
    principal: 'agent-order-bot',
    resource: 'db-main',
    capability: 'query',
    objects: ['table:orders'],
    decision: 'allow',
    stage: null,
    reason: '',
    policy_rev: '4187',
    id: ID.auditIntent,
    request_id: 'req-aa01',
    ts: '2026-06-14T03:20:11Z',
    principal_id: ID.principalAgent,
    intent_digest: 'sha256:1f3a…9c',
    tier: 'readonly',
  },
  {
    v: 1,
    kind: 'request',
    entry: 'mcp',
    origin: 'unix:uid=1000',
    principal: 'agent-order-bot',
    resource: 'db-main',
    capability: 'mutate',
    objects: ['table:orders'],
    decision: 'deny',
    stage: 'rbac',
    reason: 'denied at rbac: no grant cell (db-main, mutate) for binding observer',
    policy_rev: '4187',
    id: ID.auditDeny,
    request_id: 'req-aa02',
    ts: '2026-06-14T03:21:40Z',
    principal_id: ID.principalAgent,
    intent_digest: 'sha256:77cd…02',
  },
];

export const denyExample: DenyResponse = {
  decision: 'deny',
  denied: {
    resource: 'db-main',
    capability: 'mutate',
    objects: ['table:orders'],
  },
  reason: 'denied at rbac: no grant cell (db-main, mutate) for binding observer',
  your_grants: {
    'db-main': ['observe', 'query'],
  },
  request_hint: 'postern elevate db-main mutate',
  operator_note: '写操作请走变更单据，联系 DBA 值班。',
};

export const denialsSummary: DenialSummaryRow[] = [
  {
    principal: 'agent-order-bot',
    principal_id: ID.principalAgent,
    resource: 'db-main',
    stage: 'rbac',
    capability: 'mutate',
    count: 42,
    intent_digest: 'sha256:77cd…02',
    policy_rev: '4187',
  },
  {
    principal: 'agent-report-bot',
    principal_id: '7300000000000000999',
    resource: 'api-billing',
    stage: 'classify',
    capability: 'execute',
    count: 11,
    intent_digest: 'sha256:abcd…ef',
    policy_rev: '4187',
  },
];

export const verifyReport: VerifyReport = {
  all_pass: true,
  items: [
    { name: 'scope_out_mutate', pass: true, gap_note: null },
    { name: 'disguised_write', pass: true, gap_note: null },
    { name: 'session_tamper', pass: true, gap_note: null },
    { name: 'multi_statement', pass: true, gap_note: null },
    { name: 'default_deny_unknown_resource', pass: true, gap_note: null },
    { name: 'credential_zero_touch', pass: true, gap_note: null },
    { name: 'origin_not_trusted', pass: true, gap_note: null },
    { name: 'untrusted_origin_auth_stage', pass: true, gap_note: null },
    { name: 'redaction_probe', pass: true, gap_note: null },
  ],
};

export const grantsView: GrantsView = {
  your_grants: {
    'db-main': ['observe', 'query'],
    'api-billing': ['observe'],
  },
  temp_grants: [
    {
      id: ID.tempGrant,
      resource: 'db-main',
      capability: 'mutate',
      granted_at: '2026-06-14T01:00:00Z',
      expires_at: '2026-06-14T05:00:00Z',
      ended_at: null,
      end_reason: null,
      version: 1,
    },
  ],
};

export const principals: PrincipalRow[] = [
  { id: ID.principalAgent, name: 'agent-order-bot', kind: 'agent', version: 3 },
  { id: ID.principalHuman, name: 'alice', kind: 'human', version: 1 },
];

export const credentials: CredentialRow[] = [
  {
    id: ID.credApiKey,
    principal: 'agent-order-bot',
    principal_id: ID.principalAgent,
    kind: 'api_key',
    trust_domain: 'mcp-local',
    expires_at: '2026-09-01T00:00:00Z',
    revoked_at: null,
    version: 2,
  },
];

export const roles: Role[] = [
  {
    id: ID.roleObserver,
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

export const bindings: Binding[] = [
  {
    id: ID.bindingA,
    principal: 'agent-order-bot',
    principal_id: ID.principalAgent,
    role: 'observer',
    scope_kind: 'resource',
    scope_spec: 'db-main',
    expanded_resources: ['db-main'],
    version: 2,
  },
];

export const resources: ResourceRow[] = [
  {
    id: ID.resourceDb,
    code: 'db-main',
    adapter: 'postgres',
    transport: 'direct',
    tiers: [
      { tier: 'readonly', capabilities: ['observe', 'query'], secret_ref: 'vault://db-main/readonly' },
      { tier: 'readwrite', capabilities: ['mutate'], secret_ref: 'vault://db-main/readwrite' },
    ],
    labels: [{ key: 'env', value: 'prod' }],
    enable_flag: true,
    version: 5,
  },
];

export const constraints: ConstraintRow[] = [
  {
    id: '7300000000000005001',
    resource: 'db-main',
    capability: 'query',
    kind: 'table_allow',
    spec: '{"tables":["orders","customers"]}',
    version: 1,
  },
];

export const conditions: ConditionRow[] = [
  {
    id: '7300000000000006001',
    resource: 'db-main',
    capability: null,
    predicate: 'rate_limit',
    spec: '{"per_minute":60}',
    version: 1,
  },
];

export const denyNotes: DenyNoteRow[] = [
  {
    id: '7300000000000007001',
    resource: 'db-main',
    capability: 'mutate',
    note: '写操作请走变更单据，联系 DBA 值班。',
    version: 1,
  },
];

export const settings: SettingRow[] = [
  { key: 'approval.enabled', value: 'false', default: 'false', writable: true, version: 1, kind: 'bool' },
  { key: 'approval.on_timeout', value: 'deny', default: 'deny', writable: false, version: 1, kind: 'enum' },
  { key: 'audit.fsync', value: 'always', default: 'always', writable: true, version: 1, kind: 'enum' },
  { key: 'audit.retention_days', value: '90', default: '90', writable: true, version: 1, kind: 'int' },
  { key: 'audit.exporter.otel.enabled', value: 'false', default: 'false', writable: true, version: 1, kind: 'bool' },
];

export const approvals: ApprovalItem[] = [];
