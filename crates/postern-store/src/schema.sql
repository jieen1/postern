-- policy.db 权威策略状态结构定义（详细设计 5.2，SQLite + WAL）。
--
-- 本文件是 DB_BASE_FIELDS_REQUIRED / SEC_ADMIN_NOT_GRANTABLE 两条契约扫描器的真
-- 来源：每张业务表都声明全 8 基础字段（5.1-①），roles 带禁 admin 名 CHECK。
-- 约定（5.2）：8 基础列在前、业务列在后；时间列固定宽度 24 文本；JSON 列由对应
-- 插件校验语义；唯一性统一用 partial unique index（WHERE delete_flag = 0）；限制性
-- 表（grant_constraints/grant_conditions/mode_state/deny_notes）建表带 CHECK
-- (enable_flag = 1)。PRAGMA foreign_keys=ON / journal_mode=WAL 由开库时统一施加。

-- principals：具名主体（无全局匿名身份，3.1）。
CREATE TABLE principals (
  id          INTEGER PRIMARY KEY,
  version     INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by  TEXT    NOT NULL,
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by  TEXT    NOT NULL,
  delete_flag INTEGER NOT NULL DEFAULT 0,
  enable_flag INTEGER NOT NULL DEFAULT 1,
  name        TEXT    NOT NULL,
  kind        TEXT    NOT NULL CHECK (kind IN ('agent','program','human'))
);
CREATE UNIQUE INDEX uq_principals_name ON principals(name) WHERE delete_flag = 0;

-- credentials：凭证元数据（明文永不落库，仅 argon2id secret_hash；禁 enable_flag）。
CREATE TABLE credentials (
  id            INTEGER PRIMARY KEY,
  version       INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by    TEXT    NOT NULL,
  updated_at    TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by    TEXT    NOT NULL,
  delete_flag   INTEGER NOT NULL DEFAULT 0,
  enable_flag   INTEGER NOT NULL DEFAULT 1 CHECK (enable_flag = 1),
  principal_id  INTEGER NOT NULL REFERENCES principals(id),
  kind          TEXT    NOT NULL CHECK (kind IN ('local_process','api_key','token')),
  secret_hash   TEXT,
  match_spec    TEXT,
  trust_domain  TEXT,
  expires_at    TEXT,
  revoked_at    TEXT
);
CREATE INDEX ix_credentials_principal ON credentials(principal_id);

-- roles：信任等级（动词集），admin 名硬禁（模型层 CHECK，防大小写/空白绕过）。
CREATE TABLE roles (
  id          INTEGER PRIMARY KEY,
  version     INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by  TEXT    NOT NULL,
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by  TEXT    NOT NULL,
  delete_flag INTEGER NOT NULL DEFAULT 0,
  enable_flag INTEGER NOT NULL DEFAULT 1,
  name        TEXT    NOT NULL CHECK (lower(trim(name)) <> 'admin'),
  description TEXT
);
CREATE UNIQUE INDEX uq_roles_name ON roles(name) WHERE delete_flag = 0;

-- role_inherits：角色继承边（应用层校验无环）。
CREATE TABLE role_inherits (
  id             INTEGER PRIMARY KEY,
  version        INTEGER NOT NULL DEFAULT 0,
  created_at     TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by     TEXT    NOT NULL,
  updated_at     TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by     TEXT    NOT NULL,
  delete_flag    INTEGER NOT NULL DEFAULT 0,
  enable_flag    INTEGER NOT NULL DEFAULT 1,
  role_id        INTEGER NOT NULL REFERENCES roles(id),
  parent_role_id INTEGER NOT NULL REFERENCES roles(id)
);
CREATE UNIQUE INDEX uq_role_inherits ON role_inherits(role_id, parent_role_id) WHERE delete_flag = 0;

-- role_capabilities：角色→动词（6 动词 CHECK；action allow/escalate）。
CREATE TABLE role_capabilities (
  id          INTEGER PRIMARY KEY,
  version     INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by  TEXT    NOT NULL,
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by  TEXT    NOT NULL,
  delete_flag INTEGER NOT NULL DEFAULT 0,
  enable_flag INTEGER NOT NULL DEFAULT 1,
  role_id     INTEGER NOT NULL REFERENCES roles(id),
  capability  TEXT    NOT NULL CHECK (capability IN ('observe','query','mutate','execute','manage','destroy')),
  action      TEXT    NOT NULL CHECK (action IN ('allow','escalate'))
);
CREATE UNIQUE INDEX uq_role_capabilities ON role_capabilities(role_id, capability) WHERE delete_flag = 0;

-- resources：资源建模（本库不存任何真实地址；敏感项 vault:// 引用）。
CREATE TABLE resources (
  id               INTEGER PRIMARY KEY,
  version          INTEGER NOT NULL DEFAULT 0,
  created_at       TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by       TEXT    NOT NULL,
  updated_at       TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by       TEXT    NOT NULL,
  delete_flag      INTEGER NOT NULL DEFAULT 0,
  enable_flag      INTEGER NOT NULL DEFAULT 1,
  codename         TEXT    NOT NULL,
  adapter          TEXT    NOT NULL,
  transport        TEXT    NOT NULL,
  transport_config TEXT
);
CREATE UNIQUE INDEX uq_resources_codename ON resources(codename) WHERE delete_flag = 0;

-- resource_labels：资源标签（供 Scope 选择器按标签展开）。
CREATE TABLE resource_labels (
  id          INTEGER PRIMARY KEY,
  version     INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by  TEXT    NOT NULL,
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by  TEXT    NOT NULL,
  delete_flag INTEGER NOT NULL DEFAULT 0,
  enable_flag INTEGER NOT NULL DEFAULT 1,
  resource_id INTEGER NOT NULL REFERENCES resources(id),
  key         TEXT    NOT NULL,
  value       TEXT    NOT NULL
);
CREATE UNIQUE INDEX uq_resource_labels ON resource_labels(resource_id, key) WHERE delete_flag = 0;

-- resource_credential_tiers：引擎账号档（secret_ref vault://；auth_flow 仅非敏感）。
CREATE TABLE resource_credential_tiers (
  id           INTEGER PRIMARY KEY,
  version      INTEGER NOT NULL DEFAULT 0,
  created_at   TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by   TEXT    NOT NULL,
  updated_at   TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by   TEXT    NOT NULL,
  delete_flag  INTEGER NOT NULL DEFAULT 0,
  enable_flag  INTEGER NOT NULL DEFAULT 1,
  resource_id  INTEGER NOT NULL REFERENCES resources(id),
  tier         TEXT    NOT NULL,
  capabilities TEXT,
  secret_ref   TEXT,
  auth_flow    TEXT
);
CREATE UNIQUE INDEX uq_resource_credential_tiers ON resource_credential_tiers(resource_id, tier) WHERE delete_flag = 0;

-- bindings：主体↔角色绑定。
CREATE TABLE bindings (
  id           INTEGER PRIMARY KEY,
  version      INTEGER NOT NULL DEFAULT 0,
  created_at   TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by   TEXT    NOT NULL,
  updated_at   TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by   TEXT    NOT NULL,
  delete_flag  INTEGER NOT NULL DEFAULT 0,
  enable_flag  INTEGER NOT NULL DEFAULT 1,
  principal_id INTEGER NOT NULL REFERENCES principals(id),
  role_id      INTEGER NOT NULL REFERENCES roles(id)
);
CREATE INDEX ix_bindings_principal ON bindings(principal_id);
CREATE UNIQUE INDEX uq_bindings ON bindings(principal_id, role_id) WHERE delete_flag = 0;

-- binding_scope：绑定辖区（resource 枚举 / selector 标签选择器，二选一）。
CREATE TABLE binding_scope (
  id          INTEGER PRIMARY KEY,
  version     INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by  TEXT    NOT NULL,
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by  TEXT    NOT NULL,
  delete_flag INTEGER NOT NULL DEFAULT 0,
  enable_flag INTEGER NOT NULL DEFAULT 1,
  binding_id  INTEGER NOT NULL REFERENCES bindings(id),
  kind        TEXT    NOT NULL CHECK (kind IN ('resource','selector')),
  resource_id INTEGER REFERENCES resources(id),
  selector    TEXT
);

-- grant_constraints：对象细则（限制性表：CHECK enable_flag = 1）。
CREATE TABLE grant_constraints (
  id          INTEGER PRIMARY KEY,
  version     INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by  TEXT    NOT NULL,
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by  TEXT    NOT NULL,
  delete_flag INTEGER NOT NULL DEFAULT 0,
  enable_flag INTEGER NOT NULL DEFAULT 1 CHECK (enable_flag = 1),
  resource_id INTEGER NOT NULL REFERENCES resources(id),
  capability  TEXT    NOT NULL,
  kind        TEXT    NOT NULL,
  spec        TEXT
);
CREATE INDEX ix_grant_constraints ON grant_constraints(resource_id, capability);

-- grant_conditions：求值条件（限制性表：CHECK enable_flag = 1；NULL 列无唯一性是有意设计）。
CREATE TABLE grant_conditions (
  id          INTEGER PRIMARY KEY,
  version     INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by  TEXT    NOT NULL,
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by  TEXT    NOT NULL,
  delete_flag INTEGER NOT NULL DEFAULT 0,
  enable_flag INTEGER NOT NULL DEFAULT 1 CHECK (enable_flag = 1),
  resource_id INTEGER REFERENCES resources(id),
  capability  TEXT,
  predicate   TEXT    NOT NULL,
  spec        TEXT
);

-- temp_grants：临时授权（终态字段 ended_at/end_reason；禁 enable_flag）。
CREATE TABLE temp_grants (
  id           INTEGER PRIMARY KEY,
  version      INTEGER NOT NULL DEFAULT 0,
  created_at   TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by   TEXT    NOT NULL,
  updated_at   TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by   TEXT    NOT NULL,
  delete_flag  INTEGER NOT NULL DEFAULT 0,
  enable_flag  INTEGER NOT NULL DEFAULT 1 CHECK (enable_flag = 1),
  principal_id INTEGER NOT NULL REFERENCES principals(id),
  resource_id  INTEGER NOT NULL REFERENCES resources(id),
  capability   TEXT    NOT NULL,
  granted_at   TEXT    NOT NULL CHECK (length(granted_at) = 24),
  expires_at   TEXT    NOT NULL CHECK (length(expires_at) = 24),
  ended_at     TEXT,
  end_reason   TEXT    CHECK (end_reason IN ('expired','revoked'))
);
CREATE INDEX ix_temp_grants_principal ON temp_grants(principal_id, expires_at);

-- mode_state：辖区运行模式（限制性表：CHECK enable_flag = 1；全局唯一走 COALESCE 哨兵）。
CREATE TABLE mode_state (
  id                INTEGER PRIMARY KEY,
  version           INTEGER NOT NULL DEFAULT 0,
  created_at        TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by        TEXT    NOT NULL,
  updated_at        TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by        TEXT    NOT NULL,
  delete_flag       INTEGER NOT NULL DEFAULT 0,
  enable_flag       INTEGER NOT NULL DEFAULT 1 CHECK (enable_flag = 1),
  scope_resource_id INTEGER REFERENCES resources(id),
  mode              TEXT    NOT NULL CHECK (mode IN ('normal','observe','maintain','freeze')),
  expires_at        TEXT
);
CREATE UNIQUE INDEX uq_mode_state_scope ON mode_state(COALESCE(scope_resource_id, 0)) WHERE delete_flag = 0;

-- deny_notes：人亲笔预写的拒绝说明（限制性表：CHECK enable_flag = 1）。
CREATE TABLE deny_notes (
  id          INTEGER PRIMARY KEY,
  version     INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),
  created_by  TEXT    NOT NULL,
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),
  updated_by  TEXT    NOT NULL,
  delete_flag INTEGER NOT NULL DEFAULT 0,
  enable_flag INTEGER NOT NULL DEFAULT 1 CHECK (enable_flag = 1),
  resource_id INTEGER NOT NULL REFERENCES resources(id),
  capability  TEXT    NOT NULL,
  note        TEXT    NOT NULL
);
CREATE UNIQUE INDEX uq_deny_notes ON deny_notes(resource_id, capability) WHERE delete_flag = 0;
