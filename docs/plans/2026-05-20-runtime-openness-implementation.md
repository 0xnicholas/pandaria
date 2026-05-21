# Runtime Openness 实施计划

**Date:** 2026-05-20
**Status:** Draft
**Reference:** `docs/specs/2026-05-20-runtime-openness.md`

---

## 1. 概述

本计划将 Runtime Openness spec 的 6 个 Phase 展开为可执行的开发任务，每个 Phase 包含具体步骤、文件清单、接口变更、测试策略和验收标准。

**实施原则**：
- 每 Phase 独立可交付，编译通过、测试通过后再进入下一 Phase
- 优先保证 agent-core 的稳定性，gateway 层变更后置
- 所有新增公开 API 必须同步更新 OpenAPI 文档

---

## 2. Phase 1：HttpProxyTool + 动态工具注册

### 2.1 目标
实现外部 HTTP 工具代理，允许创建 session 时通过 API 注入自定义工具集。

### 2.2 涉及文件

| 文件 | 操作 |
|---|---|
| `crates/agent-core/src/tools/http_proxy.rs` | **新增** |
| `crates/agent-core/src/tools/mod.rs` | 修改（导出 `ToolConfig`、`HttpProxyTool`） |
| `crates/agent-core/src/error.rs` | 修改（新增 `ToolEndpointForbidden`、`ToolEndpointInvalid`） |
| `crates/tenant/src/manager.rs` | 修改（`CreateSessionParams` 新增 `tools`，`create_session()` 注入逻辑） |
| `crates/api-gateway/src/types.rs` | 修改（`CreateSessionRequest` 新增 `tools`） |
| `crates/api-gateway/src/routes/sessions.rs` | 修改（handler 转换 `tools` 字段） |

### 2.3 具体步骤

#### Step 1.1：SSRF 防护工具函数
在 `agent-core/src/tools/http_proxy.rs` 中先实现 `is_internal_endpoint(url: &str) -> bool`：
- 解析 URL，提取 host
- 检查 IP 段（127/10/172.16-31/192.168）
- 检查 `169.254.169.254`
- 检查 `localhost` 域名
- 返回 bool，命中则拦截

#### Step 1.2：HttpProxyTool 实现
```rust
impl AgentTool for HttpProxyTool {
    fn name(&self) -> &str { &self.config.name }
    fn description(&self) -> &str { &self.config.description }
    fn parameters(&self) -> serde_json::Value { self.config.parameters.clone() }
    
    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        _on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        signal: CancellationToken,
    ) -> Result<AgentToolResult, AgentError> {
        // 1. SSRF 检查
        // 2. 组装 POST body
        // 3. 带 timeout + cancel 的 HTTP 请求
        // 4. 解析响应 → AgentToolResult
    }
}
```

**注意**：`reqwest::Client` 建议使用 `api-gateway` 统一注入或复用全局 Client，避免每个 `HttpProxyTool` 实例自建连接池。可在 `SessionConfig` 中增加 `http_client: Option<reqwest::Client>`，由 `TenantManagerImpl` 统一传入。

#### Step 1.3：类型传播
- `agent-core/src/tools/http_proxy.rs` 导出 `ToolConfig`
- `CreateSessionParams` 新增 `tools: Vec<ToolConfig>`
- `SessionConfig` 已存在 `tools: Vec<AgentToolRef>`，无需新增字段。`HttpProxyTool` 实例在 `TenantManagerImpl::create_session()` 中追加到该列表
- **`CreateSessionRequest` 独立声明 `tools` 字段**（Spec 3.1.3 要求 gateway 不直接复用 `ToolConfig`，避免 crate 间类型泄漏到 API 契约层）。在 `api-gateway/src/types.rs` 中定义：
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct CreateSessionRequest {
      pub title: Option<String>,
      #[serde(default)]
      pub system_prompt: Option<String>,
      #[serde(default)]
      pub tools: Vec<ToolConfig>, // 独立声明，语义与 agent-core::ToolConfig 一致但解耦
  }
  ```
  Gateway handler 中直接透传 `req.tools` 到 `tenant::CreateSessionParams`（因为 `tenant` 已依赖 `agent-core`，`ToolConfig` 在 tenant 层可用）。

#### Step 1.4：TenantManagerImpl 注入逻辑
在 `create_session()` 中：
```rust
let mut tools: Vec<Arc<dyn AgentTool>> = vec![];
// 现有 MediaGenerationTool 注入...
for tool_config in params.tools {
    tools.push(Arc::new(HttpProxyTool::new(
        tool_config,
        tenant_id.to_string(),
        session_id.to_string(),
        http_client.clone(),
    )));
}
```

#### Step 1.5：Gateway Handler 转换
在 `api-gateway/src/routes/sessions.rs` 的 `create_session` handler 中：
```rust
let params = tenant::CreateSessionParams {
    title: req.title,
    system_prompt: req.system_prompt,
    tools: req.tools, // 直接透传
    webhook: None, // Phase 3 再处理
};
```

**HTTP 状态码映射**（对应 Spec 6.1）：
- `ToolConfig.endpoint` 不是合法 URL → `TenantError::BadRequest("tool_endpoint_invalid")` → HTTP 400
- `ToolConfig.endpoint` 命中 SSRF 黑名单 → `TenantError::BadRequest("tool_endpoint_forbidden")` → HTTP 400
建议在 `TenantManagerImpl::create_session()` 中注入 `HttpProxyTool` 前先做 URL 校验和 SSRF 检查，失败则提前返回。

### 2.4 测试策略

| 测试类型 | 内容 |
|---|---|
| 单元测试 | `http_proxy.rs`：SSRF 用例（内网 IP、localhost、合法公网 URL）、响应解析、超时取消 |
| 集成测试 | `api-gateway/tests/e2e`：通过 `CreateSessionRequest.tools` 创建带外部工具的 session，发送消息触发 tool_call，验证 HTTP 转发和响应回传 |
| Mock 工具端点 | E2E 测试中使用 `wiremock` 或本地 `tokio::net::TcpListener` 模拟外部工具服务 |

### 2.5 验收标准
- [ ] `cargo test -p agent-core` 通过（含新增 SSRF + HttpProxyTool 测试）
- [ ] `cargo test -p api-gateway --test e2e_tool_use_http` 通过（创建 session + 外部工具调用闭环）
- [ ] SSRF 命中时返回 `is_error=true`，不发起实际 HTTP 请求
- [ ] 不破坏现有 `MediaGenerationTool` 的注入逻辑

---

## 3. Phase 2：Session 状态机 + `/state` 端点

### 3.1 目标
用显式状态机替代 `is_streaming: bool`，暴露 `GET /sessions/{id}/state`。

### 3.2 涉及文件

| 文件 | 操作 |
|---|---|
| `crates/agent-core/src/harness/session.rs` | 修改（`is_streaming` → `state: AtomicU8`，新增 `reset()`） |
| `crates/agent-core/src/error.rs` | 修改（新增 `SessionInError { reason: String }`） |
| `crates/agent-core/src/harness/mod.rs` | 修改（导出 `SessionState`） |
| `crates/tenant/src/manager.rs` | 修改（`get_session()` 读取状态，`map_agent_error()` 映射新错误码） |
| `crates/tenant/src/session_entry.rs` | 修改（`ActiveSession` 暴露状态查询方法） |
| `crates/api-gateway/src/types.rs` | 修改（新增 `SessionStateResponse`） |
| `crates/api-gateway/src/routes/sessions.rs` | 修改（新增 `GET /sessions/{id}/state` handler） |

### 3.3 具体步骤

#### Step 2.1：SessionState enum
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Idle,
    Running,
    Error,
}
```
放在 `agent-core/src/harness/session.rs`，与 `SessionActor` 同文件。

#### Step 2.2：替换 is_streaming
```rust
// 删除
is_streaming: bool,

// 新增
state: std::sync::atomic::AtomicU8, // 0=Idle, 1=Running, 2=Error
error_reason: std::sync::Mutex<Option<String>>,
```

`is_streaming()` 改为：
```rust
pub fn is_streaming(&self) -> bool {
    self.state.load(Ordering::SeqCst) == 1
}
```

新增：
```rust
pub fn state(&self) -> SessionState {
    match self.state.load(Ordering::SeqCst) {
        1 => SessionState::Running,
        2 => SessionState::Error,
        _ => SessionState::Idle,
    }
}
```

#### Step 2.3：run_with_messages() 状态转换
在入口：
```rust
self.state.store(1, Ordering::SeqCst); // Running
```

在成功出口：
```rust
self.state.store(0, Ordering::SeqCst); // Idle
self.error_reason.lock().unwrap().take();
```

在错误出口：
```rust
self.state.store(2, Ordering::SeqCst); // Error
*self.error_reason.lock().unwrap() = Some(reason);
```

在 abort 出口（catch Cancelled）：
```rust
self.state.store(0, Ordering::SeqCst); // Idle
```

#### Step 2.4：Error 状态拦截
在 `prompt_with_content()`、`prompt()`、`continue_()` 入口：
```rust
if self.state.load(Ordering::SeqCst) == 2 {
    let reason = self.error_reason.lock().unwrap().clone().unwrap_or_default();
    return Err(AgentError::SessionInError { reason });
}
```

#### Step 2.5：reset() 实现
```rust
pub async fn reset(&mut self) -> Result<(), AgentError> {
    self.abort_token.cancel(); // 中断当前 turn
    self.entries.clear();
    self.recovery = RecoveryStateMachine::new(self.max_retries);
    self.state.store(0, Ordering::SeqCst);
    self.error_reason.lock().unwrap().take();
    self.abort_token = CancellationToken::new();
    Ok(())
}
```

#### Step 2.6：TenantManagerImpl 状态暴露
在 `ActiveSession` 上新增：
```rust
pub fn state(&self) -> SessionState {
    self.actor.blocking_lock().state() // 不行，async context
}
```

**问题**：`ActiveSession` 的 `actor` 是 `Arc<Mutex<SessionActor>>`，状态查询需要 `lock().await`，但 `get_session()` 是同步的 `DashMap` 读取。

**解决方案**：在 `ActiveSession` 上维护一个与 `SessionActor` 同步的 `AtomicU8`：
- `TenantManagerImpl::send_message()` 在调用 `prompt_with_content()` 前后更新 `ActiveSession.state`
- 但这会引入状态同步问题

**更好的方案**：`SessionActor.state` 本身就是 `AtomicU8`，让 `ActiveSession` 持有 `SessionActor` 的弱引用或直接暴露 `state()` 的异步方法。但 `get_session()` 返回 `SessionInfo`，需要同步填充状态。

**最终方案**：Spec 定义的是独立 `GET /sessions/{id}/state` 端点，返回 `{ state, error_reason }`。在 `TenantManager` trait 新增：
```rust
async fn get_session_state(
    &self,
    tenant_id: &str,
    session_id: &Uuid,
) -> Result<(SessionState, Option<String>), TenantError>;
```

实现：
```rust
async fn get_session_state(&self, tenant_id: &str, session_id: &Uuid) -> Result<(SessionState, Option<String>), TenantError> {
    let entry = self.sessions.get(&(tenant_id.to_string(), *session_id))
        .ok_or_else(|| TenantError::SessionNotFound(...))?;
    let actor = entry.actor.lock().await;
    Ok((actor.state(), actor.error_reason()))
}
```

其中 `SessionActor::error_reason()` 返回 `error_reason.lock().unwrap().clone()`。这样不需要改 `SessionInfo`，状态查询走独立方法。

### 3.4 测试策略

| 测试类型 | 内容 |
|---|---|
| 单元测试 | `session.rs`：状态转换矩阵（Idle→Running→Idle、Running→Error→Idle via reset、abort 回 Idle、Error 状态下 prompt 返回 SessionInError） |
| 单元测试 | `reset()` 后 entries 为空、state 为 Idle、abort_token 有效 |
| E2E 测试 | `GET /sessions/{id}/state` 在 turn 前返回 idle、turn 中返回 running、turn 后返回 idle |

### 3.5 验收标准
- [ ] `cargo test -p agent-core` 通过（状态机 + reset 测试）
- [ ] `cargo test -p api-gateway --test e2e_sse_events` 仍通过（`is_streaming()` 语义不变）
- [ ] `GET /sessions/{id}/state` 返回正确 JSON
- [ ] Error 状态下 `POST /sessions/{id}/messages` 返回 409

---

## 4. Phase 3：Webhook 事件推送

### 4.1 目标
实现 Webhook 事件推送，替代编排器维持 N 个 SSE 长连接。

### 4.2 涉及文件

| 文件 | 操作 |
|---|---|
| `crates/tenant/src/manager.rs` | 修改（`CreateSessionParams` 新增 `webhook`，`ActiveSession` 保存 `WebhookConfig`） |
| `crates/tenant/src/session_entry.rs` | 修改（`ActiveSession` 新增 `webhook` 字段） |
| `crates/tenant/src/events.rs` | 修改（新增 `WebhookEventListener`） |
| `crates/api-gateway/src/types.rs` | 修改（`CreateSessionRequest` 新增 `webhook`） |
| `crates/api-gateway/src/routes/sessions.rs` | 修改（handler 透传 `webhook`） |

### 4.3 具体步骤

#### Step 3.1：WebhookEventListener 结构

**前置安全校验**（对应 Spec 5.2）：在 `TenantManagerImpl::create_session()` 中，若 `webhook_config.url` 不为空，先执行 SSRF 检查（复用 Phase 1 的 `is_internal_endpoint()` 或独立实现），命中内网地址则返回 `TenantError::BadRequest("webhook_url_forbidden")` → HTTP 400。

WebhookEventListener 结构：
```rust
pub struct WebhookEventListener {
    config: WebhookConfig,
    tenant_id: String,
    session_id: String,
    client: reqwest::Client,
    delivery_queue: tokio::sync::mpsc::Sender<DeliveryJob>,
}

struct DeliveryJob {
    event: ServerEvent,
    delivery_id: Uuid,
}
```

内部实现：
- `mpsc` 队列消费 + 限流（Semaphore(5)）
- 指数退避重试：1s → 2s → 4s
- 连续失败 10 次后标记 `disabled`
- HMAC-SHA256 签名

#### Step 3.2：AgentEvent → ServerEvent 映射
Webhook 发送 `ServerEvent`，需要转换函数。首先，在两个位置新增 `StateChanged` variant：

**1. `agent-core/src/events.rs`**（新增 `AgentEvent::StateChanged`）：
```rust
pub enum AgentEvent {
    // ... existing variants ...
    StateChanged {
        state: SessionState,
    },
}
```

在 `SessionActor::run_with_messages()` 的状态转换点 emit：
```rust
// Idle → Running
self.emit_event(AgentEvent::StateChanged { state: SessionState::Running });

// Running → Idle / Error
self.emit_event(AgentEvent::StateChanged { state: new_state });
```

**2. `api-gateway/src/types.rs`**（新增 `ServerEvent::StateChanged`）：
```rust
pub enum ServerEvent {
    // ... existing variants ...
    #[serde(rename = "state_changed")]
    StateChanged { state: String },
}
```

转换函数：
```rust
fn agent_event_to_server_event(event: &AgentEvent) -> Option<ServerEvent> {
    match event {
        AgentEvent::TurnEnd { messages, .. } => {
            // 从 messages 提取 stop_reason 和 usage
            let (stop_reason, usage) = extract_turn_info(messages);
            Some(ServerEvent::TurnEnd { stop_reason, usage })
        }
        AgentEvent::Error { error } => Some(ServerEvent::Error {
            code: error.code().into(),
            message: error.to_sanitized_string(),
        }),
        AgentEvent::StateChanged { state } => Some(ServerEvent::StateChanged {
            state: format!("{:?}", state).to_lowercase(),
        }),
        _ => None,
    }
}
```

#### Step 3.3：TenantManagerImpl 注册
在 `create_session()` 中：
```rust
if let Some(webhook_config) = params.webhook {
    let listener = WebhookEventListener::new(
        webhook_config,
        tenant_id.to_string(),
        session_id.to_string(),
        http_client.clone(),
    );
    actor.add_event_listener(Arc::new(listener));
}
```

### 4.4 测试策略

| 测试类型 | 内容 |
|---|---|
| 单元测试 | `WebhookEventListener`：HMAC 签名正确、重退避逻辑、连续失败禁用 |
| 集成测试 | 使用 `wiremock` 作为 webhook 接收方，验证收到的事件格式和签名 |
| E2E 测试 | 创建带 webhook 的 session，发送消息，验证 webhook 端点收到 `turn_end` |

### 4.5 验收标准
- [ ] Webhook 收到的事件 JSON 与 SSE 格式一致
- [ ] HMAC 签名可验证
- [ ] 连续 10 次失败后 webhook 自动禁用，不影响 session 运行
- [ ] `ServerEvent` 新增 `StateChanged` variant，SSE 和 Webhook 均支持

---

## 5. Phase 4：API 必要补充

### 5.1 目标
实现配额查询、同步等待、批量创建、克隆、重置等 API。

### 5.2 涉及文件

| 文件 | 操作 |
|---|---|
| `crates/tenant/src/manager.rs` | 修改（新增 `get_quota()`、`batch_create_sessions()`、`clone_session()`、`reset_session()`） |
| `crates/tenant/src/manager.rs` | 修改（`TenantManager` trait 新增方法） |
| `crates/api-gateway/src/routes/` | 新增/修改（配额、批量、克隆、重置 handler） |
| `crates/api-gateway/src/types.rs` | 修改（新增响应类型） |

### 5.3 具体步骤

#### Step 4.1：TenantManager trait 扩展
```rust
#[async_trait]
pub trait TenantManager: Send + Sync {
    // ... existing methods ...
    
    async fn get_quota(&self, tenant_id: &str) -> Result<QuotaInfo, TenantError>;
    
    async fn batch_create_sessions(
        &self,
        tenant_id: &str,
        count: usize,
        template: CreateSessionParams,
    ) -> Result<BatchCreateResult, TenantError>;
    
    async fn clone_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        title: Option<String>,
    ) -> Result<SessionInfo, TenantError>;
    
    async fn reset_session(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
    ) -> Result<SessionState, TenantError>;
    
    async fn send_message_and_wait(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        content: Vec<ai_provider::Content>,
        timeout_ms: u64,
    ) -> Result<WaitResult, TenantError>;
}
```

#### Step 4.2：配额查询
```rust
pub struct QuotaInfo {
    pub tenant_id: String,
    pub max_concurrent_sessions: usize,
    pub active_sessions: usize,
    pub max_tokens_per_day: u64,
    pub tokens_used_today: u64,
    pub max_tool_calls_per_minute: u64,
    pub tool_calls_in_last_minute: u64,
    pub default_model: String,
    pub available_models: Vec<String>,
}
```

从 `TenantSupervisor` 读取 meter 数据组装。

#### Step 4.3：同步等待 `?wait=true`
Spec 要求"先创建临时事件监听器，然后再调用 `prompt_with_content()`"。为避免递归，将原 `send_message()` 拆分为内部 `_send_message()` 和公开的 `send_message()`：

```rust
// 内部方法，仅触发 turn，不处理 wait 逻辑
async fn _send_message(
    &self,
    tenant_id: &str,
    session_id: &Uuid,
    content: Vec<ai_provider::Content>,
) -> Result<u64, TenantError> {
    // 原 send_message() 的核心逻辑（获取 entry、配额检查、turn_index++、actor.prompt_with_content()）
}

// 公开方法，处理 wait 分支
pub async fn send_message(
    &self,
    tenant_id: &str,
    session_id: &Uuid,
    content: Vec<ai_provider::Content>,
    wait: bool,
    timeout_ms: u64,
) -> Result<SendMessageResult, TenantError> {
    if wait {
        // 1. 先订阅事件（竞态安全：先订阅、后触发）
        let mut rx = self.subscribe_events(tenant_id, session_id).await?;
        
        // 2. 触发 turn
        let turn_index = self._send_message(tenant_id, session_id, content).await?;
        
        // 3. 监听 TurnEnd 或 Error
        let timeout = tokio::time::Duration::from_millis(timeout_ms.min(self.max_sync_wait_ms));
        let result = tokio::time::timeout(timeout, async {
            while let Some(event) = rx.recv().await {
                match event {
                    AgentEvent::TurnEnd { messages, .. } => {
                        return Ok(WaitResult::Completed { turn_index, messages });
                    }
                    AgentEvent::Error { error } => {
                        return Err(map_agent_error(error, tenant_id));
                    }
                    _ => {}
                }
            }
            Ok(WaitResult::Timeout { turn_index })
        }).await;
        
        match result {
            Ok(Ok(WaitResult::Completed { turn_index, messages })) => {
                // 组装 200 响应
                Ok(SendMessageResult::Completed { turn_index, messages })
            }
            Ok(Ok(WaitResult::Timeout { turn_index })) | Err(_) => {
                // 返回 202
                Ok(SendMessageResult::Pending { turn_index })
            }
            Ok(Err(e)) => Err(e),
        }
    } else {
        let turn_index = self._send_message(tenant_id, session_id, content).await?;
        Ok(SendMessageResult::TurnIndex(turn_index))
    }
}
```

```rust
// 内部方法，仅触发 turn，不处理 wait 逻辑
async fn _send_message(
    &self,
    tenant_id: &str,
    session_id: &Uuid,
    content: Vec<ai_provider::Content>,
) -> Result<u64, TenantError> {
    // 原 send_message() 的核心逻辑
}

// 公开方法，处理 wait 分支
async fn send_message(
    &self,
    tenant_id: &str,
    session_id: &Uuid,
    content: Vec<ai_provider::Content>,
    wait: bool,
    timeout_ms: u64,
) -> Result<SendMessageResult, TenantError> {
    if wait {
        let mut rx = self.subscribe_events(tenant_id, session_id).await?;
        let turn_index = self._send_message(tenant_id, session_id, content).await?;
        // ... 监听逻辑 ...
    } else {
        let turn_index = self._send_message(tenant_id, session_id, content).await?;
        Ok(SendMessageResult::TurnIndex(turn_index))
    }
}
```

#### Step 4.4：批量创建
```rust
async fn batch_create_sessions(
    &self,
    tenant_id: &str,
    count: usize,
    template: CreateSessionParams,
) -> Result<BatchCreateResult, TenantError> {
    let max_count = self.server_config.max_batch_size; // 默认 10
    if count > max_count {
        return Err(TenantError::BatchSizeExceeded(max_count));
    }
    
    let supervisor = self.registry.get(tenant_id).ok_or(...)?;
    let current = supervisor.active_session_count();
    let max = supervisor.max_concurrent_sessions();
    if current + count > max {
        return Err(TenantError::QuotaExceeded(format!(
            "need {} slots, only {} available", count, max - current
        )));
    }
    
    let mut created = vec![];
    let mut failed = vec![];
    
    for _ in 0..count {
        match self.create_session(tenant_id, template.clone()).await {
            Ok(info) => created.push(info),
            Err(e) => {
                // 回滚已创建的
                for info in &created {
                    let _ = self.delete_session(tenant_id, &info.id).await;
                }
                failed.push(BatchFailure { reason: e.to_string() });
                return Err(TenantError::BatchCreateFailed { created, failed });
            }
        }
    }
    
    Ok(BatchCreateResult { created, failed })
}
```

#### Step 4.5：克隆 Session
```rust
async fn clone_session(&self, tenant_id: &str, session_id: &Uuid, title: Option<String>) -> Result<SessionInfo, TenantError> {
    let entry = self.sessions.get(&(tenant_id.to_string(), *session_id)).ok_or(...)?;
    let actor = entry.actor.lock().await;
    
    let template = CreateSessionParams {
        title,
        system_prompt: Some(actor.system_prompt()),
        model: Some(actor.model.clone()),
        tools: entry.tools.clone(), // 需要 ActiveSession 保存 tools
        webhook: entry.webhook.clone(),
    };
    drop(actor);
    
    self.create_session(tenant_id, template).await
}
```

**问题**：`ActiveSession` 目前不保存 `tools` 和 `webhook`。需要在 Phase 1/3 中确保 `ActiveSession` 保存这些配置，或从 `SessionActor` 中读取。

**解决方案**：
1. `SessionActor` 新增 `tools()` getter：
   ```rust
   pub fn tools(&self) -> Vec<AgentToolRef> { self.tools.clone() }
   ```
2. `ActiveSession` 在 Phase 1/3 中分别保存 `tools: Vec<AgentToolRef>` 和 `webhook: Option<WebhookConfig>`

**`CreateSessionParams` 字段扩展**：当前 `CreateSessionParams` 仅含 `title` 和 `system_prompt`。根据 Spec 3.4.3（批量创建 template 含 model）和克隆需求，需新增 `model: Option<String>` 字段。`create_session()` 优先使用 `params.model`，否则 fallback 到 `self.default_model`。

#### Step 4.6：重置 Session
```rust
async fn reset_session(&self, tenant_id: &str, session_id: &Uuid) -> Result<SessionState, TenantError> {
    let entry = self.sessions.get(...).ok_or(...)?;
    let mut actor = entry.actor.lock().await;
    actor.reset().await?;
    Ok(actor.state())
}
```

**响应类型**：`api-gateway/src/types.rs` 新增 `ResetSessionResponse`：
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetSessionResponse {
    pub state: String,
}
```

Handler 返回 `Json(ResetSessionResponse { state: "idle".into() })`。

### 5.4 测试策略

| 测试类型 | 内容 |
|---|---|
| 单元测试 | `TenantManagerImpl` mock 测试：批量创建回滚、配额检查 |
| E2E 测试 | `/tenant/quota`、`/messages?wait=true`、批量创建、克隆、重置 |
| E2E 测试 | 同步等待超时返回 202、成功返回 200 含 assistant message |

### 5.5 验收标准
- [ ] `GET /tenant/quota` 返回正确配额数据
- [ ] `POST /messages?wait=true` 在 turn 完成前返回 200，超时返回 202
- [ ] 批量创建 3 个 session，预检查失败时返回 429 且零创建
- [ ] 克隆 session 复制配置但不复制历史
- [ ] 重置后 `GET /state` 返回 idle

---

## 6. Phase 5：WebSocket 端点

### 6.1 目标
实现 `WS /api/v1/sessions/{id}/ws`，支持双向通信和心跳。

### 6.2 涉及文件

| 文件 | 操作 |
|---|---|
| `crates/api-gateway/src/routes/ws.rs` | **新增** |
| `crates/api-gateway/src/lib.rs` | 修改（注册 WebSocket route） |
| `crates/api-gateway/src/auth.rs` | 修改（支持 WebSocket handshake 认证） |

### 6.3 具体步骤

#### Step 5.1：WebSocket 路由
使用 `axum::extract::ws::WebSocketUpgrade`：
```rust
async fn session_ws_handler(
    ws: WebSocketUpgrade,
    Path(session_id): Path<Uuid>,
    TypedHeader(Authorization(bearer)): TypedHeader<Authorization<Bearer>>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    // 1. 认证（复用 auth_middleware 逻辑）
    // 2. 验证 session 存在
    // 3. upgrade
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, session_id, state)))
}
```

#### Step 5.2：Socket 处理
```rust
async fn handle_socket(mut socket: WebSocket, session_id: Uuid, state: AppState) {
    let mut rx = state.tenant_manager.subscribe_events(...).await?;
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    
    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                let server_event = agent_event_to_server_event(&event);
                socket.send(Message::Text(serde_json::to_string(&server_event).unwrap())).await?;
            }
            _ = interval.tick() => {
                socket.send(Message::Text(r#"{"type":"ping"}"#.to_string())).await?;
            }
            Some(Ok(msg)) = socket.recv() => {
                match msg {
                    Message::Text(text) => handle_client_message(text, &state).await,
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
}
```

#### Step 5.3：客户端消息处理
```rust
async fn handle_client_message(text: String, state: &AppState) {
    let msg: ClientWsMessage = serde_json::from_str(&text)?;
    match msg.action {
        "send_message" => { /* 调用 TenantManager::send_message */ }
        "interrupt" => { /* 调用 TenantManager::interrupt */ }
        "pong" => { /* 忽略 */ }
        _ => {}
    }
}
```

### 6.4 测试策略

| 测试类型 | 内容 |
|---|---|
| E2E 测试 | 使用 `tokio-tungstenite` 客户端连接 WS，验证认证、消息收发、心跳 |
| E2E 测试 | 同时打开 SSE 和 WS，验证两者收到相同事件 |

### 6.5 验收标准
- [ ] WebSocket 连接成功，认证失败返回 401
- [ ] 服务端每 30s 发送 ping，客户端 pong 回复
- [ ] `send_message` action 触发 turn，事件通过 WS 推送
- [ ] 关闭 SSE 后 WS 仍可独立工作

---

## 7. Phase 6：OpenAPI 文档 + E2E 测试

### 7.1 目标
更新 OpenAPI 文档，补齐 E2E 测试覆盖所有新增 API。

### 7.2 涉及文件

| 文件 | 操作 |
|---|---|
| `docs/openapi.yaml` | 修改（新增所有 endpoint、schema、error code） |
| `crates/api-gateway/tests/e2e/` | 新增/修改测试文件 |

### 7.3 具体步骤

#### Step 6.1：OpenAPI 更新
- 新增 paths：`/sessions/batch`、`/sessions/{id}/clone`、`/sessions/{id}/reset`、`/sessions/{id}/state`、`/tenant/quota`、`/sessions/{id}/messages`（wait 参数）
- 新增 schemas：`ToolConfig`、`WebhookConfig`、`SessionStateResponse`、`QuotaInfo`、`BatchCreateRequest`、`BatchCreateResult`、`WaitResult`
- 新增 error codes：`tool_endpoint_invalid`、`tool_endpoint_forbidden`、`batch_size_exceeded`、`session_in_error`

#### Step 6.2：E2E 测试补齐
每个 Phase 的验收标准对应一个 E2E 测试文件：
- `e2e_http_proxy_tool.rs` — Phase 1
- `e2e_session_state.rs` — Phase 2
- `e2e_webhook.rs` — Phase 3
- `e2e_api_extensions.rs` — Phase 4（配额、批量、克隆、重置、同步等待）
- `e2e_websocket.rs` — Phase 5

### 7.4 验收标准
- [ ] `docs/openapi.yaml` 通过 Swagger Editor 验证无语法错误
- [ ] 所有新增 API 均有对应的 E2E 测试
- [ ] `cargo test -p api-gateway --test e2e_*` 全部通过

---

## 8. 依赖关系与并行性

```
Phase 1: HttpProxyTool ──┬──→ Phase 2: Session 状态机 ──→ Phase 4: API 补充
                         │                              （需要 Phase 1/2/3 完成）
                         └──→ Phase 3: Webhook ─────────┘

Phase 5: WebSocket ──→ Phase 6: OpenAPI + E2E
（可与 Phase 4 并行，但建议串行以减少冲突）
```

**可并行**：
- Phase 1 和 Phase 2 理论上可并行（修改不同文件），但 Phase 4 的 `clone_session()` 需要两者都完成
- Phase 5 可与 Phase 4 并行开发，但共享 `api-gateway/src/routes/` 目录，建议串行

**建议执行顺序**：1 → 2 → 3 → 4 → 5 → 6

---

## 9. 风险与回滚策略

| 风险 | 影响 | 缓解措施 |
|---|---|---|
| SSRF 规则遗漏 | 安全漏洞 | 第一版严格黑名单，后续增加正向白名单；E2E 覆盖所有内网段 |
| `state: AtomicU8` 并发竞争 | 状态不一致 | `SeqCst` ordering；在 `run_with_messages()` 单线程内管理转换 |
| Webhook 重试风暴 | 压垮接收方 | 限流 Semaphore(5) + 指数退避 + 10 次禁用 |
| 同步等待超时连接挂起 | 资源泄漏 | 全局 `max_sync_wait_ms` 限制；使用 `tokio::time::timeout` |
| 批量创建回滚不完整 | 配额泄漏 | 回滚逻辑包裹在 `Drop` 中；`SessionGuard` 确保 slot 释放 |

**回滚策略**：
- 每 Phase 使用独立 commit，可通过 `git revert` 单 Phase 回滚
- 新增字段均为 `Option` 或带 `default`，保证向后兼容
- `is_streaming` 替换为 `state` 时，保留 `is_streaming()` 方法作为兼容层（内部读取 state），TUI 等消费者无感知
