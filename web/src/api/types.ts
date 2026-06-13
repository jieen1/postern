/**
 * Wire types aligned to the backend control-plane contract.
 *
 * HARD RULES (设计系统 §8, mirrored from postern-core):
 *  - Every snowflake id is a `string` over the wire — NEVER parsed as a JS
 *    number (>2^53 loses precision). All id fields below are typed `string`.
 *  - Closed enums (Capability/Stage/Mode/Decision) are exact unions matching
 *    postern-core; the UI must never invent a value outside these sets.
 *  - Pagination envelope field is `items` (not `list`), with page_no/page_size/
 *    total, default size 20 clamped to [1,200].
 *
 * Types tagged "doc-specified" have no backend DTO yet (the daemon route
 * exists but the response struct is not in postern-core); their shape follows
 * web/docs and is the spec until a backend type lands.
 */

// ── Closed enums (mirror postern-core; non-exhaustive forbidden) ──────────────

/** postern-core `Capability` (domain/capability.rs) — 6 verbs, color-temp order. */
export type Capability =
  | 'observe'
  | 'query'
  | 'mutate'
  | 'execute'
  | 'manage'
  | 'destroy';

export const CAPABILITIES: readonly Capability[] = [
  'observe',
  'query',
  'mutate',
  'execute',
  'manage',
  'destroy',
] as const;

/**
 * postern-core `Stage` (error/stage.rs) — the CLOSED deny-stage vocabulary,
 * exactly 10 values in pipeline order. NOT `#[non_exhaustive]`: the UI renders
 * only these and never fabricates a stage (notably never `connect`).
 */
export type Stage =
  | 'auth'
  | 'classify'
  | 'rbac'
  | 'constraint'
  | 'condition'
  | 'tier'
  | 'transport'
  | 'exec'
  | 'audit'
  | 'discover';

export const STAGES: readonly Stage[] = [
  'auth',
  'classify',
  'rbac',
  'constraint',
  'condition',
  'tier',
  'transport',
  'exec',
  'audit',
  'discover',
] as const;

/** postern-core `Mode` (domain/mod.rs) — kill-switch posture. */
export type Mode = 'normal' | 'observe' | 'maintain' | 'freeze';

export const MODES: readonly Mode[] = [
  'normal',
  'observe',
  'maintain',
  'freeze',
] as const;

/**
 * Decision word as recorded in audit (`decision` string field). Allow/deny are
 * the two-valued surface; an escalation cell folds to a deny (`escalate_denied`)
 * since approval is closed — core holds no pending state.
 */
export type Decision = 'allow' | 'deny' | 'escalate' | 'escalate_denied';

/** postern-core `GrantAction` — per-cell routing at step [6]. */
export type GrantAction = 'allow' | 'escalate';

/** Credential authenticator kind (daemon identity consts). */
export type CredentialKind = 'local_process' | 'api_key' | 'token';

/** Principal kind (store schema CHECK). */
export type PrincipalKind = 'agent' | 'program' | 'human';

/** Resource adapter (doc-specified set). */
export type Adapter = 'postgres' | 'http' | 'docker';

// ── Pagination (postern-core page/mod.rs) ─────────────────────────────────────

export const PAGE_DEFAULT_SIZE = 20;
export const PAGE_MAX_SIZE = 200;
export const PAGE_MIN_SIZE = 1;

export interface PageQuery {
  page_no: number;
  page_size: number;
}

/** Uniform paged-result envelope; field is `items` (NOT `list`). */
export interface Page<T> {
  items: T[];
  page_no: number;
  page_size: number;
  total: number;
}

// ── Deny model (postern-core decision/mod.rs) ─────────────────────────────────

/** Anonymized denial facts (`DeniedFacts`). */
export interface DeniedFacts {
  /** Resource code — always a code, never a real address. */
  resource: string;
  capability: Capability;
  /** Object refs the intent touched (`ObjectRef` strings). */
  objects: string[];
}

/**
 * Structured deny response (`DenyResponse`). Field set is a design promise:
 * exactly these 6. `operator_note` is ABSENT from JSON when unset; relayed
 * verbatim, never reworded. `your_grants` is the principal's OWN world only
 * (scope-bounded; out-of-scope and nonexistent are indistinguishable).
 */
export interface DenyResponse {
  decision: 'deny';
  denied: DeniedFacts;
  reason: string;
  /** resource code → capability-name strings (this principal only). */
  your_grants: Record<string, string[]>;
  /** Mechanical `postern elevate …`; null for ungrantable capabilities. */
  request_hint: string | null;
  operator_note?: string;
}

// ── Audit event (postern-core plugin/audit.rs + doc-specified envelope) ────────

/** Audit event kind (doc-specified taxonomy). */
export type AuditEventKind =
  | 'request'
  | 'policy_change'
  | 'credential_event'
  | 'lifecycle'
  | 'connection_event'
  | 'alert';

/**
 * One audit event. The first block mirrors the in-repo `AuditEvent` carrier;
 * the second block is doc-specified envelope fields (detailed design 5.3) that
 * the row renders when present but which have no backend struct field yet.
 */
export interface AuditEvent {
  // ── core carrier (plugin/audit.rs) ──
  v: number;
  kind: AuditEventKind;
  /** Shell entry: `mcp` / `http`. */
  entry: string;
  /** Gateway-observed origin (doc-specified opaque string form for the SPA). */
  origin: string;
  /** Authenticated principal name, null before/at a step [1] deny. */
  principal: string | null;
  resource: string;
  capability: Capability | null;
  objects: string[];
  decision: Decision;
  stage: Stage | null;
  reason: string;
  /** Policy revision at decision time — string (snowflake-discipline u64). */
  policy_rev: string;

  // ── doc-specified envelope (no backend struct field yet) ──
  /** Event id (snowflake string) — used for two-phase intent/outcome pairing. */
  id?: string;
  /** Request correlation id pairing an intent event with its outcome event. */
  request_id?: string;
  /** Wall-clock ms (string-safe). */
  ts?: string;
  principal_id?: string | null;
  credential_id?: string | null;
  resource_id?: string | null;
  /** Hash digest of the intent payload (never the payload itself). */
  intent_digest?: string;
  /** Hash digest of the response — present on the OUTCOME phase only. */
  response_digest?: string;
  /** Selected credential tier name. */
  tier?: string;
  /** Execution duration — present on the OUTCOME phase only. */
  duration_ms?: number;
}

export interface AuditQuery extends Partial<PageQuery> {
  since?: string;
  principal?: string;
  kind?: AuditEventKind;
  decision?: Decision;
}

// ── Denials summary (doc-specified — no backend DTO) ──────────────────────────

export type DenialWindow = '24h' | '7d' | '30d';

/** One aggregation group of repeated denials (doc-specified). */
export interface DenialSummaryRow {
  principal: string | null;
  principal_id: string | null;
  resource: string;
  stage: Stage;
  capability: Capability;
  count: number;
  /** Truncated sha256 of a sample intent. */
  intent_digest: string;
  policy_rev: string;
}

// ── Verify (postern-core control/verify.rs) ───────────────────────────────────

export interface VerifyItem {
  /** Probe codename — one of nine fixed values. */
  name: string;
  pass: boolean;
  /** Verbatim gap text on FAIL; null when pass. */
  gap_note: string | null;
}

export interface VerifyReport {
  items: VerifyItem[];
  all_pass: boolean;
}

// ── Health (doc-specified — no backend DTO) ───────────────────────────────────

export type HealthStatus = 'up' | 'degraded' | 'down';

export interface Health {
  status: HealthStatus;
  /** Audit store writable. */
  audit_writable: boolean;
  /** 0..1 capacity watermark of the audit store. */
  audit_watermark: number;
  policy_rev: string;
  uptime_ms: number;
}

// ── Mode state (doc-specified projection over POST /v1/mode) ───────────────────

/** One mode-state row: global (scope null) or a per-resource override. */
export interface ModeStateRow {
  /** Resource code, or null for the global jurisdiction. */
  scope: string | null;
  /** The mode set on this jurisdiction. */
  mode: Mode;
  /** Effective mode = global.meet(scoped) = strictest. */
  effective_mode: Mode;
  /** Absolute expiry ms, or null for no TTL. */
  expires_at: string | null;
  version: number;
  updated_at: string | null;
  updated_by: string | null;
  policy_rev: string;
}

export interface ModeSetRequest {
  /** null = global jurisdiction. */
  scope: string | null;
  mode: Mode;
  /** TTL ms; omit/null for no expiry. */
  ttl_ms?: number | null;
  /** Optimistic-lock version. */
  version: number;
}

// ── Grants (your_grants source + doc-specified temp rows) ─────────────────────

export interface TempGrantRow {
  id: string;
  resource: string;
  capability: Capability;
  granted_at: string;
  expires_at: string;
  ended_at: string | null;
  end_reason: string | null;
  version: number;
}

export interface GrantsView {
  /** resource code → capability-name strings. */
  your_grants: Record<string, string[]>;
  temp_grants: TempGrantRow[];
}

export interface ElevateRequest {
  principal: string;
  resource: string;
  capability: Capability;
  /** Required, > 0. */
  ttl_ms: number;
}

export interface RevokeRequest {
  id: string;
  version: number;
}

// ── Roles / Bindings / Constraints / Conditions / Deny-notes (doc-specified) ──

export interface RoleCapability {
  capability: Capability;
  action: GrantAction;
}

export interface Role {
  id: string;
  name: string;
  /** Daemon-expanded effective set (with inheritance). */
  effective: RoleCapability[];
  /** Directly declared set. */
  direct: RoleCapability[];
  inherits_from: string[];
  version: number;
  updated_at: string | null;
  updated_by: string | null;
}

export type ScopeKind = 'resource' | 'selector';

export interface Binding {
  id: string;
  principal: string;
  principal_id: string;
  role: string;
  scope_kind: ScopeKind;
  /** Concrete codes (resource kind) or raw selector spec text (selector kind). */
  scope_spec: string;
  expanded_resources: string[];
  version: number;
}

/** ConstraintSpec — kind + raw JSON spec text (adapter-interpreted). */
export interface ConstraintRow {
  id: string;
  resource: string;
  capability: Capability;
  kind: string;
  spec: string;
  version: number;
}

/** ConditionSpec — predicate kind + raw JSON spec text. */
export interface ConditionRow {
  id: string;
  resource: string | null;
  capability: Capability | null;
  predicate: string;
  spec: string;
  version: number;
}

export interface DenyNoteRow {
  id: string;
  resource: string;
  capability: Capability;
  /** Verbatim operator note (== DenyResponse.operator_note). */
  note: string;
  version: number;
}

// ── Resources / Principals / Credentials ──────────────────────────────────────

export interface ResourceTier {
  tier: string;
  capabilities: Capability[];
  /** Opaque reference like `vault://…` — never a secret value. */
  secret_ref: string;
}

export interface ResourceLabel {
  key: string;
  value: string;
}

export interface ResourceRow {
  id: string;
  code: string;
  adapter: Adapter;
  transport: string;
  tiers: ResourceTier[];
  labels: ResourceLabel[];
  enable_flag: boolean;
  version: number;
}

/** CapabilitySurface (plugin/channel.rs) — discovery is NOT authorization. */
export interface CapabilitySurface {
  capabilities: Capability[];
  objects: string[];
}

export interface PrincipalRow {
  id: string;
  name: string;
  kind: PrincipalKind;
  version: number;
}

/**
 * CredentialMeta (domain/snapshot.rs) — metadata + secret HASH presence only.
 * `secret_hash` is NEVER fetched or shown by the SPA; it is intentionally
 * omitted from this type so the UI cannot render it.
 */
export interface CredentialRow {
  id: string;
  principal: string;
  principal_id: string;
  kind: CredentialKind;
  /** Trust-domain label (doc-specified). */
  trust_domain: string | null;
  expires_at: string | null;
  revoked_at: string | null;
  version: number;
}

// ── Settings / Approvals (doc-specified) ──────────────────────────────────────

export interface SettingRow {
  key: string;
  value: string;
  default: string;
  writable: boolean;
  version: number;
  /** e.g. `bool` / `enum` / `int`. */
  kind: string;
}

export interface ApprovalItem {
  id: string;
  principal: string;
  resource: string;
  capability: Capability;
  status: string;
  policy_rev: string;
  expires_at: string | null;
}

// ── Write-success / error envelopes ───────────────────────────────────────────

/** Standard write success: the new policy revision (reconciliation anchor). */
export interface WriteAck {
  policy_rev: string;
}

/** Standard error body `{ error: { code, message } }`. */
export interface ApiErrorBody {
  error: {
    code: string;
    message: string;
  };
}
