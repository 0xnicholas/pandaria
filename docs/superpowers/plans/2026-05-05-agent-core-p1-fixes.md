# agent-core P1+ 缺口修复实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 agent-core 中所有 P1 及更高优先级的实现缺口，包括 Event Processor persistence、Hook 超时、Graceful Shutdown、跨 Provider 消息正规化、API 完善和工程规范。

**Architecture:** 增量式修复，每个任务独立可验证。优先修复阻塞核心功能的 P1 缺口，再补全 P2 功能，最后处理 P3 工程规范。

**Tech Stack:** Rust 2024 edition, tokio, async-trait, thiserror, tracing, uuid, llm-client

**Spec Reference:** `docs/specs/2026-05-02-agent-core.md`, `AGENTS.md`

**Current State (P0 修复后):**
- `session_entry.rs`: SessionEntry enum (Uuid), CompactionDetails, SessionContextBuilder ✅
- `file_ops.rs`: FileOperationExtractor trait, DefaultFileOperationExtractor ✅
- `compaction.rs`: CompactionActor 注入 file_op_extractor，返回 CompactionError ✅
- `session.rs`: 注入 CompactionActor，实现 run_auto_compaction/check_compaction ✅
- `error.rs`: AgentError + CompactionError ✅
- **测试**: 59 tests passed, 0 failed ✅

**缺失/不完整的 P1+ 功能:**
- Event processor 未将 new_messages append 到 entries
- Hook 调用无超时（500ms/100ms）
- SessionActor drop 不等待 event queue
- 未调用 `llm_client::transform_messages()`
- stream_options/max_retries setter 缺乏测试覆盖
- 无 README.md
- 无 testcontainers 集成测试

---

## File Map

### 新增文件
| 文件 | 职责 |
|---|---|
| `src/hook_timeout.rs` | Hook 超时包装函数 `with_timeout` |
| `README.md` | crate 文档（职责、接口、边界） |

### 修改文件
| 文件 | 变更 |
|---|---|
| `src/session.rs` | Event processor 追加 persistence、graceful shutdown |
| `src/loop.rs` | 集成 `transform_messages()` |
| `src/tool.rs` | Hook 调用添加 500ms 超时 |
| `src/hook_dispatcher.rs` | 确认 on_before_compact 签名 |
| `tests/loop_integration_tests.rs` | 添加 transform_messages 验证测试 |
| `Cargo.toml` | 可选：添加 testcontainers 到 dev-dependencies |

---

## Phase 1: Event Processor Persistence (P1 — ~30 min)

> **问题**: spec §7.4 要求 event processor 在处理事件后将 new_messages append 到 entries。当前 processor 只调用 hook 和 listener，不修改 entries。
>
> **方案**: 通过 `mpsc` channel 将 append 请求传回主 task，保持 `Vec<SessionEntry>` 的单线程访问（&mut self）。

### Task 1.1: 在 Event Processor 中追加 Messages 到 Entries

**Files:**
- Modify: `crates/agent-core/src/session.rs`

**Steps:**

- [ ] **Step 1: 修改 QueuedEvent 结构，区分消息追加和事件通知**

在 `session.rs` 顶部：
```rust
enum EventProcessorCommand {
    /// 常规事件（调用 hook + listener）
    EmitEvent { event: AgentEvent },
    /// 追加新消息到 entries（由 MessageEnd 事件触发）
    AppendMessages { messages: Vec<AgentMessage> },
}
```

- [ ] **Step 2: 修改 event_sink，将 MessageEnd 转为 AppendMessages**

在 `run_with_messages` 中：
```rust
let event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync> = Arc::new(move |event| {
    if let Some(tx) = &event_tx {
        match &event {
            AgentEvent::MessageEnd { message } => {
                // 追加消息到 entries（通过 command channel）
                let _ = tx.try_send(EventProcessorCommand::AppendMessages {
                    messages: vec![message.clone()],
                });
            }
            _ => {
                let _ = tx.try_send(EventProcessorCommand::EmitEvent { event });
            }
        }
    }
});
```

- [ ] **Step 3: 修改 event processor loop，处理 AppendMessages**

在 `spawn_event_processor` 中：
```rust
let handle = tokio::spawn(async move {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            EventProcessorCommand::EmitEvent { event } => {
                // 现有逻辑：调用 hook + listener
                match &event {
                    AgentEvent::TurnEnd { turn_index, messages } => {
                        let _ = hook_dispatcher.on_turn_end(&TurnEndCtx {
                            tenant_id: tenant_id.clone(),
                            session_id: session_id.clone(),
                            turn_index: *turn_index,
                            messages: messages.clone(),
                        }).await;
                    }
                    AgentEvent::AgentEnd { messages } => {
                        let _ = hook_dispatcher.on_agent_end(&AgentEndCtx {
                            tenant_id: tenant_id.clone(),
                            session_id: session_id.clone(),
                            messages: messages.clone(),
                        }).await;
                    }
                    _ => {}
                }
                for listener in &listeners {
                    let _ = listener.on_event(&event).await;
                }
            }
            EventProcessorCommand::AppendMessages { messages } => {
                // 这里无法直接访问 entries（&mut self 限制）
                // 方案：通过另一个 channel 发回主 task
                // 更简单方案：在 run_with_messages 中，run 结束后统一 drain
            }
        }
    }
});
```

**注意**: 由于 `entries` 是 `Vec<SessionEntry>`（非 Arc<Mutex<_>>），且 `run_with_messages` 持有 `&mut self`，最简单的方案是在 `AgentLoop::run` 返回后，在 `run_with_messages` 中统一将 `new_messages` append 到 entries。

**修正方案**: 不修改 event processor，而是在 `run_with_messages` 的 loop 结束后：
```rust
// After AgentLoop::run returns:
for msg in &new_msgs {
    self.push_message(msg.clone());
}
```

但这会导致重复 push（AgentLoop 已经 emit MessageEnd）。实际上当前代码中，`run_with_messages` 已经在 loop 结束后手动 push：
```rust
for msg in &msgs { self.push_message(msg.clone()); }
all_new_msgs.extend(msgs);
```

所以当前实现已经是正确的！**Task 1.1 实际上已经完成**（在 P0 修复中已实现）。

**验证**: 运行 `cargo test -p agent-core session` 检查 persistence 测试是否通过。

---

## Phase 2: Hook 超时机制 (P1 — ~45 min)

> **问题**: ADR-003 要求 hook 调用必须设置超时（500ms for blocking/chain, 100ms for observational）。当前直接 `await` 调用，可能永久阻塞 agent loop。
>
> **方案**: 创建 `hook_timeout.rs` 模块，提供 `with_timeout` 包装函数。在 `tool.rs` 和 `loop.rs` 中应用。

### Task 2.1: 创建 Hook 超时模块

**Files:**
- Create: `crates/agent-core/src/hook_timeout.rs`

**Steps:**

- [ ] **Step 1: 创建 hook_timeout.rs**

```rust
use std::time::Duration;
use tracing::warn;

/// Execute an async block with a timeout.
/// Returns the default value if the future does not complete within `timeout_ms`.
pub async fn with_timeout<F, T>(
    future: F,
    timeout_ms: u64,
    default: T,
    hook_name: &'static str,
) -> T
where
    F: std::future::Future<Output = T>,
{
    match tokio::time::timeout(Duration::from_millis(timeout_ms), future).await {
        Ok(result) => result,
        Err(_) => {
            warn!(
                "Hook '{}' timed out after {}ms, using default",
                hook_name, timeout_ms
            );
            default
        }
    }
}
```

- [ ] **Step 2: 添加到 lib.rs**

```rust
pub(crate) mod hook_timeout;
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p agent-core`
Expected: PASS

---

### Task 2.2: 在 ToolExecutor 中添加 Hook 超时

**Files:**
- Modify: `crates/agent-core/src/tool.rs`

**Steps:**

- [ ] **Step 1: 在 tool.rs 中使用 with_timeout**

修改 `execute_tool_call`：

```rust
use crate::hook_timeout::with_timeout;

// Step 1: on_tool_call (blocking, 500ms timeout → default Continue)
let (decision, _mutation) = with_timeout(
    self.hook_dispatcher.on_tool_call(&tool_call_ctx),
    500,
    (HookDecision::Continue, ToolCallMutation::default()),
    "on_tool_call",
).await;

// Step 3: on_tool_result (chain, 500ms timeout → default mutation)
let mutation = with_timeout(
    self.hook_dispatcher.on_tool_result(&tool_result_ctx),
    500,
    ToolResultMutation::default(),
    "on_tool_result",
).await;
```

- [ ] **Step 2: 运行 tool 测试**

Run: `cargo test -p agent-core tool`
Expected: ALL PASS

---

### Task 2.3: 在 AgentLoop 中添加 Hook 超时

**Files:**
- Modify: `crates/agent-core/src/loop.rs`

**Steps:**

- [ ] **Step 1: 在 run_turn 中为所有 hook 添加超时**

修改 hook 调用点：

```rust
use crate::hook_timeout::with_timeout;

// on_context (chain, 500ms)
let ctx_mutation = with_timeout(
    self.config.hook_dispatcher.on_context(&ctx_ctx),
    500,
    ContextMutation::default(),
    "on_context",
).await;

// on_before_provider_request (chain, 500ms)
let provider_req_mutation = with_timeout(
    self.config.hook_dispatcher.on_before_provider_request(&provider_req_ctx),
    500,
    ProviderRequestMutation::default(),
    "on_before_provider_request",
).await;

// on_after_provider_response (chain, 500ms)
let provider_resp_mutation = with_timeout(
    self.config.hook_dispatcher.on_after_provider_response(&provider_resp_ctx),
    500,
    ProviderResponseMutation::default(),
    "on_after_provider_response",
).await;

// on_turn_end (observational, 100ms) — 注意这个在 loop.rs 和 session.rs 都有
let _ = with_timeout(
    self.config.hook_dispatcher.on_turn_end(&TurnEndCtx { ... }),
    100,
    (),
    "on_turn_end",
).await;
```

- [ ] **Step 2: 运行 loop 测试**

Run: `cargo test -p agent-core loop_`
Expected: ALL PASS

---

### Task 2.4: 在 SessionActor 中添加 Observational Hook 超时

**Files:**
- Modify: `crates/agent-core/src/session.rs`

**Steps:**

- [ ] **Step 1: 在 spawn_event_processor 中为 on_turn_end/on_agent_end 添加 100ms 超时**

```rust
use crate::hook_timeout::with_timeout;

// In event processor:
AgentEvent::TurnEnd { turn_index, messages } => {
    let _ = with_timeout(
        hook_dispatcher.on_turn_end(&TurnEndCtx {
            tenant_id: tenant_id.clone(),
            session_id: session_id.clone(),
            turn_index: *turn_index,
            messages: messages.clone(),
        }),
        100,
        (),
        "on_turn_end",
    ).await;
}
AgentEvent::AgentEnd { messages } => {
    let _ = with_timeout(
        hook_dispatcher.on_agent_end(&AgentEndCtx {
            tenant_id: tenant_id.clone(),
            session_id: session_id.clone(),
            messages: messages.clone(),
        }),
        100,
        (),
        "on_agent_end",
    ).await;
}
```

- [ ] **Step 2: 在 new() 中为 on_session_start 添加 100ms 超时**

```rust
tokio::spawn(async move {
    let _ = with_timeout(
        dispatcher.on_session_start(&session_ctx),
        100,
        (),
        "on_session_start",
    ).await;
});
```

- [ ] **Step 3: 在 run_auto_compaction 中为 on_before_compact 添加 500ms 超时**

```rust
let decision = with_timeout(
    self.hook_dispatcher.on_before_compact(&compact_ctx),
    500,
    CompactDecision::Continue,
    "on_before_compact",
).await;
```

- [ ] **Step 4: 运行 session 测试**

Run: `cargo test -p agent-core session`
Expected: ALL PASS

---

## Phase 3: Graceful Shutdown (P1 — ~20 min)

> **问题**: SessionActor drop 时可能丢失正在处理的事件。
>
> **方案**: 实现 `Drop`，关闭 channel 并等待 processor task 结束。

### Task 3.1: SessionActor Drop 实现

**Files:**
- Modify: `crates/agent-core/src/session.rs`

**Steps:**

- [ ] **Step 1: 为 SessionActor 实现 Drop**

```rust
impl Drop for SessionActor {
    fn drop(&mut self) {
        // 关闭 event channel，通知 processor 结束
        if let Some(tx) = self.event_tx.take() {
            drop(tx);
        }

        // 等待 processor task 完成（带 1s 超时）
        if let Some(handle) = self.event_processor_handle.take() {
            // 注意：不能在 async context 中使用 block_on，这里简单 abort
            // 对于 fire-and-forget hook，abort 是可接受的
            handle.abort();
        }
    }
}
```

**注意**: 由于 `Drop::drop(&mut self)` 不是 async，无法使用 `await`。对于 observational hook（fire-and-forget），abort 是合理的。如果需要真正 graceful shutdown，应在 `SessionActor` 上添加 `async fn shutdown()` 方法。

- [ ] **Step 2: 添加 shutdown() 方法**

```rust
impl SessionActor {
    /// Gracefully shutdown the session, waiting for pending events to be processed.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.event_tx.take() {
            drop(tx); // Close channel
        }
        if let Some(handle) = self.event_processor_handle.take() {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                handle
            ).await;
        }
    }
}
```

- [ ] **Step 3: 添加测试**

```rust
#[tokio::test]
async fn test_session_shutdown() {
    let _ = tracing_subscriber::fmt().try_init();
    let provider = Arc::new(EchoProvider);
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(
        "t1".to_string(), "s1".to_string(),
        "prompt".to_string(), "echo".to_string(),
        provider.clone(), dispatcher,
        Arc::new(make_compaction_actor(provider)),
        vec![], None,
    );

    session.prompt("hello".to_string()).await.unwrap();
    session.shutdown().await;
    // 验证不 panic
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p agent-core test_session_shutdown`
Expected: PASS

---

## Phase 4: 跨 Provider 消息正规化 (P2 — ~30 min)

> **问题**: spec §2.2 step 2.6 要求在 `call_llm_with_retry` 前调用 `llm_client::transform_messages()`，处理 image downgrade、thinking block、tool call ID normalization、orphan padding。
>
> **确认**: `llm-client` 已提供 `transform_messages(messages, &TransformOptions) -> Vec<Message>`。

### Task 4.1: 在 AgentLoop 中集成 transform_messages

**Files:**
- Modify: `crates/agent-core/src/loop.rs`

**Steps:**

- [ ] **Step 1: 在 run_turn 中，apply_provider_request_mutation 之后添加 transform**

```rust
// After: apply_provider_request_mutation(&mut ctx, &mut stream_opts, provider_req_mutation);

// Cross-provider message normalization (spec §2.2 step 2.6)
let provider_name = self.config.provider.provider_name();
let transform_opts = llm_client::TransformOptions {
    target_api: Some(provider_name.to_string()),
    supports_images: true, // TODO: 从 model registry 获取
    preserve_thinking: false, // v0.1: strip thinking for safety
};
ctx.messages = llm_client::transform_messages(&ctx.messages, &transform_opts);
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p agent-core`
Expected: PASS

- [ ] **Step 3: 添加集成测试**

在 `tests/loop_integration_tests.rs` 中添加：

```rust
#[tokio::test]
async fn test_transform_messages_removes_thinking() {
    let _ = tracing_subscriber::fmt().try_init();

    struct ThinkingProvider;
    #[async_trait]
    impl LlmProvider for ThinkingProvider {
        fn provider_name(&self) -> &str { "thinking-test" }
        fn models(&self) -> Vec<String> { vec!["test".to_string()] }
        async fn stream(&self, _model: &str, context: LlmContext, _options: StreamOptions, _signal: CancellationToken,
        ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
            // Verify thinking blocks were removed
            for msg in &context.messages {
                if let llm_client::Message::Assistant(a) = msg {
                    for c in &a.content {
                        assert!(!matches!(c, Content::Thinking { .. }), "thinking should be removed");
                    }
                }
            }
            
            let (stream, tx) = llm_client::AssistantMessageEventStream::new(4);
            let partial = llm_client::AssistantMessage {
                content: vec![Content::Text { text: "ok".to_string(), text_signature: None }],
                provider: "thinking-test".to_string(), model: "test".to_string(),
                api: llm_client::Api { provider: "thinking-test".to_string(), model: "test".to_string() },
                usage: llm_client::Usage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 0 },
                stop_reason: StopReason::Stop, response_id: None, error_message: None,
                timestamp: std::time::SystemTime::now(),
            };
            tokio::spawn(async move {
                let _ = tx.send(llm_client::AssistantMessageEvent::Start { partial: partial.clone() }).await;
                let _ = tx.send(llm_client::AssistantMessageEvent::Done { reason: StopReason::Stop, message: partial }).await;
            });
            Ok(stream)
        }
    }

    let provider = Arc::new(ThinkingProvider);
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(
        "t1".to_string(), "s1".to_string(),
        "prompt".to_string(), "thinking-test".to_string(),
        provider.clone(), dispatcher,
        Arc::new(agent_core::CompactionActor::new(
            agent_core::CompactionConfig::default(),
            provider, "test".to_string(),
            Arc::new(agent_core::DefaultFileOperationExtractor::default()),
        )),
        vec![], None,
    );

    // 这个测试验证 transform_messages 在 AgentLoop 中被调用
    // 具体验证在 ThinkingProvider::stream 中完成
    let result = session.prompt("test".to_string()).await;
    assert!(result.is_ok());
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p agent-core test_transform_messages`
Expected: PASS

---

## Phase 5: API 完善与测试 (P2 — ~30 min)

> **问题**: `set_stream_options()` 和 `set_max_retries()` 已添加，但缺乏测试覆盖。

### Task 5.1: 添加 Setter 测试

**Files:**
- Modify: `crates/agent-core/src/session.rs` (tests 模块)

**Steps:**

- [ ] **Step 1: 添加 setter 测试**

```rust
#[tokio::test]
async fn test_session_setters() {
    let _ = tracing_subscriber::fmt().try_init();
    let provider = Arc::new(EchoProvider);
    let dispatcher = Arc::new(AllowAllDispatcher);
    let mut session = SessionActor::new(
        "t1".to_string(), "s1".to_string(),
        "prompt".to_string(), "echo".to_string(),
        provider.clone(), dispatcher,
        Arc::new(make_compaction_actor(provider)),
        vec![], None,
    );

    session.set_system_prompt("new prompt".to_string());
    assert_eq!(session.system_prompt(), "new prompt");

    session.set_model("gpt-4".to_string());
    assert_eq!(session.model_context_window(), 128_000);

    let mut opts = llm_client::StreamOptions::default();
    opts.max_tokens = Some(100);
    session.set_stream_options(opts.clone());
    
    session.set_max_retries(5);
    assert_eq!(session.max_retries, 5); // 需要添加 pub(crate) 或 accessor
}
```

**注意**: `max_retries` 和 `stream_options` 当前是 private 字段。需要添加 getter 或改为 `pub(crate)`。

- [ ] **Step 2: 添加 accessor**

```rust
impl SessionActor {
    pub fn max_retries(&self) -> u32 { self.max_retries }
    pub fn stream_options(&self) -> &llm_client::StreamOptions { &self.stream_options }
}
```

- [ ] **Step 3: 运行测试**

Run: `cargo test -p agent-core test_session_setters`
Expected: PASS

---

## Phase 6: README.md (P3 — ~20 min)

### Task 6.1: 创建 README.md

**Files:**
- Create: `crates/agent-core/README.md`

**Steps:**

- [ ] **Step 1: 编写 README**

```markdown
# agent-core

Agent loop 核心运行时。驱动 LLM tool use 协议的双层循环，管理 session 生命周期，定义 HookDispatcher 依赖反转边界。

## 职责

- **AgentLoop**: 驱动 tool-use 协议（ADR-001）
- **SessionActor**: 管理 per-tenant session 生命周期
- **HookDispatcher**: Extension 边界（ADR-002/ADR-003）
- **SessionStore**: 持久化边界（ADR-005）
- **ToolExecutor**: 工具执行管道

## 公开接口

### 核心类型

- `SessionActor`: Session 生命周期管理（prompt/continue/complete/abort）
- `AgentLoop`: Agent loop 驱动（run）
- `AgentLoopConfig`: Loop 配置
- `HookDispatcher`: Extension hook trait（10 个方法）
- `SessionStore`: 持久化 trait（save_session/load_session）

### 事件系统

- `AgentEvent`: 16 个变体的枚举
- `AgentEventListener`: 事件监听 trait

### Compaction

- `CompactionActor`: 上下文压缩
- `CompactionConfig`: 压缩配置

### 错误

- `AgentError`: 7 个变体
- `CompactionError`: 2 个变体

## 边界

- **向上**: 被 `extensions` 和 `api-gateway` 依赖
- **向下**: 依赖 `llm-client`
- **横向**: `persistence` crate 实现 `SessionStore`

## 设计约束

- 所有异步代码使用 `tokio`
- Hook 调用带超时（500ms blocking/chain, 100ms observational）
- Extension panic 不传播到 agent loop（`catch_panic`）
- `tenant_id` 出现在所有 tracing span 中
```

- [ ] **Step 2: 验证格式**

Run: `cargo doc -p agent-core --no-deps`
Expected: 无警告

---

## Phase 7: 集成测试增强 (P3 — ~30 min)

### Task 7.1: 添加 testcontainers 集成测试（可选）

**Files:**
- Modify: `crates/agent-core/Cargo.toml`
- Create: `crates/agent-core/tests/session_store_integration_tests.rs`

**Steps:**

- [ ] **Step 1: 添加 testcontainers 到 Cargo.toml**

```toml
[dev-dependencies]
testcontainers = "0.15"
```

- [ ] **Step 2: 创建集成测试文件**

```rust
use agent_core::{SessionActor, SessionStore, AgentError};
use agent_core::types::SessionEntry;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// In-memory store for testing (already exists in session.rs tests)
struct MemoryStore {
    data: Mutex<Vec<(String, String, Vec<SessionEntry>)>>,
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn save_session(&self, tenant_id: &str, session_id: &str, entries: &[SessionEntry]) -> Result<(), AgentError> {
        self.data.lock().unwrap().push((
            tenant_id.to_string(),
            session_id.to_string(),
            entries.to_vec(),
        ));
        Ok(())
    }

    async fn load_session(&self, tenant_id: &str, session_id: &str) -> Result<Vec<SessionEntry>, AgentError> {
        let data = self.data.lock().unwrap();
        Ok(data.iter()
            .rev()
            .find_map(|(tid, sid, msgs)| {
                if tid == tenant_id && sid == session_id { Some(msgs.clone()) } else { None }
            })
            .unwrap_or_default())
    }
}

#[tokio::test]
async fn test_session_persistence_roundtrip() {
    let store = Arc::new(MemoryStore { data: Mutex::new(Vec::new()) });
    // ... test implementation
}
```

**注意**: 由于 `SessionStore` 的实现（Redis/PostgreSQL）在 `persistence` crate 中，agent-core 中的集成测试使用 in-memory store 即可。真正的外部存储集成测试应在 `persistence` crate 中编写。

- [ ] **Step 3: 运行测试**

Run: `cargo test -p agent-core --test session_store_integration_tests`
Expected: PASS

---

## 全局验证

### 最终编译检查

```bash
cargo check -p agent-core
cargo test -p agent-core
cargo clippy -p agent-core -- -D warnings
cargo doc -p agent-core --no-deps
```

### 预期结果

- `cargo check`: 无 error，warnings 在合理范围
- `cargo test`: 所有测试通过（预计 65+ 测试）
- `cargo clippy`: 无 clippy error
- `cargo doc`: 所有公开 API 有文档注释

---

## Review Loop

1. **Plan Review**: 使用 `plan-document-reviewer` 子agent 审查本计划
2. **Implementation Review**: 每完成一个 Phase，运行 `cargo test` 验证
3. **Final Review**: 所有 Phase 完成后，运行完整测试套件 + clippy

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-05-agent-core-p1-fixes.md`.**

**Two execution options:**

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration. Best for parallelizable work.

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints for review. Best for focused sequential work.

**Which approach?**
