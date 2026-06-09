# 第六章：部署与运维

> **目标读者**：想把 Pandaria 生态跑在生产环境的 SRE 和运维工程师。  
> **前提**：了解各项目的职责（第四章）和集成方式（第五章）。

---

## 6.1 全栈 Docker Compose

Pandaria 生态的推荐部署方式。以下是一个生产可用的完整栈：

```yaml
# docker-compose.yml
version: "3.9"

services:
  # ═══════════════════════════════════════
  # Pandaria 及其依赖
  # ═══════════════════════════════════════

  pandaria-postgres:
    image: postgres:17-alpine
    environment:
      POSTGRES_DB: pandaria
      POSTGRES_HOST_AUTH_METHOD: scram-sha-256
      POSTGRES_PASSWORD: ${PANDARIA_PG_PASSWORD}
    volumes:
      - pandaria_pg_data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres -d pandaria"]
      interval: 5s
      timeout: 3s
      retries: 5

  pandaria-redis:
    image: redis:7.2-alpine
    command: redis-server --appendonly yes
    volumes:
      - pandaria_redis_data:/data
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 5s
      timeout: 3s
      retries: 5

  pandaria:
    build:
      context: ./pandaria
      dockerfile: Dockerfile
    depends_on:
      pandaria-postgres:
        condition: service_healthy
      pandaria-redis:
        condition: service_healthy
    environment:
      - DATABASE_URL=postgres://postgres:${PANDARIA_PG_PASSWORD}@pandaria-postgres:5432/pandaria
      - REDIS_URL=redis://pandaria-redis:6379
      - PANDARIA_EMERALD_URL=http://emerald:8000
      - PANDARIA_CONSTELL_URL=http://constell-web:3000
      - RUST_LOG=info,pandaria=debug
    ports:
      - "8080:8080"
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
      interval: 10s
      timeout: 5s
      retries: 3
    restart: unless-stopped

  # ═══════════════════════════════════════
  # Emerald — 记忆系统
  # ═══════════════════════════════════════

  emerald:
    build:
      context: ./Emerald
      dockerfile: Dockerfile
    environment:
      - EMERALD_DATABASE_URL=postgresql://...    # Emerald 自己的 PG/图数据库
      - EMERALD_VECTOR_STORE_URL=...             # 向量存储
      - EMERALD_S3_ENDPOINT=...                  # 对象存储
      - LOG_LEVEL=info
    ports:
      - "8000:8000"
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8000/health"]
      interval: 10s
      timeout: 5s
      retries: 3
    restart: unless-stopped
    # Emerald 可能有自己的 PG/Redis/S3 依赖，按需添加

  # ═══════════════════════════════════════
  # Tavern — 编排框架
  # ═══════════════════════════════════════

  tavern:
    build:
      context: ./Tavern
      dockerfile: Dockerfile
    environment:
      - RUNTIME_URL=http://pandaria:8080
      - PANDARIA_AUTH_SECRET=${TAVERN_AUTH_SECRET}
      - PANDARIA_TENANT_ID=${TAVERN_TENANT_ID}
      - SERVER_HOST=0.0.0.0
      - SERVER_PORT=3000
      - AGENT_CONFIG_DIR=/configs/agents
      - WORKFLOW_CONFIG_DIR=/configs/workflows
      - RUST_LOG=info,tavern=debug
    volumes:
      - ./configs/agents:/configs/agents:ro
      - ./configs/workflows:/configs/workflows:ro
      - tavern_data:/data     # SQLite EventStore
    ports:
      - "3001:3000"
    depends_on:
      pandaria:
        condition: service_healthy
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/health"]
      interval: 10s
      timeout: 5s
      retries: 3
    restart: unless-stopped

  # ═══════════════════════════════════════
  # Constell — 可观测性平台（完整栈）
  # ═══════════════════════════════════════

  constell-postgres:
    image: postgres:17-alpine
    environment:
      POSTGRES_DB: constell
      POSTGRES_HOST_AUTH_METHOD: scram-sha-256
      POSTGRES_PASSWORD: ${CONSTELL_PG_PASSWORD}
    volumes:
      - constell_pg_data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres -d constell"]
      interval: 5s
      timeout: 3s
      retries: 5

  constell-clickhouse:
    image: clickhouse/clickhouse-server:25-alpine
    environment:
      CLICKHOUSE_DB: constell
      CLICKHOUSE_USER: default
      CLICKHOUSE_PASSWORD: ${CONSTELL_CH_PASSWORD}
    volumes:
      - constell_ch_data:/var/lib/clickhouse
    healthcheck:
      test: ["CMD", "clickhouse-client", "--query", "SELECT 1"]
      interval: 5s
      timeout: 3s
      retries: 5

  constell-redis:
    image: redis:7.2-alpine
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 5s
      timeout: 3s
      retries: 5

  constell-minio:
    image: minio/minio:latest
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: ${CONSTELL_MINIO_USER}
      MINIO_ROOT_PASSWORD: ${CONSTELL_MINIO_PASSWORD}
    volumes:
      - constell_minio_data:/data
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:9000/minio/health/live"]
      interval: 10s
      timeout: 5s
      retries: 3

  constell-web:
    build:
      context: ./Constell
      dockerfile: Dockerfile
    depends_on:
      constell-postgres:
        condition: service_healthy
      constell-clickhouse:
        condition: service_healthy
      constell-redis:
        condition: service_healthy
    environment:
      - DATABASE_URL=postgres://postgres:${CONSTELL_PG_PASSWORD}@constell-postgres:5432/constell
      - CLICKHOUSE_URL=http://constell-clickhouse:8123
      - REDIS_HOST=constell-redis
      - REDIS_PORT=6379
      - CONSTELL_S3_EVENT_UPLOAD_ENDPOINT=http://constell-minio:9000
      - CONSTELL_S3_EVENT_UPLOAD_ACCESS_KEY_ID=${CONSTELL_MINIO_USER}
      - CONSTELL_S3_EVENT_UPLOAD_SECRET_ACCESS_KEY=${CONSTELL_MINIO_PASSWORD}
    ports:
      - "3000:3000"
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/api/health"]
      interval: 10s
      timeout: 5s
      retries: 3

  constell-worker:
    build:
      context: ./Constell
      dockerfile: worker/Dockerfile
    depends_on:
      constell-postgres:
        condition: service_healthy
      constell-clickhouse:
        condition: service_healthy
      constell-redis:
        condition: service_healthy
    environment:
      - DATABASE_URL=postgres://postgres:${CONSTELL_PG_PASSWORD}@constell-postgres:5432/constell
      - CLICKHOUSE_URL=http://constell-clickhouse:8123
      - REDIS_HOST=constell-redis
      - REDIS_PORT=6379
    restart: unless-stopped

volumes:
  pandaria_pg_data:
  pandaria_redis_data:
  tavern_data:
  constell_pg_data:
  constell_ch_data:
  constell_minio_data:
```

### 6.1.1 环境变量

```bash
# .env（不要提交到 git）

# Pandaria
PANDARIA_PG_PASSWORD=secure-password-here

# Tavern
TAVERN_AUTH_SECRET=32-byte-secret-for-hmac
TAVERN_TENANT_ID=my-organization

# Constell
CONSTELL_PG_PASSWORD=secure-password-here
CONSTELL_CH_PASSWORD=clickhouse-password
CONSTELL_MINIO_USER=minioadmin
CONSTELL_MINIO_PASSWORD=minioadmin
```

### 6.1.2 启动

```bash
# 首次启动（含数据库初始化）
docker compose up -d --wait

# 验证全部健康
docker compose ps
# 所有服务应显示 healthy

# 查看日志
docker compose logs -f pandaria
```

---

## 6.2 端口规划

| 服务 | 内部端口 | 外部端口 | 用途 |
|------|:---:|:---:|------|
| pandaria (api-gateway) | 8080 | 8080 | REST + SSE API |
| emerald | 8000 | 8000 | Memory REST API |
| tavern | 3000 | 3001 | Workflow REST API |
| constell-web | 3000 | 3000 | 可观测性 Web UI + API |
| constell-clickhouse | 8123 | 8123 | ClickHouse HTTP 接口 |
| constell-minio | 9000 | — | S3 兼容存储（内部） |
| constell-minio-console | 9001 | 9001 | MinIO 控制台 |

---

## 6.3 健康检查矩阵

| 服务 | 健康检查端点 | 依赖 |
|------|------------|------|
| pandaria | `GET /health` | PostgreSQL + Redis |
| emerald | `GET /health` | PostgreSQL + 向量存储 |
| tavern | `GET /health` | Pandaria |
| constell-web | `GET /api/health` | PostgreSQL + ClickHouse + Redis |

**告警规则**：
- 任一服务 health check 连续失败 3 次 → 告警
- Pandaria session 创建失败率 > 1% → 告警
- Emerald recall 超时率 > 5% → 告警
- Tavern workflow 执行失败率 > 2% → 告警

---

## 6.4 日志

所有项目使用结构化日志：

| 项目 | 日志框架 | 格式 | 级别控制 |
|------|---------|------|---------|
| Pandaria | `tokio-tracing` | JSON / 文本 | `RUST_LOG` |
| Emerald | Python `logging` | JSON | `LOG_LEVEL` |
| Tavern | `tokio-tracing` | JSON / 文本 | `RUST_LOG` |
| Constell | `pino` / winston | JSON | `LOG_LEVEL` |

**日志收集**：推荐通过 `docker compose logs` 转发到 Loki / ELK / Datadog。

**关键日志字段**：所有 Pandaria/Tavern 日志携带 `tenant_id` 和 `session_id`，支持按租户/会话过滤。

**敏感数据**：LLM API Key 不出现于任何日志。PII 通过 ContentFilter hook 脱敏。

---

## 6.5 资源规划

### 6.5.1 最小配置（开发环境）

| 服务 | CPU | 内存 | 磁盘 |
|------|:--:|:---:|------|
| pandaria | 1 | 512MB | — |
| pandaria-postgres | 0.5 | 256MB | 10GB |
| pandaria-redis | 0.25 | 128MB | 1GB |
| emerald | 1 | 1GB | — |
| tavern | 0.5 | 256MB | 1GB (EventStore) |
| constell-web | 0.5 | 512MB | — |
| constell-worker | 0.5 | 512MB | — |
| constell-postgres | 0.5 | 256MB | 20GB |
| constell-clickhouse | 1 | 2GB | 50GB |
| constell-redis | 0.25 | 256MB | 2GB |
| constell-minio | 0.25 | 256MB | 20GB |

### 6.5.2 生产配置建议

| 服务 | 建议 |
|------|------|
| Pandaria | 水平扩展（多实例 + 共享 PG/Redis）。per-instance 2 CPU / 1GB |
| Tavern | 水平扩展。EventStore 用 PostgreSQL（非 SQLite） |
| Emerald | 垂直扩展（图数据库内存需求高）。建议 4GB+ |
| Constell ClickHouse | 独立 SSD。写入密集型 |
| Constell PostgreSQL | 连接池（pgbouncer）。建议 4GB+ |

---

## 6.6 备份策略

| 数据 | 存储 | 备份方式 | 频率 |
|------|------|---------|------|
| Session 状态 | Pandaria PostgreSQL | `pg_dump` / WAL 归档 | 实时 + 每日全量 |
| Session 缓存 | Pandaria Redis | RDB + AOF | 持久化 + 每日备份 RDB |
| EventStore | Tavern PostgreSQL | `pg_dump` / WAL 归档 | 实时 + 每日全量 |
| Emerald 图谱 | Emerald 图数据库 | 数据库原生备份 | 每日 |
| Constell 元数据 | Constell PostgreSQL | `pg_dump` / WAL 归档 | 实时 + 每日全量 |
| Constell 事件 | Constell ClickHouse | `clickhouse-backup` | 每日 |
| Constell Blob | Constell MinIO | `mc mirror` | 每日 |

---

## 6.7 版本兼容性

部署时必须确保各项目版本匹配。参考兼容性矩阵：

| Pandaria | Emerald | Pawbun | Tavern | Constell | 状态 |
|:---:|:---:|:---:|:---:|:---:|:---:|
| 0.2.x | 0.2.0 | 0.2.x | 0.2.x - 0.3.x | 0.3.x | ✅ current |
| 0.3.x | 0.2.0 | 0.2.x | 0.4.x | 0.3.x | 🎯 target |
| 0.3.x | 0.3.x | 0.3.x | 0.4.x | 0.5.x | 📋 planned |

详见 [兼容性矩阵 Spec](../specs/2026-05-28-ecosystem-integration-deepening.md#4-版本兼容性矩阵)。

---

## 6.8 升级流程

```
1. 备份全部数据库
2. 在 staging 环境验证新版本兼容性
3. 按依赖顺序升级：
   a. Emerald（记忆系统，无下游依赖）
   b. Constell（可观测性，无下游依赖）
   c. Pawbun（库，通过 Pandaria 重新编译）
   d. Pandaria（核心运行时）
   e. Tavern（编排，依赖 Pandaria）
4. 每个服务升级后验证 health check + smoke test
5. 若任一步失败 → 回滚到上一个版本
```

---

## 6.9 监控指标

### Pandaria 关键指标

| 指标 | 说明 | 告警阈值 |
|------|------|:---:|
| `session_create_rate` | Session 创建速率 | — |
| `session_active_count` | 活跃 session 数 | > 租户配额 90% |
| `turn_duration_p95` | Turn 延迟 P95 | > 30s |
| `tool_call_error_rate` | 工具调用错误率 | > 5% |
| `llm_error_rate` | LLM 调用错误率 | > 2% |
| `compaction_trigger_rate` | 压缩触发频率 | — |

### Tavern 关键指标

| 指标 | 说明 | 告警阈值 |
|------|------|:---:|
| `workflow_execution_rate` | Workflow 执行速率 | — |
| `workflow_failure_rate` | Workflow 失败率 | > 2% |
| `step_retry_rate` | Step 重试率 | > 10% |
| `execution_duration_p95` | 执行时长 P95 | > 5min |

### Emerald 关键指标

| 指标 | 说明 | 告警阈值 |
|------|------|:---:|
| `memory_ingestion_rate` | 记忆摄入速率 | — |
| `extraction_latency_p95` | 提取延迟 P95 | > 5s |
| `search_latency_p95` | 搜索延迟 P95 | > 500ms |
| `profile_fetch_latency_p95` | 画像获取延迟 P95 | > 100ms |

---

## 6.10 故障恢复

| 故障 | 恢复方式 |
|------|---------|
| Pandaria 进程崩溃 | Docker restart policy (`unless-stopped`)，session 从 PostgreSQL 自动恢复 |
| PostgreSQL 宕机 | 主从切换 / 从备份恢复。Pandaria 自动重连 |
| Redis 宕机 | AOF/RDB 恢复。Pandaria 降级为无缓存模式 |
| Emerald 不可达 | Pandaria `remember` 失败静默（warn log），`recall` 超时返回空结果 |
| Constell 不可达 | Reporter 丢弃事件（warn log），不影响 agent 正常运行 |
| Tavern 崩溃 | EventStore 恢复未完成的 execution 状态（checkpoint recovery） |

---

## 6.11 下一步

- 完整的集成深化方案 → [生态集成 Spec](../specs/2026-05-28-ecosystem-integration-deepening.md)
- 生态项目概览 → [第四章：生态项目概览](./04-ecosystem.md)
- 集成指南 → [第五章：集成指南](./05-integration.md)
