# api-gateway

pandaria 服务端 HTTP 入口层。

## 职责

- 认证（Bearer token HMAC-SHA256）
- 路由（REST API）
- SSE 事件流转发
- 限流（per-tenant token bucket）

## 不负责

- Session 生命周期管理
- Agent loop 执行
- 租户调度

以上由 `tenant` / `agent-core` crate 负责。

## 依赖方向

```
api-gateway → tavern-comp → agent-core → pawbun-toolkit → pawbun-files
    │              │              │
    │         tavern-core    ai-provider
    │
    └── tenant → storage
```

api-gateway **禁止**直接依赖 `ai-provider`。所有 LLM 相关类型通过 `agent-core` re-export 或内部重新定义。

api-gateway 通过 `tavern-comp` 接入工作流编排能力，通过 `pawbun-toolkit` 接入工具抽象。

## API 端点

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/healthz` | 健康检查 |
| POST | `/api/v1/sessions` | 创建 session |
| GET | `/api/v1/sessions` | 列出 session |
| GET | `/api/v1/sessions/{id}` | 获取 session 元数据 |
| PATCH | `/api/v1/sessions/{id}` | 更新 session 元数据 |
| DELETE | `/api/v1/sessions/{id}` | 删除 session |
| POST | `/api/v1/sessions/{id}/messages` | 发送消息 |
| DELETE | `/api/v1/sessions/{id}/messages/current` | 中断当前 turn |
| GET | `/api/v1/sessions/{id}/events` | SSE 事件流 |
| POST | `/api/v1/sessions/{id}/compact` | 触发上下文压缩 |
| GET | `/api/v1/sessions/{id}/messages` | 获取历史消息 |

## 运行

```bash
cargo run -p api-gateway
```

环境变量：
- `PANDARIA_BIND_ADDR` — 绑定地址（默认 `0.0.0.0:8080`）
- `PANDARIA_AUTH_SECRET` — HMAC 签名密钥（生产环境必须设置）
- `PANDARIA_SSRF_ALLOWLIST` — SSRF 防护的 allowlist（详见下文）
- `ASPECTUS_BASE_URL` — Aspectus 服务地址（默认 `http://localhost:3100`）
- `ASPECTUS_SERVICE_TOKEN` — Aspectus 服务 token（**必填**）
- `ASPECTUS_TIMEOUT_MS` — Aspectus introspection 超时（默认 2000ms）

## SSRF 防护（`HttpProxyTool` / Webhook 投递）

`HttpProxyTool` 和 webhook 投递路径默认**严格拒绝**所有内网地址：
- IPv4 私有段：`127.0.0.0/8`、`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`、`169.254.0.0/16`、`0.0.0.0/8`
- IPv6：`::1`、`fc00::/7`、`fe80::/10`
- `localhost`（任意大小写）
- 非 `http`/`https` scheme
- 解析失败的 URL

### 允许特定内网目标（用于服务间集成，如 pandaria ↔ DayPaw）

设置 `PANDARIA_SSRF_ALLOWLIST`：

```bash
export PANDARIA_SSRF_ALLOWLIST=10.0.0.0/8,192.168.1.0/24,daypaw.internal,localhost
```

格式：
- IPv4 CIDR: `a.b.c.d/n`（`0 <= n <= 32`）
- IPv6 CIDR: `ipv6/n`（`0 <= n <= 128`）
- 域名后缀: `daypaw.internal`（匹配 `api.daypaw.internal`）
- 单标签 hostname: `localhost`、`internal` 等
- 多条以逗号分隔，前后空白会被去除

### 行为

- allowlist 中的条目**显式放行**，覆盖默认严格 deny
- 单个 entry 解析失败 → `tracing::warn!` 并跳过，其他合法 entry 继续生效
- allowlist env var 已设置但**全部 entry 解析失败** → **服务 panic 拒绝启动**（避免带病运行）
- 不设置 → strict policy（所有内网被拒）

### ⚠️ 安全警告

错误的 allowlist 配置会让恶意 agent 通过工具调用访问内网服务（数据库、Redis、云 metadata 服务等）。建议：

1. 使用**最窄**的 CIDR（如 `10.1.2.0/24` 而非 `10.0.0.0/8`）
2. **永远不要**在 allowlist 中包含 `169.254.0.0/16`（云 metadata 服务 `169.254.169.254`）
3. 配合网络层 ACL（k8s NetworkPolicy、安全组）
4. 监控 `HttpProxyTool` 调用日志，定期审计异常流量

## 测试

```bash
cargo test -p api-gateway
```
