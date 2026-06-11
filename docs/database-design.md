# Pandaria 生态 — 统一数据库设计方案

> 状态：Proposed
> 日期：2026-06-11
> 版本：v1.0

---

## 1. 设计原则

### 1.1 核心决策：各自独立，schema 统一

| 决策 | 结论 |
|------|------|
| 每个项目是否有自己的数据库？ | **是**。Aspectus、Pandaria、Tavern 各自拥有独立的 PostgreSQL 数据库 |
| 是否共享同一个 PostgreSQL 实例？ | **可以**（开发/小规模部署），但逻辑上各自数据库彼此不可见 |
| 跨库是否有外键约束？ | **没有**。`tenant_id` 是应用层的逻辑纽带，非数据库级外键 |
| Pawbun 是否需要数据库？ | **不**。它是 Rust library，不持久化数据 |

### 1.2 为什么分开而不是合库

| 维度 | 合库风险 | 分开收益 |
|------|---------|---------|
| **热路径隔离** | Tavern 大事件流回放可能拖慢 Aspectus 自省（P95 < 5ms 硬约束） | 各自 IO 预算独立，互不干扰 |
| **故障隔离** | 一个项目的慢查询/死锁可能波及全生态 | 单库故障不扩散 |
| **安全边界** | Tavern/Pandaria 被攻破 = 能读到 Aspectus 的用户密码哈希 | 每个服务只连自己的 DB，攻击面隔离 |
| **备份策略** | audit_log 合规保留 7 年，session 7 天自动过期——策略必须取最大公约数 | 各自 RPO/RTO、保留周期独立配置 |
| **资源扩缩** | 无法按项目独立升配、读写分离 | Pandaria 读多写少、Tavern 顺序写密集——各自独立调优 |
| **Schema 迁移** | 一个项目的 migration 变更需要评估对其他项目的性能影响 | 各自独立 sqlx migrate，节奏自由 |

### 1.3 何时可以合库

仅在以下场景合并到一个 PG 实例的多个 database 是合理的：
- 本地开发（docker-compose 起一个 PG，三个 database）
- 极小规模部署（单机、单租户、日均 < 1000 请求）

此时用 PostgreSQL 的 database 级隔离（非 schema 级），权限完全分离。

---

## 2. 生态全景：谁有数据库

```
         ┌──────────┐     ┌──────────┐     ┌──────────┐
         │ Aspectus │     │ Pandaria │     │  Tavern  │
         │   有 DB   │     │   有 DB   │     │   有 DB   │
         └─────┬────┘     └─────┬────┘     └─────┬────┘
               │                │                │
               │   tenant_id (应用层逻辑纽带)       │
               │                │                │
         ┌─────┴────────────────┴────────────────┴─────┐
         │              同一个 PostgreSQL 实例            │
         │  DATABASE aspectus │ pandaria │ tavern       │
         └──────────────────────────────────────────────┘

         ┌──────────┐
         │  Pawbun  │  ← 无数据库（纯 Rust library）
         └──────────┘
```

---

## 3. 数据库：`aspectus`

**所属项目**：Aspectus — 统一身份与多租户管理服务
**定位**：生态的单一身份源。Tenant、User、API Key、Role、Scope、审计日志的权威存储。

### 3.1 枚举类型

```sql
CREATE TYPE identity_type AS ENUM ('user', 'service_account');
CREATE TYPE project AS ENUM ('pandaria','tavern','emerald','constell','tokencamp','heirloom');
CREATE TYPE role_type AS ENUM ('user', 'service_account', 'both');
CREATE TYPE password_encryption_method AS ENUM ('Argon2id');
```

### 3.2 核心表

```sql
-- ============================================================
-- 租户
-- ============================================================
CREATE TABLE tenants (
    id          VARCHAR(21) PRIMARY KEY,
    name        VARCHAR(128) NOT NULL,
    quotas      JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================================
-- 用户
-- ============================================================
CREATE TABLE users (
    id                          VARCHAR(21) PRIMARY KEY,
    tenant_id                   VARCHAR(21) NOT NULL REFERENCES tenants(id)
                                    ON UPDATE CASCADE ON DELETE CASCADE,
    email                       VARCHAR(256),
    password_hash               VARCHAR(256),
    password_encryption_method  password_encryption_method DEFAULT 'Argon2id',
    display_name                VARCHAR(128),
    is_suspended                BOOLEAN NOT NULL DEFAULT false,
    last_sign_in_at             TIMESTAMPTZ,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT users__email UNIQUE (tenant_id, email)
);

CREATE INDEX users__tenant ON users (tenant_id, id);

-- ============================================================
-- 服务账号
-- ============================================================
CREATE TABLE service_accounts (
    id          VARCHAR(21) PRIMARY KEY,
    tenant_id   VARCHAR(21) NOT NULL REFERENCES tenants(id)
                    ON UPDATE CASCADE ON DELETE CASCADE,
    label       VARCHAR(128) NOT NULL,
    description TEXT,
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX service_accounts__tenant
    ON service_accounts (tenant_id, id);
CREATE INDEX service_accounts__tenant_created
    ON service_accounts (tenant_id, created_at DESC);

-- ============================================================
-- API Key
-- ============================================================
CREATE TABLE api_keys (
    id                  VARCHAR(21) PRIMARY KEY,
    tenant_id           VARCHAR(21) NOT NULL REFERENCES tenants(id)
                            ON UPDATE CASCADE ON DELETE CASCADE,
    service_account_id  VARCHAR(21) REFERENCES service_accounts(id)
                            ON UPDATE CASCADE ON DELETE CASCADE,
    user_id             VARCHAR(21) REFERENCES users(id)
                            ON UPDATE CASCADE ON DELETE CASCADE,
    project             project NOT NULL,
    token_type          VARCHAR(16) NOT NULL DEFAULT 'api_key',
    key_hash            VARCHAR(64) NOT NULL,
    key_prefix          VARCHAR(32) NOT NULL,
    scopes              TEXT[] NOT NULL DEFAULT '{}',
    expires_at          TIMESTAMPTZ,
    revoked_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT api_keys__one_owner CHECK (
        (user_id IS NOT NULL AND service_account_id IS NULL) OR
        (user_id IS NULL AND service_account_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX api_keys__key_hash ON api_keys (key_hash);
CREATE INDEX api_keys__tenant ON api_keys (tenant_id, created_at DESC);
CREATE INDEX api_keys__service_account
    ON api_keys (tenant_id, service_account_id, project);
CREATE INDEX api_keys__service_account_id ON api_keys (service_account_id);

-- ============================================================
-- Scope 定义
-- ============================================================
CREATE TABLE scopes (
    id          VARCHAR(21) PRIMARY KEY,
    name        VARCHAR(256) NOT NULL UNIQUE,
    description TEXT
);

-- ============================================================
-- 角色
-- ============================================================
CREATE TABLE roles (
    id          VARCHAR(21) PRIMARY KEY,
    name        VARCHAR(128) NOT NULL UNIQUE,
    description VARCHAR(256),
    type        role_type NOT NULL DEFAULT 'user',
    is_default  BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX roles__type ON roles (type);

-- ============================================================
-- 角色-Scope 关联
-- ============================================================
CREATE TABLE roles_scopes (
    id        VARCHAR(21) PRIMARY KEY,
    role_id   VARCHAR(21) NOT NULL REFERENCES roles(id)
                  ON UPDATE CASCADE ON DELETE CASCADE,
    scope_id  VARCHAR(21) NOT NULL REFERENCES scopes(id)
                  ON UPDATE CASCADE ON DELETE CASCADE,
    UNIQUE (role_id, scope_id)
);

-- ============================================================
-- 用户-角色关联
-- ============================================================
CREATE TABLE users_roles (
    id        VARCHAR(21) PRIMARY KEY,
    user_id   VARCHAR(21) NOT NULL REFERENCES users(id)
                  ON UPDATE CASCADE ON DELETE CASCADE,
    role_id   VARCHAR(21) NOT NULL REFERENCES roles(id)
                  ON UPDATE CASCADE ON DELETE CASCADE,
    UNIQUE (user_id, role_id),
    CONSTRAINT users_roles__role_type CHECK (
        check_role_type(role_id, ARRAY['user','both']::role_type[])
    )
);

-- role_type 约束函数
CREATE FUNCTION check_role_type(
    target_role_id  VARCHAR(21),
    allowed_types   role_type[]
) RETURNS BOOLEAN AS $$
BEGIN
    RETURN (SELECT type FROM roles WHERE id = target_role_id) = ANY(allowed_types);
END;
$$ LANGUAGE plpgsql;

-- ============================================================
-- Service Token（项目间内部认证）
-- ============================================================
CREATE TABLE service_tokens (
    project     project NOT NULL PRIMARY KEY,
    token_hash  VARCHAR(64) NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX service_tokens__token_hash ON service_tokens (token_hash);

-- ============================================================
-- OAuth2
-- ============================================================
CREATE TABLE oauth2_clients (
    client_id     VARCHAR(64) PRIMARY KEY,
    name          VARCHAR(128) NOT NULL,
    redirect_uris TEXT[] NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE authorization_codes (
    code         VARCHAR(64) PRIMARY KEY,
    user_id      VARCHAR(21) NOT NULL REFERENCES users(id)
                     ON UPDATE CASCADE ON DELETE CASCADE,
    client_id    VARCHAR(21) NOT NULL,
    redirect_uri TEXT NOT NULL,
    expires_at   TIMESTAMPTZ NOT NULL,
    used         BOOLEAN NOT NULL DEFAULT false
);

CREATE TABLE refresh_tokens (
    token_hash  VARCHAR(64) PRIMARY KEY,
    user_id     VARCHAR(21) NOT NULL REFERENCES users(id)
                    ON UPDATE CASCADE ON DELETE CASCADE,
    client_id   VARCHAR(21) NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    revoked_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================================
-- 审计日志
-- ============================================================
CREATE TABLE audit_logs (
    id          VARCHAR(21) PRIMARY KEY,
    tenant_id   VARCHAR(21) NOT NULL REFERENCES tenants(id)
                    ON UPDATE CASCADE ON DELETE CASCADE,
    actor_id    VARCHAR(21) NOT NULL,
    actor_type  identity_type NOT NULL,
    action      VARCHAR(64) NOT NULL,
    target_type VARCHAR(32) NOT NULL,
    target_id   VARCHAR(21) NOT NULL,
    metadata    JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX audit_logs__tenant ON audit_logs (tenant_id, created_at DESC);
CREATE INDEX audit_logs__actor  ON audit_logs (tenant_id, actor_id, created_at DESC);
CREATE INDEX audit_logs__action ON audit_logs (tenant_id, action, created_at DESC);
CREATE INDEX audit_logs__target ON audit_logs (target_type, target_id);
CREATE INDEX audit_logs__created_at ON audit_logs (created_at DESC);

-- ============================================================
-- 触发器
-- ============================================================
CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER set_users_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE PROCEDURE set_updated_at();

CREATE TRIGGER set_service_tokens_updated_at
    BEFORE UPDATE ON service_tokens
    FOR EACH ROW EXECUTE PROCEDURE set_updated_at();
```

### 3.3 种子数据

```sql
-- Scope 定义（生态 6 个项目的权限标签）
INSERT INTO scopes (id, name, description) VALUES
-- Pandaria
('sc_pa_session_create', 'pandaria:session:create', 'Create agent session'),
('sc_pa_session_read',   'pandaria:session:read',   'Read session details'),
('sc_pa_session_delete', 'pandaria:session:delete', 'Delete a session'),
('sc_pa_session_manage', 'pandaria:session:manage', 'Manage session lifecycle'),
('sc_pa_agent_execute',  'pandaria:agent:execute',  'Execute an agent task'),
('sc_pa_agent_manage',   'pandaria:agent:manage',   'Register/configure agents'),
-- Tavern
('sc_tv_workflow_run',    'tavern:workflow:run',    'Execute a workflow'),
('sc_tv_workflow_deploy', 'tavern:workflow:deploy', 'Deploy a workflow definition'),
('sc_tv_workflow_read',   'tavern:workflow:read',   'Read workflow status'),
('sc_tv_workflow_manage', 'tavern:workflow:manage', 'Manage workflow definitions');

-- 角色
INSERT INTO roles (id, name, description, type, is_default) VALUES
('role_tenant_admin', 'tenant-admin',    'Full tenant management',       'both',             false),
('role_agent_dev',    'agent-developer', 'Agent development access',     'user',             true),
('role_agent_op',     'agent-operator',  'Agent operation access',       'user',             false),
('role_ci_deployer',  'ci-deployer',     'CI/CD deployment',             'service_account',  false);
```

---

## 4. 数据库：`pandaria`

**所属项目**：Pandaria — Agent Runtime
**定位**：Session 状态持久化、Token 消耗计量、租户资源用量计数。

### 4.1 Session 会话

```sql
CREATE TABLE sessions (
    tenant_id   TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    entries     JSONB NOT NULL DEFAULT '[]',
    status      VARCHAR(16) NOT NULL DEFAULT 'active',
    metadata    JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, session_id)
);

CREATE INDEX idx_sessions_tenant  ON sessions (tenant_id, updated_at DESC);
CREATE INDEX idx_sessions_status  ON sessions (tenant_id, status);
```

**entries JSONB 结构**：

```json
[
  {
    "type": "Message",
    "id": "msg_abc123",
    "message": {
      "type": "UserMessage",
      "content": [{"type": "Text", "text": "帮我重构这个文件"}]
    }
  },
  {
    "type": "Message",
    "id": "msg_def456",
    "message": {
      "type": "AssistantMessage",
      "content": [...],
      "provider": "anthropic",
      "model": "claude-sonnet-4-5-20250929",
      "usage": {"input_tokens": 500, "output_tokens": 200},
      "stop_reason": "Stop",
      "response_id": "resp_xyz"
    }
  },
  {
    "type": "Compaction",
    "id": "comp_001",
    "summary": "Prior context summary...",
    "first_kept_entry_id": "msg_abc123",
    "tokens_before": 15000,
    "timestamp": "2026-06-11T10:30:00Z"
  }
]
```

### 4.2 Token 消耗明细

```sql
CREATE TABLE session_token_usage (
    id             BIGSERIAL PRIMARY KEY,
    tenant_id      TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    turn_number    INTEGER NOT NULL,
    input_tokens   BIGINT NOT NULL DEFAULT 0,
    output_tokens  BIGINT NOT NULL DEFAULT 0,
    model          VARCHAR(128),
    provider       VARCHAR(64),
    recorded_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_token_usage_tenant  ON session_token_usage (tenant_id, recorded_at DESC);
CREATE INDEX idx_token_usage_session ON session_token_usage (tenant_id, session_id);
```

### 4.3 租户资源用量计数（滑窗）

```sql
CREATE TABLE tenant_usage_counters (
    tenant_id    TEXT NOT NULL,
    metric       VARCHAR(64) NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    value        BIGINT NOT NULL DEFAULT 0,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, metric, window_start)
);

-- 常见 metric 值：
--   'monthly_tokens'        — 月 token 消耗
--   'concurrent_sessions'   — 当前活跃 session 数
--   'daily_tool_calls'      — 日工具调用次数
```

### 4.4 Redis 补充

Pandaria 同时使用 Redis 作为 session 的热缓存层（低延迟读写），键命名规范：

```
pandaria:session:{tenant_id}:{session_id}   → JSON entries (TTL 7天)
pandaria:tenant:{tenant_id}:sessions        → SET of session_id
```

Redis 是 **缓存**，非权威存储。PG 的 `sessions` 表是唯一真相源。

---

## 5. 数据库：`tavern`

**所属项目**：Tavern — 多 Agent 工作流编排
**定位**：Agent 定义、Workflow 定义、执行实例（事件溯源状态机）、事件流和快照。

### 5.1 Agent 定义

```sql
CREATE TABLE agent_definitions (
    id                  VARCHAR(64) PRIMARY KEY,
    tenant_id           TEXT NOT NULL,
    name                VARCHAR(128) NOT NULL,
    description         TEXT,
    model_provider      VARCHAR(64) NOT NULL,
    model_name          VARCHAR(128) NOT NULL,
    model_temperature   FLOAT NOT NULL DEFAULT 0.7,
    instructions        TEXT NOT NULL,
    skills              JSONB NOT NULL DEFAULT '[]',
    constraints         JSONB NOT NULL DEFAULT '[]',
    memory_config       JSONB NOT NULL DEFAULT '{"enabled": false}',
    source              VARCHAR(64) NOT NULL DEFAULT 'yaml',
    version             INTEGER NOT NULL DEFAULT 1,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_agent_defs_tenant ON agent_definitions (tenant_id, id);
```

**skills JSONB 结构**：

```json
[
  {
    "id": "web_search",
    "name": "web_search",
    "description": "Search the web",
    "parameters": {
      "type": "object",
      "properties": {
        "query": {"type": "string", "description": "Search query"}
      },
      "required": ["query"]
    },
    "timeout_ms": 30000,
    "runner": "subprocess",
    "command": "search-tool",
    "config": {"max_results": 5}
  }
]
```

### 5.2 Workflow 定义

```sql
CREATE TABLE workflow_definitions (
    id              VARCHAR(64) PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    name            VARCHAR(128) NOT NULL,
    description     TEXT,
    process         VARCHAR(32) NOT NULL DEFAULT 'sequential',
    steps           JSONB NOT NULL,
    inputs          JSONB NOT NULL DEFAULT '[]',
    outputs         JSONB NOT NULL DEFAULT '[]',
    planning_config JSONB,
    webhook_config  JSONB,
    schedule        VARCHAR(64),
    schedule_inputs JSONB DEFAULT '{}',
    source          VARCHAR(64) NOT NULL DEFAULT 'yaml',
    version         INTEGER NOT NULL DEFAULT 1,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_workflow_defs_tenant ON workflow_definitions (tenant_id, id);
```

**steps JSONB 结构**：

```json
[
  {
    "id": "research",
    "agent_id": "researcher",
    "task": "研究以下主题: {{topic}}",
    "depends_on": [],
    "or_depends_on": [],
    "output_key": "research_notes",
    "timeout": 300,
    "retries": 1,
    "retry_delay": 2,
    "wait_for_signal": null,
    "signal_timeout": null,
    "breakpoint": false,
    "model_override": null,
    "expected_output": "Detailed research notes",
    "router": null
  }
]
```

### 5.3 工作流执行实例

```sql
CREATE TABLE workflow_instances (
    instance_id   TEXT PRIMARY KEY,
    tenant_id     TEXT NOT NULL,
    workflow_id   TEXT NOT NULL,
    status        TEXT NOT NULL,
    inputs        JSONB NOT NULL DEFAULT '{}',
    context       JSONB NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at  TIMESTAMPTZ
);

CREATE INDEX idx_instances_tenant   ON workflow_instances (tenant_id);
CREATE INDEX idx_instances_status   ON workflow_instances (status);
CREATE INDEX idx_instances_workflow ON workflow_instances (workflow_id);
CREATE INDEX idx_instances_created  ON workflow_instances (tenant_id, created_at DESC);
```

**status 取值**：`pending` | `running` | `waiting_for_signal` | `sleeping` | `completed` | `failed`

### 5.4 事件流（事件溯源核心）

```sql
CREATE TABLE workflow_events (
    id           BIGSERIAL PRIMARY KEY,
    instance_id  TEXT NOT NULL REFERENCES workflow_instances(instance_id) ON DELETE CASCADE,
    event_type   VARCHAR(64) NOT NULL,
    step_id      VARCHAR(64),
    payload      JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_events_instance_seq ON workflow_events (instance_id, id);
CREATE INDEX idx_events_type         ON workflow_events (instance_id, event_type);
CREATE INDEX idx_events_step         ON workflow_events (instance_id, step_id)
    WHERE step_id IS NOT NULL;
```

**event_type 枚举**（对应 Rust `WorkflowEvent` enum）：

| event_type | 说明 |
|-----------|------|
| `instance_created` | 实例创建 |
| `instance_started` | 开始执行 |
| `step_scheduled` | 步骤已调度 |
| `step_started` | 步骤开始 |
| `step_completed` | 步骤完成（含 output） |
| `step_failed` | 步骤失败（含 error、will_retry） |
| `step_retry_scheduled` | 重试已调度 |
| `signal_wait_started` | 等待外部信号 |
| `breakpoint_hit` | 断点命中暂停 |
| `signal_received` | 信号到达（含 action: approve / reject） |
| `timer_fired` | 定时器触发 |
| `cancel_requested` | 取消请求 |
| `llm_call_started` | LLM 调用开始 |
| `llm_call_completed` | LLM 调用完成（含 usage） |
| `llm_call_failed` | LLM 调用失败 |
| `tool_call_started` | 工具调用开始 |
| `tool_call_completed` | 工具调用完成 |
| `tool_call_failed` | 工具调用失败 |
| `workflow_completed` | 工作流完成 |
| `workflow_failed` | 工作流失败 |

### 5.5 快照（可选优化）

```sql
CREATE TABLE workflow_snapshots (
    instance_id TEXT PRIMARY KEY REFERENCES workflow_instances(instance_id) ON DELETE CASCADE,
    state       JSONB NOT NULL,
    version     INTEGER NOT NULL DEFAULT 0,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

快照用于避免从头回放全量事件流。`state` 是 `InstanceState` 的序列化 JSON，包含 `status`、`context`、`step_results`、`completed_steps` 等当前状态。

---

## 6. 跨项目关系

### 6.1 tenant_id 是全局逻辑纽带

```
aspectus.tenants.id ───(逻辑)──→ pandaria.sessions.tenant_id
                     ───(逻辑)──→ tavern.workflow_instances.tenant_id
                     ───(逻辑)──→ tavern.agent_definitions.tenant_id
```

**没有数据库级外键约束**。`tenant_id` 的一致性由 Aspectus 的 `/introspect` 端点保证——所有项目的每个请求都先经过 Aspectus 验证 token，返回的 `tenant_id` 是权威值。项目内部不会自创 tenant。

### 6.2 跨项目引用

| 引用方向 | 字段 | 约束 |
|----------|------|------|
| Aspectus → 生态 | `api_keys.project` | Aspectus 的 enum 定义了生态所有项目 |
| Aspectus → 生态 | `scopes.name` = `pandaria:session:create` 等 | Scope 格式 `{project}:{resource}:{action}` |
| Tavern → Pandaria | `workflow_instances` 中步骤执行时调用 Pandaria API | HTTP 调用，不存 DB |
| Pandaria → Aspectus | 每次请求调 `/introspect` | HTTP 调用，tenant_id 存在 sessions 中 |

### 6.3 典型查询：一个租户的全景画像

要查「租户 T 在生态中干了什么」，需要分别查询三个数据库，然后在应用层聚合：

```sql
-- 在 aspectus: 身份信息
SELECT * FROM tenants WHERE id = 'tenant_abc';
SELECT * FROM users WHERE tenant_id = 'tenant_abc';
SELECT * FROM api_keys WHERE tenant_id = 'tenant_abc';
SELECT * FROM audit_logs WHERE tenant_id = 'tenant_abc'
  ORDER BY created_at DESC LIMIT 100;

-- 在 pandaria: session 活动
SELECT * FROM sessions WHERE tenant_id = 'tenant_abc'
  ORDER BY updated_at DESC;
SELECT SUM(output_tokens) FROM session_token_usage
  WHERE tenant_id = 'tenant_abc'
    AND recorded_at >= now() - INTERVAL '30 days';

-- 在 tavern: workflow 执行
SELECT * FROM workflow_instances WHERE tenant_id = 'tenant_abc'
  ORDER BY created_at DESC LIMIT 50;
```

> 未来可在 Constell 的 ClickHouse 中建立物化视图，统一查询租户全景数据。

---

## 7. 部署配置

### 7.1 生产环境

```yaml
# docker-compose 中的 PostgreSQL 初始化
services:
  postgres:
    image: postgres:17
    environment:
      POSTGRES_MULTIPLE_DATABASES: aspectus,pandaria,tavern
    volumes:
      - ./init-multi-db.sh:/docker-entrypoint-initdb.d/init.sh

# 每个服务的连接配置（各自 .env）
# Aspectus/.env
DATABASE_URL=postgres://aspectus_app:xxx@postgres:5432/aspectus

# Pandaria/.env
DATABASE_URL=postgres://pandaria_app:xxx@postgres:5432/pandaria

# Tavern/.env
DATABASE_URL=postgres://tavern_app:xxx@postgres:5432/tavern
```

### 7.2 开发环境

```bash
# 本地开发 — 同一个 PG 实例，三个 database
# docker-env 启动 PG
cd docker-env/compose && docker compose up -d postgres

# 手动创建三个数据库
psql -h localhost -U postgres <<EOF
CREATE DATABASE aspectus;
CREATE DATABASE pandaria;
CREATE DATABASE tavern;
EOF

# 各项目运行 migration
cd Aspectus  && DATABASE_URL=postgres://postgres:postgres@localhost:5432/aspectus sqlx migrate run
cd pandaria && DATABASE_URL=postgres://postgres:postgres@localhost:5432/pandaria cargo run -p storage -- init
cd Tavern   && TAVERN_STORE__DATABASE_URL=postgres://postgres:postgres@localhost:5432/tavern cargo run -p tavern-server
```

### 7.3 权限隔离（即使合 PG 实例也做）

```sql
-- 每个服务有独立的 PG user，只能访问自己的 database
CREATE USER aspectus_app WITH PASSWORD 'xxx';
CREATE USER pandaria_app WITH PASSWORD 'xxx';
CREATE USER tavern_app   WITH PASSWORD 'xxx';

GRANT ALL ON DATABASE aspectus TO aspectus_app;
GRANT ALL ON DATABASE pandaria TO pandaria_app;
GRANT ALL ON DATABASE tavern   TO tavern_app;

-- aspectus_app 看不到 pandaria database 里的任何东西
-- pandaria_app 看不到 tavern database 里的任何东西
```

---

## 8. 命名规范

| 类别 | 规范 | 示例 |
|------|------|------|
| **表名** | 小写 snake_case，复数 | `sessions`、`workflow_events` |
| **索引** | `idx_{table}_{purpose}` | `idx_sessions_tenant`、`idx_events_instance_seq` |
| **约束** | `{table}__{purpose}` | `api_keys__one_owner`、`users__email` |
| **枚举** | 小写 snake_case | `project`、`identity_type` |
| **主键** | `id VARCHAR(21)`（short-id 格式） | `ten_abc123xyz` |
| **时间戳** | `TIMESTAMPTZ`，统一 UTC | `created_at`、`updated_at` |
| **JSON 半结构化** | `JSONB` | `entries`、`payload`、`context` |
| **外键** | `{table}_id` | `tenant_id`、`session_id`、`instance_id` |

---

## 9. 迁移策略

### 9.1 从现状到统一

| 阶段 | Aspectus | Pandaria | Tavern |
|------|----------|----------|--------|
| **现状** | PostgreSQL ✅（aspectus DB） | PostgreSQL（独立 sessions 表） | SQLite/PostgreSQL（独立 3 张表） |
| **Phase 1** | 不动 | 不动 | Tavern 默认 PG，SQLite 仅开发 |
| **Phase 2** | 不动 | sessions 扩展 `status`/`metadata` 字段 | workflow_instances 扩展 `tenant_id`/`inputs`/`context` |
| **Phase 3** | 不动 | `session_token_usage` + `tenant_usage_counters` 表 | `agent_definitions` + `workflow_definitions` 表 |
| **Phase 4** | 生态接入 Aspectus | Pandaria 接入 Aspectus `/introspect` | Tavern 接入 Aspectus `/introspect` |

关键点：**Phase 1-3 是数据库层面的变化，Phase 4 是应用层的接入。可以先做好表结构，再接 Aspectus——不依赖顺序。**

### 9.2 向后兼容

- Pandaria 的 `sessions` 表加入新列（`status`、`metadata`）时使用 `DEFAULT` 值，现有行自动填充
- Tavern 的 `workflow_instances` 加入 `tenant_id` 时，先用 `'default'` 占位（单租户部署），正式接入 Aspectus 后迁移
- Tavern 新增 `agent_definitions` / `workflow_definitions` 表时，保持对 YAML 文件热加载的支持（两种 source 共存）

---

## 10. 不做什么

| ❌ 不做 | 理由 |
|--------|------|
| 跨数据库外键约束 | 服务边界不应由 DB 耦合。tenant_id 的一致性由 Aspectus `/introspect` 保证 |
| 跨库 JOIN | 性能隐患 + 耦合。租户全景查询走 Constell/ClickHouse |
| 为 Pawbun 建表 | 它是 library，无持久化需求 |
| 统一 migration 工具 | 各项目用各自的 sqlx migrate，节奏独立 |
| 共享 user/session 表 | 服务各有自己的数据模型，不应让 Tavern 直接读 Pandaria 的 sessions 表 |

---

## 11. 参考资料

- [Aspectus AGENTS.md](../../Aspectus/AGENTS.md) — 身份模型设计 + ADR
- [Pandaria AGENTS.md](../AGENTS.md) — Session 生命周期 + 持久化设计
- [Tavern AGENTS.md](../../Tavern/AGENTS.md) — 事件溯源 + 工作流编排设计
- [生态系统概览](ecosystem.md) — 项目间依赖关系
- RFC 7662 — OAuth 2.0 Token Introspection（Aspectus 自省端点遵循的语义）

---

*本文档随生态演进持续更新。新增需要持久化的项目或现有项目新增表时，同步更新本文档。*
