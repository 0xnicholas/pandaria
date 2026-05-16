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
api-gateway → tenant → extensions → agent-core → ai-provider
```

api-gateway **禁止**直接依赖 `ai-provider`。所有 LLM 相关类型通过 `agent-core` re-export 或内部重新定义。

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

## 测试

```bash
cargo test -p api-gateway
```
