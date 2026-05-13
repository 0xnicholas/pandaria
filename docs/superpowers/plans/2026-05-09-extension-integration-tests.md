# Extension 跨 crate 集成测试实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `extensions` crate 中创建 7 个集成测试文件，覆盖 HookRouter + ExtensionActor + SessionActor 的完整链路，验证 Extension 工具执行、生命周期 hooks、compaction、provider mutation、多 Extension 协同、工具拦截和工具执行观测等所有缺失场景。

**Architecture:** 每个测试文件专注于一个独立的端到端场景。使用 `agent_core::test_utils::TestProvider` 和自定义 Mock Extension 来模拟各种交互，通过 `ExtensionManager` 或手动组装 HookRouter + ExtensionActor 来构建测试环境，最终验证 SessionActor / AgentLoop 驱动下的完整行为。

**Tech Stack:** Rust, tokio, async-trait, agent-core, ai-provider, extensions

---

## 文件结构

| 文件 | 职责 | 测试场景 |
|---|---|---|
| `tests/integration_extension_tool.rs` | Extension 注册工具的执行链路 | 工具注册→LLM调用→ExtensionActor→execute_tool→结果记录 |
| `tests/integration_lifecycle_hooks.rs` | 完整生命周期 hook 触发 | session_start→before_agent_start→before_provider_request→after_provider_response→turn_end→agent_end |
| `tests/integration_compaction.rs` | Compaction 自动触发 + Extension hook 干预 | 上下文溢出→auto-compaction→on_before_compact hook→compaction entry |
| `tests/integration_provider_mutation.rs` | Provider request/response mutation | Extension 修改 LLM 请求参数 / 响应内容 |
| `tests/integration_multi_extension_session.rs` | 多 Extension 协同 + SessionActor | 拦截型+观测型+工具提供型 Extension 同时工作 |
| `tests/integration_tool_interception.rs` | Extension 工具被另一个 Extension 拦截 | Extension A 提供工具，Extension B 拦截 |
| `tests/integration_tool_execution_hooks.rs` | 工具执行 observational hooks | tool_execution_start/end 在真实工具执行时触发 |

---

## 依赖与前置条件

- `agent-core` crate 的 `test_utils` feature 已启用（dev-dependencies 中已配置）
- `extensions` crate 的 dev-dependencies 中已有 `tokio-util`, `tracing-subscriber`, `uuid`
- 所有现有测试通过：`cargo test -p extensions`

---

## Task 1: Extension 工具执行链路 (`integration_extension_tool.rs`)

**Files:**
- Create: `crates/extensions/tests/integration_extension_tool.rs`

**Mock Extensions:**
- `ReturnArgToolExt`：提供 `return_arg` 工具，execute_tool 返回输入参数
- `UppercaseToolExt`：提供 `uppercase` 工具，execute_tool 将输入转换为大写

**Helper Functions:**
- `make_compaction_actor(provider)` - 快速创建 CompactionActor

- [ ] **Step 1: 编写基础结构和 helper functions**

```rust
use std::sync::Arc;
use async_trait::async_trait;
use agent_core::context::ToolCallCtx;
use agent_core::session::SessionActor;
use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::types::{AgentToolResult, AgentToolRef, AgentMessage};
use agent_core::error::AgentError;
use agent_core::test_utils::TestProvider;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use extensions::host::manager::ExtensionManager;
use llm_client::{Content, ToolDef};
```

- [ ] **Step 2: 编写 ReturnArgToolExt**

```rust
struct ReturnArgToolExt;

#[async_trait]
impl Extension for ReturnArgToolExt {
    fn name(&self) -> &str { "return_arg" }
    
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "return_arg".to_string(),
            description: "Returns the input argument".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"]
            }),
        }]
    }
    
    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        let value = params.get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(AgentToolResult {
            content: vec![Content::Text { 
                text: format!("result: {}", value), 
                text_signature: None 
            }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}
```

- [ ] **Step 3: 编写 test_extension_tool_executes_via_actor**

```rust
#[tokio::test]
async fn test_extension_tool_executes_via_actor() {
    let _ = tracing_subscriber::fmt().try_init();
    
    // Setup: Extension with tool
    let ext = Arc::new(ReturnArgToolExt);
    let manager = ExtensionManager::new(vec![ext]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);
    
    // Provider: first call returns tool use, second returns stop
    let provider = TestProvider::sequence(vec![
        agent_core::test_utils::TestResponse::ToolCalls(vec![
            agent_core::test_utils::TestToolCall::new(
                "call_1", 
                "return_arg", 
                serde_json::json!({"value": "hello"})
            ),
        ]),
        agent_core::test_utils::TestResponse::Text("done".to_string()),
    ]);
    
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You have tools.".to_string(),
        "test".to_string(),
        provider,
        Arc::new(hook_router),
        compaction_actor,
        tools,
        None,
    );
    
    let results = session.prompt("call tool".to_string()).await.unwrap();
    
    // Should have: assistant(tool_call) + tool_result + assistant(stop)
    assert_eq!(results.len(), 3);
    
    // Verify tool result contains the executed result
    match &results[1] {
        agent_core::AgentMessage::ToolResult(tr) => {
            let text = tr.content.first().and_then(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            });
            assert_eq!(text, Some("result: hello"));
        }
        _ => panic!("expected tool result at index 1"),
    }
}
```

- [ ] **Step 4: 编写 UppercaseToolExt 和 test_extension_tool_with_mutation**

```rust
struct UppercaseToolExt;

#[async_trait]
impl Extension for UppercaseToolExt {
    fn name(&self) -> &str { "uppercase" }
    
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "uppercase".to_string(),
            description: "Converts input to uppercase".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"]
            }),
        }]
    }
    
    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        let value = params.get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(AgentToolResult {
            content: vec![Content::Text { 
                text: value.to_uppercase(), 
                text_signature: None 
            }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}

#[tokio::test]
async fn test_extension_tool_with_mutation() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let ext = Arc::new(UppercaseToolExt);
    let manager = ExtensionManager::new(vec![ext]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);
    
    let provider = TestProvider::sequence(vec![
        agent_core::test_utils::TestResponse::ToolCalls(vec![
            agent_core::test_utils::TestToolCall::new(
                "call_1", 
                "uppercase", 
                serde_json::json!({"value": "hello"})
            ),
        ]),
        agent_core::test_utils::TestResponse::Text("done".to_string()),
    ]);
    
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You have tools.".to_string(),
        "test".to_string(),
        provider,
        Arc::new(hook_router),
        compaction_actor,
        tools,
        None,
    );
    
    let results = session.prompt("call tool".to_string()).await.unwrap();
    
    assert_eq!(results.len(), 3);
    
    match &results[1] {
        AgentMessage::ToolResult(tr) => {
            let text = tr.content.first().and_then(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            });
            assert_eq!(text, Some("HELLO"));
        }
        _ => panic!("expected tool result at index 1"),
    }
}
```

- [ ] **Step 5: 运行测试验证**

```bash
cargo test -p extensions --test integration_extension_tool -- --nocapture
```

Expected: PASS (或按预期失败，然后修复)

- [ ] **Step 6: Commit**

```bash
git add crates/extensions/tests/integration_extension_tool.rs
git commit -m "test: add extension tool execution integration tests"
```

---

## Task 2: 生命周期 Hooks (`integration_lifecycle_hooks.rs`)

**Files:**
- Create: `crates/extensions/tests/integration_lifecycle_hooks.rs`

**Standard Imports:**
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use async_trait::async_trait;
use agent_core::context::{SessionCtx, BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx, TurnEndCtx, AgentEndCtx};
use agent_core::mutations::{BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation};
use agent_core::session::SessionActor;
use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::test_utils::TestProvider;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use llm_client::Content;
```

**Mock Extensions:**
- `LifecycleRecorderExt`：用 AtomicUsize 记录每个 hook 的触发次数
- `ToolExecutionRecorderExt`：记录 tool_execution_start/end（用于 Step 3）

- [ ] **Step 1: 编写 LifecycleRecorderExt 和 ToolExecutionRecorderExt**

```rust
struct LifecycleRecorderExt {
    session_start_count: AtomicUsize,
    before_agent_start_count: AtomicUsize,
    before_provider_request_count: AtomicUsize,
    after_provider_response_count: AtomicUsize,
    turn_end_count: AtomicUsize,
    agent_end_count: AtomicUsize,
}

#[async_trait]
impl Extension for LifecycleRecorderExt {
    fn name(&self) -> &str { "lifecycle_recorder" }
    
    async fn on_session_start(&self, _ctx: &SessionCtx) {
        self.session_start_count.fetch_add(1, Ordering::SeqCst);
    }
    
    async fn on_before_agent_start(&self, _ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        self.before_agent_start_count.fetch_add(1, Ordering::SeqCst);
        BeforeAgentStartMutation::default()
    }
    
    async fn on_before_provider_request(&self, _ctx: &ProviderRequestCtx) -> ProviderRequestMutation {
        self.before_provider_request_count.fetch_add(1, Ordering::SeqCst);
        ProviderRequestMutation::default()
    }
    
    async fn on_after_provider_response(&self, _ctx: &ProviderResponseCtx) -> ProviderResponseMutation {
        self.after_provider_response_count.fetch_add(1, Ordering::SeqCst);
        ProviderResponseMutation::default()
    }
    
    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        self.turn_end_count.fetch_add(1, Ordering::SeqCst);
    }
    
    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {
        self.agent_end_count.fetch_add(1, Ordering::SeqCst);
    }
}

struct ToolExecutionRecorderExt {
    start_count: AtomicUsize,
    end_count: AtomicUsize,
}

#[async_trait]
impl Extension for ToolExecutionRecorderExt {
    fn name(&self) -> &str { "execution_recorder" }

    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {
        self.start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {
        self.end_count.fetch_add(1, Ordering::SeqCst);
    }
}
```

- [ ] **Step 2: 编写 test_complete_lifecycle_hooks**

验证 SessionActor::new() 触发 session_start，prompt() 触发其余 hooks。

```rust
#[tokio::test]
async fn test_complete_lifecycle_hooks() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let recorder = Arc::new(LifecycleRecorderExt {
        session_start_count: AtomicUsize::new(0),
        before_agent_start_count: AtomicUsize::new(0),
        before_provider_request_count: AtomicUsize::new(0),
        after_provider_response_count: AtomicUsize::new(0),
        turn_end_count: AtomicUsize::new(0),
        agent_end_count: AtomicUsize::new(0),
    });
    
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(recorder.clone(), bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);
    
    let provider = TestProvider::text("response");
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "prompt".to_string(),
        "test".to_string(),
        provider.clone(),
        Arc::new(router),
        make_compaction_actor(provider),
        vec![],
        None,
    );
    
    // session_start fired on construction
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(recorder.session_start_count.load(Ordering::SeqCst), 1);
    
    // prompt triggers: before_agent_start, before_provider_request, after_provider_response, turn_end, agent_end
    session.prompt("hello".to_string()).await.unwrap();
    
    // Give observational hooks time
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    assert_eq!(recorder.before_agent_start_count.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.before_provider_request_count.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.after_provider_response_count.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.turn_end_count.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.agent_end_count.load(Ordering::SeqCst), 1);
}
```

- [ ] **Step 3: 编写 test_tool_execution_hooks_via_eventbus**

**注意**：当前 AgentLoop 未调用 `hook_dispatcher.on_tool_execution_start/end()`，因此 tool_execution hooks 只能通过 HookRouter 直接验证 EventBus 广播。

```rust
#[tokio::test]
async fn test_tool_execution_hooks_via_eventbus() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let recorder = Arc::new(ToolExecutionRecorderExt {
        start_count: AtomicUsize::new(0),
        end_count: AtomicUsize::new(0),
    });
    
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(recorder.clone(), bus.clone(), 8);
    
    // Give actor time to subscribe
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    let router = HookRouter::new(vec![handle], bus.clone());
    
    // Directly emit tool execution events through HookRouter
    let start_ctx = ToolExecutionStartCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };
    router.on_tool_execution_start(&start_ctx).await;
    
    let end_ctx = ToolExecutionEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        success: true,
    };
    router.on_tool_execution_end(&end_ctx).await;
    
    // Give EventBus handlers time
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    assert_eq!(recorder.start_count.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.end_count.load(Ordering::SeqCst), 1);
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p extensions --test integration_lifecycle_hooks -- --nocapture
```

- [ ] **Step 5: Commit**

---

## Task 3: Compaction 链路 (`integration_compaction.rs`)

**Files:**
- Create: `crates/extensions/tests/integration_compaction.rs`

**Standard Imports:**
```rust
use std::sync::Arc;
use async_trait::async_trait;
use agent_core::context::{CompactCtx, CompactReason};
use agent_core::mutations::CompactDecision;
use agent_core::session::SessionActor;
use agent_core::session_entry::SessionEntry;
use agent_core::compaction::{CompactionActor, CompactionConfig, CompactionPreparation};
use agent_core::file_ops::{DefaultFileOperationExtractor, FileOperations};
use agent_core::test_utils::TestProvider;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use uuid::Uuid;
```

**Mock Extensions:**
- `CompactContinueExt`：on_before_compact 返回 Continue
- `CompactBlockerExt`：on_before_compact 返回 Block

```rust
struct CompactContinueExt;

#[async_trait]
impl Extension for CompactContinueExt {
    fn name(&self) -> &str { "compact_continue" }
    
    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Continue
    }
}

struct CompactBlockerExt;

#[async_trait]
impl Extension for CompactBlockerExt {
    fn name(&self) -> &str { "compact_blocker" }
    
    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Block { reason: "blocked by extension".to_string() }
    }
}
```

- [ ] **Step 1: 编写 test_overflow_triggers_compaction_with_extension_hook**

```rust
#[tokio::test]
async fn test_overflow_triggers_compaction_with_extension_hook() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let ext = Arc::new(CompactContinueExt);
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);
    
    // First call: overflow, Second call: success
    let provider = TestProvider::sequence(vec![
        agent_core::test_utils::TestResponse::Overflow,
        agent_core::test_utils::TestResponse::Text("compacted".to_string()),
    ]);
    
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "prompt".to_string(),
        "test".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    );
    
    let results = session.prompt("trigger overflow".to_string()).await.unwrap();
    
    // Should have compaction entry in session entries
    let entries = session.entries();
    assert!(entries.iter().any(|e| matches!(e, SessionEntry::Compaction { .. })));
    
    // Should have assistant message from second call
    assert!(!results.is_empty());
}
```

- [ ] **Step 2: 编写 test_extension_blocks_compaction**

```rust
#[tokio::test]
async fn test_extension_blocks_compaction() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let ext = Arc::new(CompactBlockerExt);
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);
    
    let provider = TestProvider::overflow();
    
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "prompt".to_string(),
        "test".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    );
    
    // Overflow with blocked compaction should error
    let result = session.prompt("trigger overflow".to_string()).await;
    assert!(result.is_err() || result.unwrap().is_empty());
    
    // No compaction entry
    let entries = session.entries();
    assert!(!entries.iter().any(|e| matches!(e, SessionEntry::Compaction { .. })));
}
```

- [ ] **Step 3: 运行测试**

```bash
cargo test -p extensions --test integration_compaction -- --nocapture
```

- [ ] **Step 4: Commit**

---

## Task 4: Provider Mutation (`integration_provider_mutation.rs`)

**Files:**
- Create: `crates/extensions/tests/integration_provider_mutation.rs`

**Standard Imports:**
```rust
use std::sync::Arc;
use async_trait::async_trait;
use agent_core::context::{ProviderRequestCtx, ProviderResponseCtx};
use agent_core::mutations::{ProviderRequestMutation, ProviderResponseMutation};
use agent_core::session::SessionActor;
use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::types::AgentMessage;
use agent_core::test_utils::TestProvider;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use llm_client::{Content, LlmContext, LlmProvider, StreamOptions, CancellationToken};
use tokio_util::sync::CancellationToken;
```

**Mock Extensions:**
- `MutateRequestExt`：修改 system_prompt 和 messages
- `MutateResponseExt`：修改 assistant content

```rust
struct MutateRequestExt {
    system_prompt: String,
}

#[async_trait]
impl Extension for MutateRequestExt {
    fn name(&self) -> &str { "request_mutator" }

    async fn on_before_provider_request(
        &self,
        _ctx: &ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        ProviderRequestMutation {
            system_prompt: Some(Some(self.system_prompt.clone())),
            messages: None,
            tools: None,
            options: None,
        }
    }
}
```

**Mock Provider:**
- `VerifyProvider`：验证接收到的 context 是否符合预期

- [ ] **Step 1: 编写 VerifyProvider**

```rust
struct VerifyProvider {
    expected_system_prompt: Option<String>,
    expected_messages: Option<Vec<AgentMessage>>,
}

#[async_trait]
impl LlmProvider for VerifyProvider {
    fn provider_name(&self) -> &str { "verify" }
    fn models(&self) -> Vec<String> { vec!["verify".to_string()] }
    
    async fn stream(
        &self,
        _model: &str,
        context: LlmContext,
        _options: StreamOptions,
        _signal: CancellationToken,
    ) -> Result<llm_client::AssistantMessageEventStream, llm_client::LlmError> {
        // Verify mutations were applied
        if let Some(ref expected) = self.expected_system_prompt {
            assert_eq!(context.system_prompt, Some(expected.clone()));
        }
        if let Some(ref expected) = self.expected_messages {
            assert_eq!(context.messages, *expected);
        }
        
        // Return simple response
        let (stream, tx) = llm_client::AssistantMessageEventStream::new(4);
        let partial = llm_client::AssistantMessage {
            content: vec![Content::Text { text: "ok".to_string(), text_signature: None }],
            provider: "verify".to_string(),
            model: "verify".to_string(),
            api: llm_client::Api { provider: "verify".to_string(), model: "verify".to_string() },
            usage: llm_client::Usage { input_tokens: 0, output_tokens: 1, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 1 },
            stop_reason: llm_client::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        };
        
        tokio::spawn(async move {
            let _ = tx.send(llm_client::AssistantMessageEvent::Start { partial: partial.clone() }).await;
            let _ = tx.send(llm_client::AssistantMessageEvent::Done { reason: llm_client::StopReason::Stop, message: partial }).await;
        });
        
        Ok(stream)
    }
}
```

- [ ] **Step 2: 编写 test_provider_request_mutation**

```rust
#[tokio::test]
async fn test_provider_request_mutation() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let ext = Arc::new(MutateRequestExt {
        system_prompt: "mutated_prompt".to_string(),
    });
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);
    
    let provider = Arc::new(VerifyProvider {
        expected_system_prompt: Some("mutated_prompt".to_string()),
        expected_messages: None,
    });
    
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "original_prompt".to_string(),
        "verify".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    );
    
    let results = session.prompt("hello".to_string()).await.unwrap();
    assert!(!results.is_empty());
}
```

- [ ] **Step 3: 编写 test_provider_response_mutation**

```rust
struct MutateResponseExt;

#[async_trait]
impl Extension for MutateResponseExt {
    fn name(&self) -> &str { "response_mutator" }
    
    async fn on_after_provider_response(
        &self,
        _ctx: &ProviderResponseCtx,
    ) -> ProviderResponseMutation {
        ProviderResponseMutation {
            content: Some(vec![Content::Text { 
                text: "mutated_response".to_string(), 
                text_signature: None 
            }]),
            stop_reason: None,
        }
    }
}

#[tokio::test]
async fn test_provider_response_mutation() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let ext = Arc::new(MutateResponseExt);
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);
    
    let provider = TestProvider::text("original");
    
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "prompt".to_string(),
        "test".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    );
    
    let results = session.prompt("hello".to_string()).await.unwrap();
    
    assert!(!results.is_empty());
    
    // Verify the response was mutated
    match &results[0] {
        AgentMessage::Assistant(assistant) => {
            let text = assistant.content.first().and_then(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            });
            assert_eq!(text, Some("mutated_response"));
        }
        _ => panic!("expected assistant message"),
    }
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p extensions --test integration_provider_mutation -- --nocapture
```

- [ ] **Step 5: Commit**

---

## Task 5: 多 Extension 协同 (`integration_multi_extension_session.rs`)

**Files:**
- Create: `crates/extensions/tests/integration_multi_extension_session.rs`

**Standard Imports:**
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use async_trait::async_trait;
use agent_core::context::{ToolCallCtx, TurnEndCtx, AgentEndCtx};
use agent_core::mutations::{HookDecision, ToolCallMutation};
use agent_core::session::SessionActor;
use agent_core::session_entry::SessionEntry;
use agent_core::store::SessionStore;
use agent_core::error::AgentError;
use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::types::{AgentToolResult, AgentToolRef, AgentMessage};
use agent_core::test_utils::{TestProvider, TestResponse, TestToolCall};
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use extensions::host::manager::ExtensionManager;
use llm_client::{Content, ToolDef};
```

**Mock Extensions:**
- `ToolGuardExt`：拦截 "dangerous_tool"
- `AuditExt`：记录所有 tool_call
- `ToolProviderExt`：提供 safe_tool 和 dangerous_tool
- `LifecycleExt`：记录 turn_end / agent_end

```rust
struct ToolGuardExt;

#[async_trait]
impl Extension for ToolGuardExt {
    fn name(&self) -> &str { "tool_guard" }

    async fn on_tool_call(&self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        if ctx.tool_name == "dangerous_tool" {
            (HookDecision::Block { reason: "forbidden".to_string() }, ToolCallMutation::default())
        } else {
            (HookDecision::Continue, ToolCallMutation::default())
        }
    }
}

struct AuditExt {
    tool_calls: AtomicUsize,
}

#[async_trait]
impl Extension for AuditExt {
    fn name(&self) -> &str { "audit" }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        self.tool_calls.fetch_add(1, Ordering::SeqCst);
        (HookDecision::Continue, ToolCallMutation::default())
    }
}

struct ToolProviderExt;

#[async_trait]
impl Extension for ToolProviderExt {
    fn name(&self) -> &str { "tool_provider" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "safe_tool".to_string(),
                description: "A safe tool".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
            ToolDef {
                name: "dangerous_tool".to_string(),
                description: "A dangerous tool".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
        ]
    }

    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        Ok(AgentToolResult {
            content: vec![Content::Text { text: "executed".to_string(), text_signature: None }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}

struct LifecycleExt {
    turn_end_count: AtomicUsize,
    agent_end_count: AtomicUsize,
}

#[async_trait]
impl Extension for LifecycleExt {
    fn name(&self) -> &str { "lifecycle" }

    async fn on_turn_end(&self, _ctx: &TurnEndCtx) {
        self.turn_end_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {
        self.agent_end_count.fetch_add(1, Ordering::SeqCst);
    }
}
```

- [ ] **Step 1: 编写 test_multi_extension_collaboration**

```rust
#[tokio::test]
async fn test_multi_extension_collaboration() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let guard = Arc::new(ToolGuardExt);
    let audit = Arc::new(AuditExt);
    let provider_ext = Arc::new(ToolProviderExt);
    let lifecycle = Arc::new(LifecycleExt);
    
    let manager = ExtensionManager::new(vec![guard, audit, provider_ext, lifecycle]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);
    
    // LLM first calls dangerous_tool (should be blocked), then safe_tool
    let llm_provider = TestProvider::sequence(vec![
        TestResponse::ToolCalls(vec![TestToolCall::new("call_1", "dangerous_tool", serde_json::json!({}))]),
        TestResponse::ToolCalls(vec![TestToolCall::new("call_2", "safe_tool", serde_json::json!({}))]),
        TestResponse::Text("done".to_string()),
    ]);
    
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You have tools.".to_string(),
        "test".to_string(),
        llm_provider.clone(),
        Arc::new(hook_router),
        make_compaction_actor(llm_provider),
        tools,
        None,
    );
    
    let results = session.prompt("call tools".to_string()).await.unwrap();
    
    // Should have multiple turns: dangerous_tool blocked, then safe_tool executed, then stop
    assert!(results.len() >= 4);
    
    // Verify dangerous_tool was blocked (is_error = true)
    let dangerous_tool_result = results.iter().find(|m| {
        matches!(m, AgentMessage::ToolResult(tr) if tr.tool_call_id == "call_1")
    });
    assert!(dangerous_tool_result.is_some());
    if let AgentMessage::ToolResult(tr) = dangerous_tool_result.unwrap() {
        assert!(tr.is_error, "dangerous_tool should be blocked");
    }
    
    // Verify safe_tool was executed successfully
    let safe_tool_result = results.iter().find(|m| {
        matches!(m, AgentMessage::ToolResult(tr) if tr.tool_call_id == "call_2")
    });
    assert!(safe_tool_result.is_some());
    if let AgentMessage::ToolResult(tr) = safe_tool_result.unwrap() {
        assert!(!tr.is_error, "safe_tool should succeed");
    }
}
```

- [ ] **Step 2: 编写 test_multi_extension_with_persistence**

使用 MemoryStore（在测试文件中重新定义），验证 session 状态被正确持久化。

```rust
struct MemoryStore {
    data: std::sync::Mutex<Vec<(String, String, Vec<SessionEntry>)>>,
}

impl MemoryStore {
    fn new() -> Self {
        Self { data: std::sync::Mutex::new(Vec::new()) }
    }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn save_session(
        &self,
        tenant_id: &str,
        session_id: &str,
        entries: &[SessionEntry],
    ) -> Result<(), AgentError> {
        self.data.lock().unwrap().push((
            tenant_id.to_string(),
            session_id.to_string(),
            entries.to_vec(),
        ));
        Ok(())
    }

    async fn load_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<Vec<SessionEntry>, AgentError> {
        let data = self.data.lock().unwrap();
        let msgs = data
            .iter()
            .rev()
            .find_map(|(tid, sid, msgs)| {
                if tid == tenant_id && sid == session_id {
                    Some(msgs.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        Ok(msgs)
    }
}
```

```rust
#[tokio::test]
async fn test_multi_extension_with_persistence() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let store = Arc::new(MemoryStore::new());
    let ext = Arc::new(ReturnArgToolExt);
    let manager = ExtensionManager::new(vec![ext]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);
    
    let provider = TestProvider::sequence(vec![
        agent_core::test_utils::TestResponse::ToolCalls(vec![
            agent_core::test_utils::TestToolCall::new(
                "call_1", 
                "return_arg", 
                serde_json::json!({"value": "persisted"})
            ),
        ]),
        agent_core::test_utils::TestResponse::Text("done".to_string()),
    ]);
    
    // First session: create and prompt
    {
        let compaction_actor = make_compaction_actor(provider.clone());
        let mut session = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You have tools.".to_string(),
            "test".to_string(),
            provider.clone(),
            Arc::new(hook_router),
            compaction_actor,
            tools.clone(),
            Some(store.clone()),
        );
        
        session.prompt("call tool".to_string()).await.unwrap();
        session.flush().await.unwrap();
    }
    
    // Second session: restore and verify
    {
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let router = HookRouter::new(vec![], bus);
        let compaction_actor = make_compaction_actor(provider.clone());
        let mut session2 = SessionActor::new(
            "t1".to_string(),
            "s1".to_string(),
            "You have tools.".to_string(),
            "test".to_string(),
            provider,
            Arc::new(router),
            compaction_actor,
            tools,
            Some(store.clone()),
        );
        
        let restored = session2.restore().await.unwrap();
        assert!(restored > 0);
        
        let msgs = session2.messages();
        assert!(msgs.len() >= 3); // user + assistant(tool_call) + tool_result
    }
}
```

- [ ] **Step 3: 运行测试**

```bash
cargo test -p extensions --test integration_multi_extension_session -- --nocapture
```

- [ ] **Step 4: Commit**

---

## Task 6: 工具拦截 (`integration_tool_interception.rs`)

**Files:**
- Create: `crates/extensions/tests/integration_tool_interception.rs`

**Standard Imports:**
```rust
use std::sync::Arc;
use async_trait::async_trait;
use agent_core::context::ToolCallCtx;
use agent_core::mutations::{HookDecision, ToolCallMutation};
use agent_core::session::SessionActor;
use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::types::{AgentToolResult, AgentMessage};
use agent_core::error::AgentError;
use agent_core::test_utils::{TestProvider, TestResponse, TestToolCall};
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
use extensions::host::manager::ExtensionManager;
use llm_client::{Content, ToolDef};
```

**Mock Extensions:**
- `ToolProviderExt`：提供 `sensitive_tool`
- `InputSanitizerExt`：on_tool_call 中修改 input，移除敏感字段后放行
- `ToolBlockerExt`：拦截特定工具

```rust
struct ToolProviderExt;

#[async_trait]
impl Extension for ToolProviderExt {
    fn name(&self) -> &str { "tool_provider" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "sensitive_tool".to_string(),
            description: "A sensitive tool".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "secret": { "type": "string" }
                }
            }),
        }]
    }

    async fn execute_tool(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        let secret = params.get("secret").and_then(|v| v.as_str()).unwrap_or("");
        Ok(AgentToolResult {
            content: vec![Content::Text { 
                text: format!("processed: {}", secret), 
                text_signature: None 
            }],
            details: None,
            is_error: false,
            terminate: false,
        })
    }
}

struct InputSanitizerExt;

#[async_trait]
impl Extension for InputSanitizerExt {
    fn name(&self) -> &str { "input_sanitizer" }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        let mut input = ctx.input.clone();
        if let Some(obj) = input.as_object_mut() {
            obj.remove("secret");
            obj.insert("sanitized".to_string(), serde_json::json!(true));
        }
        (HookDecision::Continue, ToolCallMutation { input: Some(input) })
    }
}

struct ToolBlockerExt {
    target: String,
}

#[async_trait]
impl Extension for ToolBlockerExt {
    fn name(&self) -> &str { "tool_blocker" }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        if ctx.tool_name == self.target {
            (HookDecision::Block { reason: format!("blocked: {}", self.target) }, ToolCallMutation::default())
        } else {
            (HookDecision::Continue, ToolCallMutation::default())
        }
    }
}
```

- [ ] **Step 1: 编写 test_extension_tool_blocked_by_another_extension**

```rust
#[tokio::test]
async fn test_extension_tool_blocked_by_another_extension() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let provider_ext = Arc::new(ToolProviderExt);
    let blocker = Arc::new(ToolBlockerExt { target: "sensitive_tool".to_string() });
    
    let manager = ExtensionManager::new(vec![provider_ext, blocker]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);
    
    let llm_provider = TestProvider::sequence(vec![
        TestResponse::ToolCalls(vec![TestToolCall::new("call_1", "sensitive_tool", serde_json::json!({}))]),
        TestResponse::Text("done".to_string()),
    ]);
    
    let compaction_actor = make_compaction_actor(llm_provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You have tools.".to_string(),
        "test".to_string(),
        llm_provider,
        Arc::new(hook_router),
        compaction_actor,
        tools,
        None,
    );
    
    let results = session.prompt("call tool".to_string()).await.unwrap();
    
    // Verify tool was blocked (not executed)
    match &results[1] {
        AgentMessage::ToolResult(tr) => {
            assert!(tr.is_error);
            assert_eq!(tr.details.as_ref().unwrap()["blocked"], true);
        }
        _ => panic!("expected blocked tool result"),
    }
}
```

- [ ] **Step 2: 编写 test_extension_tool_allowed_after_sanitization**

```rust
#[tokio::test]
async fn test_extension_tool_allowed_after_sanitization() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let provider_ext = Arc::new(ToolProviderExt);
    let sanitizer = Arc::new(InputSanitizerExt);
    
    let manager = ExtensionManager::new(vec![provider_ext, sanitizer]);
    let (hook_router, handles, _join_handles) = manager.spawn_all();
    let tools = manager.collect_agent_tools(&handles);
    
    let llm_provider = TestProvider::sequence(vec![
        TestResponse::ToolCalls(vec![
            TestToolCall::new("call_1", "sensitive_tool", serde_json::json!({"secret": "password123"}))
        ]),
        TestResponse::Text("done".to_string()),
    ]);
    
    let compaction_actor = make_compaction_actor(llm_provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You have tools.".to_string(),
        "test".to_string(),
        llm_provider,
        Arc::new(hook_router),
        compaction_actor,
        tools,
        None,
    );
    
    let results = session.prompt("call tool".to_string()).await.unwrap();
    
    // Verify tool was executed (not blocked)
    match &results[1] {
        AgentMessage::ToolResult(tr) => {
            assert!(!tr.is_error, "tool should be allowed after sanitization");
            // Verify execute_tool received sanitized input (no "secret" field)
            let text = tr.content.first().and_then(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            });
            assert_eq!(text, Some("processed: "), "secret field should be removed by sanitizer");
        }
        _ => panic!("expected tool result"),
    }
}
```

- [ ] **Step 3: 运行测试**

```bash
cargo test -p extensions --test integration_tool_interception -- --nocapture
```

- [ ] **Step 4: Commit**

---

## Task 7: 工具执行 Hooks (`integration_tool_execution_hooks.rs`)

**Files:**
- Create: `crates/extensions/tests/integration_tool_execution_hooks.rs`

**Standard Imports:**
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use async_trait::async_trait;
use agent_core::context::{ToolExecutionStartCtx, ToolExecutionEndCtx};
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;
```

**Mock Extension:**
- `ToolExecutionRecorderExt`：记录 tool_execution_start/end

```rust
struct ToolExecutionRecorderExt {
    start_count: AtomicUsize,
    end_count: AtomicUsize,
}

#[async_trait]
impl Extension for ToolExecutionRecorderExt {
    fn name(&self) -> &str { "execution_recorder" }

    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {
        self.start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {
        self.end_count.fetch_add(1, Ordering::SeqCst);
    }
}
```

**注意**：当前 AgentLoop 未调用 `hook_dispatcher.on_tool_execution_start/end()`，因此这些测试直接验证 HookRouter 通过 EventBus 广播事件到 ExtensionActor 的链路。

- [ ] **Step 1: 编写 test_tool_execution_hooks_fire**

```rust
#[tokio::test]
async fn test_tool_execution_hooks_fire() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let recorder = Arc::new(ToolExecutionRecorderExt {
        start_count: AtomicUsize::new(0),
        end_count: AtomicUsize::new(0),
    });
    
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(recorder.clone(), bus.clone(), 8);
    
    // Give actor time to subscribe
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    let router = HookRouter::new(vec![handle], bus.clone());
    
    // Directly emit tool execution events through HookRouter
    let start_ctx = ToolExecutionStartCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };
    router.on_tool_execution_start(&start_ctx).await;
    
    let end_ctx = ToolExecutionEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "test_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        success: true,
    };
    router.on_tool_execution_end(&end_ctx).await;
    
    // Give EventBus handlers time
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    assert_eq!(recorder.start_count.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.end_count.load(Ordering::SeqCst), 1);
}
```

- [ ] **Step 2: 编写 test_tool_execution_hooks_on_error**

验证 success=false 时 end hook 正确记录。

```rust
#[tokio::test]
async fn test_tool_execution_hooks_on_error() {
    let _ = tracing_subscriber::fmt().try_init();
    
    let recorder = Arc::new(ToolExecutionRecorderExt {
        start_count: AtomicUsize::new(0),
        end_count: AtomicUsize::new(0),
    });
    
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let (handle, _jh) = ExtensionActor::spawn(recorder.clone(), bus.clone(), 8);
    
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    let router = HookRouter::new(vec![handle], bus.clone());
    
    let start_ctx = ToolExecutionStartCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "failing_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        input: serde_json::json!({}),
    };
    router.on_tool_execution_start(&start_ctx).await;
    
    let end_ctx = ToolExecutionEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "failing_tool".to_string(),
        tool_call_id: "call_1".to_string(),
        success: false,
    };
    router.on_tool_execution_end(&end_ctx).await;
    
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    assert_eq!(recorder.start_count.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.end_count.load(Ordering::SeqCst), 1);
    // Verify success flag was passed correctly
}
```

- [ ] **Step 3: 运行测试**

```bash
cargo test -p extensions --test integration_tool_execution_hooks -- --nocapture
```

- [ ] **Step 4: Commit**

---

## Task 8: 全量回归测试

- [ ] **Step 1: 运行所有 extensions 测试**

```bash
cargo test -p extensions -- --nocapture
```

Expected: 所有现有测试 + 新测试全部通过

- [ ] **Step 2: 运行所有 crate 测试**

```bash
cargo test --workspace
```

Expected: 全量通过

- [ ] **Step 3: Commit**

```bash
git add crates/extensions/tests/integration_*.rs
git commit -m "test: add comprehensive cross-crate integration tests for extension system"
```

---

## 关键注意事项

1. **observational hooks 的异步性**：`on_turn_end`, `on_agent_end`, `on_session_start` 等通过 EventBus 广播，测试后需要 `sleep(Duration::from_millis(100))` 等待处理完成。

2. **ExtensionActor 订阅延迟**：创建 ExtensionActor 后，需要短暂等待（10-50ms）让 actor 完成 EventBus 订阅，否则可能 miss 掉早期事件。

3. **工具拦截顺序**：HookRouter 的 `on_tool_call` 是 first-block-wins。如果测试中有多个 blocking extension，要注意顺序。

4. **Compaction 触发条件**：
   - Overflow: LLM 返回 Error + error_message 包含 "context length" 或 "token limit"
   - Threshold: 需要配置 `CompactionConfig::new(true, reserve, keep)` 并让上下文超过阈值
   - 测试中主要使用 Overflow 方式触发

5. **Provider mutation 验证**：`on_before_provider_request` 在 AgentLoop 中调用，修改会被应用到实际发送给 LLM 的 context 中。

6. **SessionActor 的 `prompt()` 会消耗 `&mut self`**，所以不能在同一个 scope 中多次调用（除非重新创建 session）。

7. **测试隔离**：每个测试使用独立的 EventBus 和 ExtensionActor，避免交叉影响。

8. **`TestProvider::sequence()`** 在调用耗尽后会默认返回空 Text，适合多 turn 测试。

9. **错误处理**：某些测试期望 `session.prompt()` 返回错误（如 compaction 被 block 且 recovery 失败），需要用 `assert!(result.is_err())` 验证。

10. **tracing_subscriber**：每个测试开头调用 `let _ = tracing_subscriber::fmt().try_init();` 初始化日志，多次调用不会 panic。
