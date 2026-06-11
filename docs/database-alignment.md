# Pandaria — 数据库对齐方案

> 参见主设计文档：[docs/database-design.md](database-design.md)

## 定位

Pandaria 的数据库 (`pandaria`) 负责 Agent Runtime 的状态持久化——session 会话、token 消耗计量、租户资源用量计数。

## 当前状态

| 表 | 当前 | 目标 | 变更 |
|----|:--:|:--:|------|
| `sessions` | ✅ 已有 | 扩展两个字段 | `ADD COLUMN` |
| `session_token_usage` | ❌ 无 | 新增 | `CREATE TABLE` |
| `tenant_usage_counters` | ❌ 无 | 新增 | `CREATE TABLE` |

### sessions 表 — 当前 DDL

```sql
-- 当前 storage/src/session/postgres.rs 中的 init() 创建
CREATE TABLE IF NOT EXISTS sessions (
    tenant_id   TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    entries     JSONB NOT NULL DEFAULT '[]',
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, session_id)
);
```

### sessions 表 — 目标 DDL（变化部分）

```sql
-- 新增两个字段
ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS status VARCHAR(16) NOT NULL DEFAULT 'active';

ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}';

-- 新增索引
CREATE INDEX IF NOT EXISTS idx_sessions_status
    ON sessions (tenant_id, status);
```

**status 取值**：`active` | `completed` | `aborted`

**metadata 示例**：
```json
{
  "model": "claude-sonnet-4-5-20250929",
  "provider": "anthropic",
  "system_prompt_hash": "sha256:abc123",
  "hook_config": "default",
  "tools_count": 12,
  "workspace_root": "/workspaces/tenant_abc"
}
```

## 需要新增的表

### session_token_usage

```sql
CREATE TABLE IF NOT EXISTS session_token_usage (
    id             BIGSERIAL PRIMARY KEY,
    tenant_id      TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    turn_number    INTEGER NOT NULL,
    input_tokens   BIGINT NOT NULL DEFAULT 0,
    output_tokens  BIGINT NOT NULL DEFAULT 0,
    model          VARCHAR(128),
    provider       VARCHAR(64),
    recorded_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_token_usage_tenant
    ON session_token_usage (tenant_id, recorded_at DESC);
CREATE INDEX IF NOT EXISTS idx_token_usage_session
    ON session_token_usage (tenant_id, session_id);
```

> 替代当前 `tenant::TenantTokenMeterExtension` 中的内存滑窗计数器，提供持久化的 token 消耗明细。

### tenant_usage_counters

```sql
CREATE TABLE IF NOT EXISTS tenant_usage_counters (
    tenant_id    TEXT NOT NULL,
    metric       VARCHAR(64) NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    value        BIGINT NOT NULL DEFAULT 0,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, metric, window_start)
);
```

**metric 预定义值**：
- `monthly_tokens` — 当月 token 总消耗
- `concurrent_sessions` — 当前活跃 session 数
- `daily_tool_calls` — 当日工具调用次数

## Redis 不变

Redis 键命名与 SessionStore 保持现有设计：

```
pandaria:session:{tenant_id}:{session_id}   → JSON (TTL 7天)
pandaria:tenant:{tenant_id}:sessions        → SET of session_id
```

Redis 仍然是**缓存层**，PG `sessions` 表是唯一真相源。

## 代码变更点

| 文件 | 变更 |
|------|------|
| `crates/storage/src/session/postgres.rs` | `init()` 中创建新表和索引；`save_session()` 写入 `status`/`metadata` |
| `crates/storage/src/session/mod.rs` | `SessionStore` trait 不变 |
| `crates/agent-core/src/harness/session.rs` | `SessionActor` 写入 `metadata` 和状态变更时更新 `status` |
| `crates/tenant/` | `TenantTokenMeterExtension` 或新增模块写入 `session_token_usage` / `tenant_usage_counters` |

## 数据库连接

```bash
# 生产环境
DATABASE_URL=postgres://pandaria_app:xxx@postgres:5432/pandaria

# 开发环境
DATABASE_URL=postgres://postgres:postgres@localhost:5432/pandaria
```

## 下一步

- [ ] `ALTER TABLE sessions ADD COLUMN status, metadata`（无锁，`DEFAULT` 保证现有行兼容）
- [ ] 新建 `session_token_usage` 和 `tenant_usage_counters` 表
- [ ] `PgSessionStore::init()` 更新为创建所有表
- [ ] `SessionActor` 写入 `status` 值（start → active, end → completed/aborted）
- [ ] 接入 Aspectus `/introspect` 验证 token（替换 HMAC token，使用 Aspectus 返回的 `tenant_id`）
