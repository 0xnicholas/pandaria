# agent-core 详细模块规格

> 状态: 🟡 部分实现中 | 依赖: ai-provider | 被依赖: extensions

## 模块职责

Agent loop 核心运行时。驱动 LLM tool use 协议的双层循环，管理 session 生命周期，定义 HookDispatcher 依赖反转边界，AgentEvent 事件系统。

## 引用类型速查

本节列出 spec 中引用的外部类型，来自 `ai-provider` 或 `agent-core` 已有代码。

| 类型 | 来源 | 关键字段/变体 |
|---|---|---|
| `AgentMessage` | `agent_core::types` | type alias of `llm_client::Message`; variants: `User(UserMessage)`, `Assistant(AssistantMessage)`, `ToolResult(ToolResultMessage)` |
| `AgentToolRef` | `agent_core::types` | `Arc<dyn AgentTool>` |
| `AgentTool` trait | `agent_core::types` | `name()`, `description()`, `parameters()`, `execution_mode()` → `ToolExecutionMode` (default `Parallel`), `execute(id, params, on_progress?)` → `AgentToolResult` |
| `AgentToolResult` | `agent_core::types` | `content: Vec<Content>`, `details: Option<Value>`, `is_error: bool`, `terminate: bool` |
| `AgentToolProgressUpdate` | `agent_core::types` | `content: String` — streaming progress update emitted during tool execution |
| `ToolExecutionMode` | `agent_core::types` | `Sequential`, `Parallel` (default `Parallel`) |
| `ToolCall` | `llm_client` | `id: String`, `name: String`, `arguments: Value` |
| `Content` | `llm_client` | `Text { text }`, `Image { data, mime_type }`, `Thinking { thinking, thinking_signature, redacted }`, `ToolCall(ToolCall)` |
| `ToolDef` | `llm_client` | `name`, `description`, `parameters: Value` |
| `UserMessage` | `llm_client` | `content: Vec<Content>`, `timestamp` |
| `AssistantMessage` | `llm_client` | `content: Vec<Content>`, `api`, `usage`, `stop_reason`, `response_id`, `error_message`, `timestamp` |
| `ToolResultMessage` | `llm_client` | `tool_call_id`, `tool_name`, `content`, `details`, `is_error`, `timestamp` |
| `StopReason` | `llm_client` | `Stop`, `Length`, `ToolUse`, `Error`, `Aborted` |
| `LlmContext` | `llm_client` | `system_prompt: Option<String>`, `messages: Vec<Message>`, `tools: Option<Vec<ToolDef>>` |
| `LlmProvider` | `llm_client` | `stream(model, context, options, signal)` → `AssistantMessageEventStream` |
| `StreamOptions` | `llm_client` | `max_tokens`, `temperature`, `top_p` |
| `LlmError` | `llm_client` | `RateLimited`, `Overloaded`, `InvalidRequest`, `ProviderError`, `Timeout`, `Cancelled`, `Serialization` |
| `HookDispatcher` trait | `agent_core::hook_dispatcher` | 10 methods: 2 blocking (on_tool_call, on_before_compact), 5 chain (on_tool_result, on_context, on_before_agent_start, on_before_provider_request, on_after_provider_response), 3 observational (on_turn_end, on_agent_end, on_session_start). See §9.3 |
| `HookDecision` | `agent_core::mutations` | `Continue`, `Block { reason }` |
| `ToolResultMutation` | `agent_core::mutations` | `content: Option<Vec<Content>>`, `details: Option<Value>`, `is_error: Option<bool>`, `terminate: Option<bool>` |
| `ContextMutation` | `agent_core::mutations` | `messages: Option<Vec<AgentMessage>>` |
| `BeforeAgentStartCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `system_prompt: Option<String>`, `messages: Vec<AgentMessage>`, `tools: Vec<Value>`, `model: String` |
| `BeforeAgentStartMutation` | `agent_core::mutations` | `system_prompt: Option<String>`, `messages: Option<Vec<AgentMessage>>` |
| `ProviderRequestCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `model: String`, `turn_index: u64`, `system_prompt: Option<String>`, `messages: Vec<AgentMessage>`, `tools: Option<Vec<ToolDef>>`, `options: ProviderStreamOptions` |
| `ProviderRequestMutation` | `agent_core::mutations` | `system_prompt: Option<Option<String>>`, `messages: Option<Vec<AgentMessage>>`, `tools: Option<Option<Vec<ToolDef>>>`, `options: Option<ProviderStreamOptions>` |
| `ProviderStreamOptions` | `agent_core::provider_opts` | `max_tokens: Option<u32>`, `temperature: Option<f32>`, `top_p: Option<f32>`, `reasoning: Option<ReasoningLevel>`, `max_retries: Option<u32>`, `timeout: Option<Duration>` — safe subset of StreamOptions without callbacks or secrets |
| `ProviderResponseCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `model: String`, `turn_index: u64`, `attempt: u32`, `messages_before: Vec<AgentMessage>`, `content: Vec<Content>`, `stop_reason: StopReason` |
| `ProviderResponseMutation` | `agent_core::mutations` | `content: Option<Vec<Content>>`, `stop_reason: Option<StopReason>` |
| `CompactionError` | `agent_core::error` | `AlreadyCompacted`, `LlmError(String)` |
| `AssistantMessageEvent` | `llm_client::streaming` | Stream events: Start, TextStart/Delta/End, ThinkingStart/Delta/End, ToolCallStart/Delta/End, Done, Error. See ai-provider spec §4.1 |
| `ToolExecutor` | `agent_core::tool` | `new(tenant_id, session_id, Arc<HookDispatcher>, AgentToolRef)`, `execute_tool_call(&ToolCall)` → `Result<ToolResultMessage, AgentError>` |
| `AgentError` | `agent_core::error` | `ToolNotFound`, `ToolExecutionFailed`, `HookDispatchError`, `LlmError(#[from] llm_client::LlmError)`, `LlmResponseError(String)`, `Cancelled`, `CompactionFailed` |
| `CancellationToken` | `tokio_util::sync` | `new()`, `child_token()`, `cancel()`, `is_cancelled()` |
| `ContextCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `messages: Vec<AgentMessage>` — used for `on_context` in Extension trait |
| `resolve_orphan_tool_calls` | `agent_core::loop_` | `fn(messages: &mut Vec<AgentMessage>)` — scans for unresolved `ToolCall` ids and injects synthetic error `ToolResultMessage` entries. See §2.2 step 1.5 |
| `TurnEndCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `turn_index: u64`, `messages: Vec<AgentMessage>` |
| `AgentEndCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `messages: Vec<AgentMessage>` |
| `SessionCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `system_prompt: String`, `tools: Vec<Value>` |
| `ToolCallCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `tool_name: String`, `tool_call_id: String`, `input: Value` |
| `ToolResultCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `tool_name: String`, `tool_call_id: String`, `input: Value`, `content: Vec<Content>`, `details: Option<Value>`, `is_error: bool` |
| `CompactCtx` | `agent_core::context` | `tenant_id: String`, `session_id: String`, `preparation: CompactionPreparation`, `entries: Vec<SessionEntry>`, `reason: CompactReason` |
| `CompactDecision` | `agent_core::mutations` | `Continue`, `Block { reason }`, `Replace { result: CompactionResult }` |
| `CompactReason` | `agent_core::context` | `Overflow`, `Threshold`, `Manual` |
| `SessionStore` trait | `agent_core::store` | `save_session(tenant_id, session_id, messages)`, `load_session(tenant_id, session_id)` → `Vec<AgentMessage>` — persistence boundary, implemented by persistence crate |

---

## 1. 新增类型

### 1.1 SessionEntry (`src/session_entry.rs`)

Session 是 entry 的有序序列，不只是消息的累加。引入 entry 概念支持 compaction boundary、future 的 branch point / settings change 等元数据。

```rust
use uuid::Uuid;
use std::time::SystemTime;
use crate::types::AgentMessage;

/// A single entry in the session history.
#[derive(Debug, Clone)]
pub enum SessionEntry {
    /// A standard message (user, assistant, tool result)
    Message {
        id: Uuid,
        message: AgentMessage,
    },
    /// A compaction boundary — marks where old messages were summarized.
    /// Entries before this boundary are not sent to LLM context.
    Compaction {
        id: Uuid,
        summary: String,
        first_kept_entry_id: Uuid,
        tokens_before: usize,
        details: Option<CompactionDetails>,
        from_extension: bool,
        timestamp: SystemTime,
    },
}

impl SessionEntry {
    pub fn id(&self) -> Option<Uuid> {
        match self {
            SessionEntry::Message { id, .. } => Some(*id),
            SessionEntry::Compaction { id, .. } => Some(*id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// Builds AgentMessage context from entries for LLM consumption.
/// - Skips entries before the last compaction boundary
/// - Injects compaction summary as the first message (system-like)
pub struct SessionContextBuilder;

impl SessionContextBuilder {
    pub fn build_context(entries: &[SessionEntry]) -> Vec<AgentMessage> {
        // Find last compaction boundary
        let last_compaction_idx = entries.iter().rposition(|e| matches!(e, SessionEntry::Compaction { .. }));
        let start_idx = last_compaction_idx.map(|i| i + 1).unwrap_or(0);
        
        let mut messages = Vec::new();
        
        // Inject compaction summary if exists
        if let Some(SessionEntry::Compaction { summary, .. }) = last_compaction_idx.map(|i| &entries[i]) {
            messages.push(AgentMessage::User(llm_client::UserMessage {
                content: vec![Content::Text { text: format!("[Context Summary]\n{}", summary) }],
                timestamp: SystemTime::now(),
            }));
        }
        
        // Collect messages after boundary, excluding error assistant messages
        // (error messages are kept in entries for transcript but not sent to LLM)
        for entry in &entries[start_idx..] {
            if let SessionEntry::Message { message: msg, .. } = entry {
                // Skip assistant messages with Error stop_reason
                if let AgentMessage::Assistant(assistant) = msg {
                    if assistant.stop_reason == StopReason::Error {
                        continue;
                    }
                }
                messages.push(msg.clone());
            }
        }
        
        messages
    }
}
```

### 1.2 AgentEvent 与 AgentEventListener (`src/events.rs`)

agent-core 级事件，通过 `AgentEventListener` 回调传递给订阅者。

```rust
use crate::types::AgentMessage;
use crate::error::AgentError;
use crate::context::CompactReason;
use crate::compaction::CompactionResult;
use llm_client::ToolResultMessage;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Parent AgentLoop event. Fired once per run.
    AgentStart,
    AgentEnd { messages: Vec<AgentMessage> },

    /// Turn lifecycle. Fired once per LLM round-trip.
    TurnStart { turn_index: u64 },
    TurnEnd { turn_index: u64, messages: Vec<AgentMessage> },

    /// Message streaming. MessageStart → MessageUpdate* → MessageEnd.
    MessageStart { message_index: u64 },
    MessageUpdate { message_index: u64, content_delta: String },
    MessageEnd { message: AgentMessage },

    /// Tool execution lifecycle.
    ToolExecutionStart { tool_call_id: String, tool_name: String },
    ToolExecutionUpdate { tool_call_id: String, content: String },
    ToolExecutionEnd { tool_call_id: String, result: ToolResultMessage },

    /// Compaction lifecycle.
    CompactionStart { reason: CompactReason },
    CompactionEnd {
        reason: CompactReason,
        result: Option<CompactionResult>,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },

    /// Auto-retry after error.
    AutoRetryStart { attempt: u32, max_attempts: u32, delay_ms: u64 },
    AutoRetryEnd { success: bool, error: Option<String> },

    /// Fatal error that terminated the agent loop.
    Error { error: AgentError },
}
```

```rust
use async_trait::async_trait;

#[async_trait]
pub trait AgentEventListener: Send + Sync {
    async fn on_event(&self, event: &AgentEvent);
}
```

### 1.3 AgentLoopConfig (`src/loop.rs`)

```rust
use std::sync::{Arc, Mutex};

pub struct AgentLoopConfig {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub provider: Arc<dyn LlmProvider>,
    pub hook_dispatcher: Arc<dyn HookDispatcher>,
    pub tools: Vec<AgentToolRef>,
    pub system_prompt: Option<String>,
    pub stream_options: StreamOptions,
    pub max_retries: u32,
    /// Shared queue — drained before each inner turn loop
    pub steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    /// Shared queue — injected after inner turn loop stops
    pub follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    /// Callback to emit AgentEvent to SessionActor's event queue
    pub event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync>,
}
```

### 1.4 新增 Context/Mutation 类型

The following types are referenced throughout AgentLoop and SessionActor pseudocode.
Full `pub struct`/`pub enum` definitions correspond to the reference table at §0.

```rust
// ——— before_agent_start ———
pub struct BeforeAgentStartCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<serde_json::Value>,
    pub model: String,
}

pub struct BeforeAgentStartMutation {
    pub system_prompt: Option<String>,
    pub messages: Option<Vec<AgentMessage>>,
}

// ——— provider_request ———
pub struct ProviderRequestCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub turn_index: u64,
    pub system_prompt: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub tools: Option<Vec<llm_client::ToolDef>>,
    pub options: ProviderStreamOptions,
}

pub struct ProviderRequestMutation {
    pub system_prompt: Option<Option<String>>,
    pub messages: Option<Vec<AgentMessage>>,
    pub tools: Option<Option<Vec<llm_client::ToolDef>>>,
    pub options: Option<ProviderStreamOptions>,
}

// ——— provider_response ———
pub struct ProviderResponseCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub turn_index: u64,
    pub attempt: u32,
    pub messages_before: Vec<AgentMessage>,
    pub content: Vec<llm_client::Content>,
    pub stop_reason: llm_client::StopReason,
}

pub struct ProviderResponseMutation {
    pub content: Option<Vec<llm_client::Content>>,
    pub stop_reason: Option<llm_client::StopReason>,
}

// ——— compaction ———
pub struct CompactCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub preparation: CompactionPreparation,
    pub entries: Vec<SessionEntry>,
    pub reason: CompactReason,
}

pub enum CompactDecision {
    Continue,
    Block { reason: String },
    Replace { result: CompactionResult },
}

pub enum CompactReason {
    Overflow,
    Threshold,
    Manual,
}

// ——— error ———
pub enum CompactionError {
    AlreadyCompacted,
    LlmError(String),
}

// ——— safe subset of StreamOptions for ProviderRequestMutation ———
pub struct ProviderStreamOptions {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub reasoning: Option<llm_client::ReasoningLevel>,
    pub max_retries: Option<u32>,
    pub timeout: Option<std::time::Duration>,
}

impl ProviderStreamOptions {
    pub fn from(options: &StreamOptions) -> Self { /* omit callbacks + secrets */ }
}
```

### 1.6 辅助函数

```rust
/// Apply ProviderRequestMutation to LlmContext + StreamOptions in-place.
/// Only `Some` fields are applied; `None` fields leave the current value unchanged.
/// `system_prompt: Some(None)` clears the system prompt.
fn apply_provider_request_mutation(
    ctx: &mut LlmContext,
    opts: &mut StreamOptions,
    mutation: ProviderRequestMutation,
);

/// Apply ProviderResponseMutation to AssistantMessage in-place.
fn apply_provider_response_mutation(
    msg: &mut AssistantMessage,
    mutation: ProviderResponseMutation,
);

/// Build tool value definitions from AgentToolRef slice.
/// Converts `AgentTool` trait objects to `serde_json::Value` for SessionCtx.
fn build_tool_value_defs(tools: &[AgentToolRef]) -> Vec<serde_json::Value>;
```

---

## 2. AgentLoop 双层循环

### 2.1 外层 — follow-up

```
run(config, initial_messages, signal) → Result<Vec<AgentMessage>, AgentError>

// 0. before_agent_start hook (chain — each handler can inject messages, override system_prompt)
let agent_start_ctx = BeforeAgentStartCtx {
    system_prompt: config.system_prompt.clone(),
    messages: initial_messages,
    tools: build_tool_value_defs(&config.tools),
    model: config.model.clone(),
};
let agent_start_mutation = config.hook_dispatcher.on_before_agent_start(&agent_start_ctx).await;
let system_prompt = agent_start_mutation.system_prompt.or(config.system_prompt);
let mut messages = agent_start_mutation.messages.unwrap_or(initial_messages);
let mut new_messages = Vec::new();
let mut turn_index: u64 = 0;
let mut message_index: u64 = 0;

(config.event_sink)(AgentEvent::AgentStart)

loop {
  // —— drain steer queue ——
  {
    let mut q = config.steer_queue.lock().expect("steer queue poisoned");
    messages.extend(q.drain(..));
  }

  // —— inner turn loop ——
  loop {
    result = run_turn(&mut messages, &mut new_messages, &mut turn_index,
                      &mut message_index, &system_prompt, &config, signal)
    match result {
      TurnResult::ToolUse => continue,
      TurnResult::Stop => break,
      TurnResult::Error(e) => {
        (config.event_sink)(AgentEvent::Error { error: e.clone() })
        return Err(e)
      }
    }
  }

  // —— drain follow_up queue ——
  {
    let mut q = config.follow_up_queue.lock().expect("follow_up queue poisoned");
    let follow_ups: Vec<_> = q.drain(..).collect();
    if follow_ups.is_empty() { break }
    messages.extend(follow_ups);
    new_messages.extend(follow_ups);
    continue
  }
}

(config.event_sink)(AgentEvent::AgentEnd { messages })
config.hook_dispatcher.on_agent_end(&AgentEndCtx {
    tenant_id: config.tenant_id.clone(),
    session_id: config.session_id.clone(),
    messages: messages.clone(),
}).await;
return Ok(new_messages)
```

### 2.2 内层 — turn

```
run_turn(messages, new_messages, turn_index, message_index, system_prompt, config, signal) → TurnResult

*turn_index += 1

(config.event_sink)(AgentEvent::TurnStart { turn_index: *turn_index })

// 1. Transform context via hook chain
let after_context_messages = messages.clone();  // snapshot for after_provider_response
let ctx_ctx = ContextCtx { messages: messages.clone() };
let mutation = config.hook_dispatcher.on_context(&ctx_ctx).await;
let mut transformed = mutation.messages.unwrap_or_else(|| messages.clone());

// 1.5. Synthesize orphan tool results (NEW — pi.dev alignment)
// When session history contains unresolved ToolCalls (e.g. after restore or
// cross-provider replay), inject synthetic error ToolResultMessages before
// sending to LLM. Without this step, LLM rejects the context with
// "messages must alternate user/assistant/tool".
resolve_orphan_tool_calls(&mut transformed);

// 2. Build LlmContext
let mut stream_opts = config.stream_options.clone();
ctx = LlmContext {
  system_prompt: system_prompt.clone(),
  messages: transformed,
  tools: build_tool_defs(&config.tools),
}

// 2.5 before_provider_request (chain — extensions can modify system_prompt, messages, tools, options)
let provider_req_ctx = ProviderRequestCtx {
  tenant_id: config.tenant_id.clone(),
  session_id: config.session_id.clone(),
  model: config.model.clone(),
  turn_index: *turn_index,
  system_prompt: ctx.system_prompt.clone(),
  messages: ctx.messages.clone(),
  tools: ctx.tools.clone(),
  options: ProviderStreamOptions::from(&config.stream_options),
};
let provider_req_mutation = config.hook_dispatcher.on_before_provider_request(&provider_req_ctx).await;
apply_provider_request_mutation(&mut ctx, &mut stream_opts, provider_req_mutation);

// 2.6 Cross-provider message normalization (ai-provider::transform_messages)
// Ensures message format compatibility when switching between providers
// (tool_call_id truncation, image downgrade, thinking-block stripping, orphan tool call padding)
let model = get_model(config.provider.provider_name(), &config.model);
let transform_opts = llm_client::TransformOptions {
    target_api: model.map(|m| m.api.clone()).unwrap_or_default(),
    supports_images: model.map(|m| m.input_modalities.contains(&llm_client::Modality::Image)).unwrap_or(false),
    preserve_thinking: false, // v0.1: strip thinking blocks for safety
};
ctx.messages = llm_client::transform_messages(&ctx.messages, &transform_opts);

// 3. Call LLM with retry
(retry_count, assistant_msg) = call_llm_with_retry(ctx, stream_opts, config, *message_index, signal)?

// 3.5 after_provider_response (chain — extensions can modify content / stop_reason, e.g. content safety)
// NOTE: this fires AFTER stream consumption and BEFORE tool call extraction.
// Extensions that modify assistant_msg.content will affect which tool calls are extracted.
let provider_resp_ctx = ProviderResponseCtx {
  tenant_id: config.tenant_id.clone(),
  session_id: config.session_id.clone(),
  model: config.model.clone(),
  turn_index: *turn_index,
  attempt: retry_count,
  messages_before: after_context_messages,
  content: assistant_msg.content.clone(),
  stop_reason: assistant_msg.stop_reason.clone(),
};
let provider_resp_mutation = config.hook_dispatcher.on_after_provider_response(&provider_resp_ctx).await;
apply_provider_response_mutation(&mut assistant_msg, provider_resp_mutation);

// 4. Emit message_end
*message_index += 1
(config.event_sink)(AgentEvent::MessageEnd { message: assistant_msg.clone() })
new_messages.push(assistant_msg.clone())
messages.push(assistant_msg.clone())

// 5. Extract ToolCalls
tool_calls = extract_tool_calls(&assistant_msg.content)
if tool_calls.is_empty() {
  // Check for error stop reasons first — propagate as Err to SessionActor for retry logic
  match assistant_msg.stop_reason {
    StopReason::Error | StopReason::Aborted | StopReason::Length => {
      let err_msg = assistant_msg.error_message.clone()
        .unwrap_or_else(|| format!("LLM returned stop reason: {:?}", assistant_msg.stop_reason));
      error!(
        tenant_id = %config.tenant_id,
        session_id = %config.session_id,
        turn = *turn_index,
        stop_reason = ?assistant_msg.stop_reason,
        error = %err_msg,
        "LLM response error",
      );
      let turn_ctx = TurnEndCtx {
        tenant_id: config.tenant_id.clone(),
        session_id: config.session_id.clone(),
        turn_index: *turn_index,
        messages: messages.clone(),
      };
      config.hook_dispatcher.on_turn_end(&turn_ctx).await;
      return TurnResult::Error(AgentError::LlmResponseError(err_msg))
    }
    _ => {}
  }
  (config.event_sink)(AgentEvent::TurnEnd { turn_index: *turn_index, messages: messages.clone() })
  config.hook_dispatcher.on_turn_end(&TurnEndCtx {
      tenant_id: config.tenant_id.clone(),
      session_id: config.session_id.clone(),
      turn_index: *turn_index, messages: messages.clone()
  }).await;
  return TurnResult::Stop
}

// 6. Execute tools (partitioned by mode)
tool_results = execute_tools(tool_calls, config, signal).await
let mut all_terminate = !tool_results.is_empty();
for result in &tool_results {
  new_messages.push(AgentMessage::ToolResult(result.clone()))
  messages.push(AgentMessage::ToolResult(result.clone()))
  let terminated = result.details.as_ref()
    .and_then(|d| d.get("_terminate"))
    .and_then(|v| v.as_bool())
    .unwrap_or(false);
  if !terminated { all_terminate = false; }
}

(config.event_sink)(AgentEvent::TurnEnd { turn_index: *turn_index, messages: messages.clone() })
config.hook_dispatcher.on_turn_end(&TurnEndCtx {
    tenant_id: config.tenant_id.clone(),
    session_id: config.session_id.clone(),
    turn_index: *turn_index, messages: messages.clone()
}).await;

if all_terminate {
  return TurnResult::Stop
}

if assistant_msg.stop_reason == StopReason::ToolUse {
  return TurnResult::ToolUse
} else {
  // Check for error stop reasons after tool execution — failures here
  // should propagate to SessionActor for retry/compaction logic
  if assistant_msg.stop_reason == StopReason::Error
      || assistant_msg.stop_reason == StopReason::Aborted
      || assistant_msg.stop_reason == StopReason::Length
  {
      let err_msg = assistant_msg.error_message.clone()
          .unwrap_or_else(|| format!("LLM returned stop reason: {:?}", assistant_msg.stop_reason));
      error!(
        tenant_id = %config.tenant_id,
        session_id = %config.session_id,
        turn = *turn_index,
        stop_reason = ?assistant_msg.stop_reason,
        error = %err_msg,
        "LLM response error after tool execution",
      );
      return TurnResult::Error(AgentError::LlmResponseError(err_msg))
  }
  return TurnResult::Stop
}
```

### 2.2.1 孤儿 Tool Call 合成 (`resolve_orphan_tool_calls`)

对标 pi.dev `transform-messages.ts:158-188`。在构建 LLM 请求前，扫
描历史消息中未 resolve 的 `ToolCall`，注入合成错误 `ToolResultMessage`。

```rust
/// Scan messages for ToolCall IDs that lack a corresponding ToolResultMessage,
/// and inject synthetic error ToolResultMessages.
///
/// Algorithm:
/// 1. Collect all ToolCall IDs from AssistantMessage content blocks
/// 2. Collect all tool_call_ids from ToolResultMessages
/// 3. For each orphan ID in (1) - (2):
///    - Find the AssistantMessage that contains the orphan ToolCall
///    - Insert a synthetic ToolResultMessage immediately after it
///    - Mark details with `_orphan: true` for audit trail
///
/// Orphan tool calls occur when:
///   - Session history is restored from persistence after a mid-turn abort
///   - Context was truncated (compaction removed tool results)
///   - Messages are replayed on a different provider that drops results
fn resolve_orphan_tool_calls(messages: &mut Vec<AgentMessage>) {
    use std::collections::HashSet;

    let mut tool_call_ids: Vec<(usize, String)> = Vec::new();
    let mut resolved_ids: HashSet<String> = HashSet::new();

    for (i, msg) in messages.iter().enumerate() {
        match msg {
            AgentMessage::Assistant(a) => {
                for content in &a.content {
                    if let Content::ToolCall(tc) = content {
                        tool_call_ids.push((i, tc.id.clone()));
                    }
                }
            }
            AgentMessage::ToolResult(tr) => {
                resolved_ids.insert(tr.tool_call_id.clone());
            }
            _ => {}
        }
    }

    // Inject synthetic results for orphans, iterating in reverse
    // to preserve insertion indices.
    let mut orphans: Vec<(usize, String, String)> = tool_call_ids
        .into_iter()
        .filter(|(_, id)| !resolved_ids.contains(id))
        .map(|(idx, id)| {
            // Find the tool name from the AssistantMessage via match arm
            // (AgentMessage is a llm_client::Message enum; no .as_assistant() helper exists)
            let tool_name = match &messages[idx] {
                AgentMessage::Assistant(a) => a.content.iter().find_map(|c| match c {
                    Content::ToolCall(tc) if tc.id == id => Some(tc.name.clone()),
                    _ => None,
                }),
                _ => None,
            }
            .unwrap_or_else(|| "unknown".to_string());
            (idx, id, tool_name)
        })
        .collect();

    // Insert in reverse index order to avoid shifting.
    // Note: when multiple orphans exist in the same AssistantMessage,
    // their synthetic results will be inserted in reversed order relative
    // to each other. This is acceptable — matching is by tool_call_id,
    // not position within the assistant message.
    orphans.sort_by(|a, b| b.0.cmp(&a.0));
    for (idx, id, tool_name) in orphans {
        let result_msg = AgentMessage::ToolResult(llm_client::ToolResultMessage {
            tool_call_id: id.clone(),
            tool_name,
            content: vec![],
            details: Some(serde_json::json!({
                "_orphan": true,
                "message": "tool call was not executed (context truncated or restored)"
            })),
            is_error: true,
            timestamp: std::time::SystemTime::now(),
        });
        messages.insert(idx + 1, result_msg);
    }
}
```

| 决策 | 理由 |
|---|---|
| 在 `on_context` 之后执行 | Extension 的 context mutation 可能引入/解决孤儿，需在清理前完成 |
| 不通过 Extension 实现 | 正确性保证，不是可选行为。Core 层确保所有 session 受益 |
| 合成结果标记 `_orphan: true` | 审计和 debug 用，区别于正常 tool not found |
| 插入位置：孤儿 AssistantMessage 之后 | 保持 `user → assistant → tool result` 的交替模式 |

### 2.3 LLM 调用 + 指数退避重试

```
call_llm_with_retry(ctx: LlmContext, stream_opts: StreamOptions, config: &AgentLoopConfig, message_index: u64, signal: &CancellationToken) → Result<(u32, AssistantMessage), AgentError>

for attempt in 0..config.max_retries:
  result = config.provider.stream(model, ctx.clone(), stream_opts.clone(), signal.child_token())

  match result:
    Ok(stream) => {
      (config.event_sink)(AgentEvent::MessageStart { message_index })
      for each delta in stream:
        match delta:
          TextDelta(delta) => {
            (config.event_sink)(AgentEvent::MessageUpdate { message_index, content_delta: delta })
            accumulate delta into text_accum[content_index]
          }
          TextEnd { content_index, text } =>
            push Content::Text { text: accumulated_or_text, text_signature: None } to content
          ThinkingDelta { content_index, delta } =>
            accumulate delta into thinking_accum[content_index]
          ThinkingEnd { content_index, thinking } =>
            // v0.2: populate thinking_signature from provider stream (Done event message)
            // v0.2: detect redacted=false from provider event type (Anthropic Thinking vs RedactedThinking)
            push Content::Thinking { thinking: accumulated_or_thinking, thinking_signature: None, redacted: false } to content
          ToolCallDelta(tc) => accumulate tc into content
          Done { reason, message } => return Ok((attempt, message))
          Error { error } => return Err(AgentError::LlmError(...))
      }
      return Err(AgentError::LlmError("stream ended without terminal event"))
    }
    Err(RateLimited | Overloaded) if attempt < config.max_retries - 1 => {
      sleep(2_u64.pow(attempt) * 100ms)
      continue
    }
    Err(other) => return Err(other.into())
return Err(AgentError::LlmError("all retries exhausted"))
```

---

## 3. 并行工具执行

```rust
async fn execute_tools(
    tool_calls: Vec<ToolCall>,
    config: &AgentLoopConfig,
    signal: &CancellationToken,
) -> Vec<ToolResultMessage> {
    let mut results = Vec::new();

    let (sequential_calls, parallel_calls): (Vec<_>, Vec<_>) = tool_calls
        .into_iter()
        .partition(|tc| {
            config.tools.iter()
                .find(|t| t.name() == tc.name)
                .map(|t| t.execution_mode() == ToolExecutionMode::Sequential)
                .unwrap_or(true)
        });

    for tc in sequential_calls {
        let result = execute_single_tool(&tc, config, signal).await;
        results.push(result);
    }

    if !parallel_calls.is_empty() {
        let futures: Vec<_> = parallel_calls.iter().map(|tc| {
            execute_single_tool(tc, config, signal)
        }).collect();
        let parallel_results = futures::future::join_all(futures).await;
        results.extend(parallel_results);
    }

    results
}

async fn execute_single_tool(
    tc: &ToolCall,
    config: &AgentLoopConfig,
    signal: &CancellationToken,
) -> ToolResultMessage {
    (config.event_sink)(AgentEvent::ToolExecutionStart { tool_call_id: tc.id.clone(), tool_name: tc.name.clone() });

    let on_progress = |update: AgentToolProgressUpdate| {
        (config.event_sink)(AgentEvent::ToolExecutionUpdate {
            tool_call_id: tc.id.clone(),
            content: update.content.clone(),
        });
    };

    let tool = config.tools.iter().find(|t| t.name() == tc.name).cloned();
    let result = match tool {
        Some(tool) => {
            let executor = ToolExecutor::new(
                config.tenant_id.clone(),
                config.session_id.clone(),
                config.hook_dispatcher.clone(),
                tool,
            );
            executor.execute_tool_call(tc, Some(&on_progress)).await
        }
        None => Err(AgentError::ToolNotFound(tc.name.clone())),
    };

    let result_msg = match result {
        Ok(msg) => msg,
        Err(e) => ToolResultMessage {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            content: vec![],
            details: Some(serde_json::json!({"error": e.to_string()})),
            is_error: true,
            timestamp: SystemTime::now(),
        },
    };

    (config.event_sink)(AgentEvent::ToolExecutionEnd { tool_call_id: tc.id.clone(), result: result_msg.clone() });
    result_msg
}
```

---

## 4. ToolExecutor (`src/tool.rs`)

```rust
use crate::hook_dispatcher::HookDispatcher;

pub struct ToolExecutor {
    tenant_id: String,
    session_id: String,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    tool: AgentToolRef,
}

impl ToolExecutor {
    pub fn new(
        tenant_id: String,
        session_id: String,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        tool: AgentToolRef,
    ) -> Self;

    /// Execute a tool call through the full pipeline:
    /// 1. Build ToolCallCtx and invoke on_tool_call hooks (blocking — first-block-wins)
    /// 2. If not blocked: spawn tool execution via tokio::task::spawn_blocking
    ///    - Tool progress updates emitted via AgentEvent::ToolExecutionUpdate
    /// 3. Build ToolResultCtx and invoke on_tool_result hooks (chaining)
    /// 4. Apply mutations: content, details, is_error, terminate
    /// 5. Embed `_terminate` flag into details JSON
    ///
    /// Hook timeouts: 500ms for blocking/chain (on_tool_call, on_tool_result).
    pub async fn execute_tool_call(
        &self,
        tc: &ToolCall,
        on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    ) -> Result<ToolResultMessage, AgentError>;
}
```

---

## 5. FileOperationExtractor (`src/file_ops.rs`)

从 assistant message 的 tool call 中提取文件操作记录，供 compaction 使用。

```rust
/// Tracks file operations observed in a conversation segment.
#[derive(Debug, Default, Clone)]
pub struct FileOperations {
    pub read: Vec<String>,
    pub written: Vec<String>,
    pub edited: Vec<String>,
}

/// Extracts file operations from assistant tool calls.
pub trait FileOperationExtractor: Send + Sync {
    fn extract(&self, messages: &[AgentMessage]) -> FileOperations;
}

/// Default implementation based on tool name matching.
pub struct DefaultFileOperationExtractor {
    read_tool_names: Vec<String>,
    write_tool_names: Vec<String>,
    edit_tool_names: Vec<String>,
    path_arg_name: String,
}

impl Default for DefaultFileOperationExtractor {
    fn default() -> Self {
        Self {
            read_tool_names: vec!["read".to_string()],
            write_tool_names: vec!["write".to_string()],
            edit_tool_names: vec!["edit".to_string()],
            path_arg_name: "path".to_string(),
        }
    }
}

impl FileOperationExtractor for DefaultFileOperationExtractor {
    fn extract(&self, messages: &[AgentMessage]) -> FileOperations {
        let mut ops = FileOperations::default();
        
        for msg in messages {
            if let AgentMessage::Assistant(assistant) = msg {
                for content in &assistant.content {
                    if let Content::ToolCall(tc) = content {
                        let path = tc.arguments
                            .get(&self.path_arg_name)
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        
                        if let Some(path) = path {
                            if self.read_tool_names.contains(&tc.name) {
                                ops.read.push(path);
                            } else if self.write_tool_names.contains(&tc.name) {
                                ops.written.push(path);
                            } else if self.edit_tool_names.contains(&tc.name) {
                                ops.edited.push(path);
                            }
                        }
                    }
                }
            }
        }
        
        // Deduplicate and sort
        ops.read.sort_unstable();
        ops.read.dedup();
        ops.written.sort_unstable();
        ops.written.dedup();
        ops.edited.sort_unstable();
        ops.edited.dedup();
        
        ops
    }
}
```

---

## 6. CompactionActor (`src/compaction.rs`)

对标 pi.dev 的总结式 compaction。将旧消息通过 LLM 生成结构化摘要，释放 context window。

### 6.1 配置与结构

```rust
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use llm_client::{LlmProvider, LlmContext, StreamOptions};
use crate::types::AgentMessage;
use crate::session_entry::{SessionEntry, CompactionDetails};
use crate::file_ops::{FileOperationExtractor, FileOperations};

pub struct CompactionConfig {
    pub enabled: bool,
    pub reserve_tokens: usize,      // 给 summary 保留的预算，默认 16384
    pub keep_recent_tokens: usize,  // 保留的最新消息 token 数，默认 20000
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16384,
            keep_recent_tokens: 20000,
        }
    }
}

pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: uuid::Uuid,
    pub tokens_before: usize,
    pub details: Option<CompactionDetails>,
}

pub struct CompactionPreparation {
    pub first_kept_entry_id: uuid::Uuid,
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: usize,
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
}

pub struct CompactionActor {
    config: CompactionConfig,
    provider: Arc<dyn LlmProvider>,
    model: String,
    file_op_extractor: Arc<dyn FileOperationExtractor>,
}

impl CompactionActor {
    pub fn new(
        config: CompactionConfig,
        provider: Arc<dyn LlmProvider>,
        model: String,
        file_op_extractor: Arc<dyn FileOperationExtractor>,
    ) -> Self;
}
```

### 6.2 核心方法

```rust
impl CompactionActor {
    /// Full compaction pipeline.
    pub async fn compact(
        &self,
        entries: &[SessionEntry],
        signal: &CancellationToken,
    ) -> Result<CompactionResult, CompactionError>;

    /// Step 1: Prepare — find cut point and collect messages to summarize.
    fn prepare(&self, entries: &[SessionEntry]) -> Result<CompactionPreparation, CompactionError>;

    /// Step 2: Generate summary via LLM.
    async fn generate_summary(
        &self,
        preparation: &CompactionPreparation,
        signal: &CancellationToken,
    ) -> Result<String, CompactionError>;
}
```

### 6.3 Token 估算

```rust
fn estimate_tokens(message: &AgentMessage) -> usize {
    let chars = match message {
        AgentMessage::User(user) => {
            user.content.iter().map(|c| match c {
                Content::Text { text } => text.len(),
                Content::Image { .. } => 4800, // ≈ 1200 tokens
                _ => 0,
            }).sum()
        }
        AgentMessage::Assistant(assistant) => {
            assistant.content.iter().map(|c| match c {
                Content::Text { text } => text.len(),
                Content::Thinking { thinking } => thinking.len(),
                Content::ToolCall(tc) => tc.name.len() + serde_json::to_string(&tc.arguments).unwrap_or_default().len(),
                Content::Image { .. } => 4800,
            }).sum()
        }
        AgentMessage::ToolResult(result) => {
            result.content.iter().map(|c| match c {
                Content::Text { text } => text.len(),
                Content::Image { .. } => 4800,
                _ => 0,
            }).sum()
        }
    };
    (chars as f64 / 4.0).ceil() as usize
}

fn estimate_context_tokens(entries: &[SessionEntry]) -> usize {
    // Priority: use last assistant message's usage.totalTokens if available
    let mut tokens = 0;
    let mut last_usage_tokens: Option<usize> = None;
    let mut last_usage_idx: Option<usize> = None;
    
    for (i, entry) in entries.iter().enumerate() {
        if let SessionEntry::Message { message: AgentMessage::Assistant(assistant), .. } = entry {
            if assistant.stop_reason != StopReason::Aborted && assistant.stop_reason != StopReason::Error {
                if let Some(usage) = &assistant.usage {
                    last_usage_tokens = Some(usage.total_tokens as usize);
                    last_usage_idx = Some(i);
                }
            }
        }
    }
    
    if let Some(usage_tokens) = last_usage_tokens {
        tokens = usage_tokens;
        // Add trailing messages after last usage
        if let Some(idx) = last_usage_idx {
            for entry in &entries[idx + 1..] {
                if let SessionEntry::Message { message: msg, .. } = entry {
                    tokens += estimate_tokens(msg);
                }
            }
        }
    } else {
        // No usage data — estimate all
        for entry in entries {
            if let SessionEntry::Message { message: msg, .. } = entry {
                tokens += estimate_tokens(msg);
            }
        }
    }
    
    tokens
}
```

### 6.4 Cut Point 算法

核心逻辑：倒序累计 token，找到超过 `keep_recent_tokens` 的位置，前向对齐到合法 cut point。

```rust
#[derive(Debug)]
struct CutPoint {
    first_kept_entry_index: usize,
    turn_start_index: Option<usize>,
    is_split_turn: bool,
}

fn find_cut_point(
    entries: &[SessionEntry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: usize,
) -> CutPoint {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);
    
    if cut_points.is_empty() {
        return CutPoint {
            first_kept_entry_index: start_index,
            turn_start_index: None,
            is_split_turn: false,
        };
    }
    
    // 1. Accumulate tokens from newest to oldest
    let mut accumulated = 0;
    let mut cut_index = cut_points[0];
    
    for i in (start_index..end_index).rev() {
        if let SessionEntry::Message { message: msg, .. } = &entries[i] {
            accumulated += estimate_tokens(msg);
            
            // 2. Stop when exceeding keep_recent_tokens
            if accumulated >= keep_recent_tokens {
                // 3. Find nearest valid cut point >= current position
                cut_index = cut_points.iter()
                    .find(|&&cp| cp >= i)
                    .copied()
                    .unwrap_or(cut_points[0]);
                break;
            }
        }
    }
    
    // 4. Absorb adjacent non-message entries (future-proofing for new entry types)
    while cut_index > start_index {
        match &entries[cut_index - 1] {
            SessionEntry::Compaction { .. } => break,
            SessionEntry::Message { .. } => break,
            // Future entry types (e.g. settings change) are absorbed into keep region
            _ => cut_index -= 1,
        }
    }
    
    // 5. Detect mid-turn split
    let is_user_msg = matches!(
        &entries[cut_index],
        SessionEntry::Message { message: AgentMessage::User(_), .. }
    );
    
    let turn_start_index = if is_user_msg {
        None
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };
    
    CutPoint {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !is_user_msg && turn_start_index.is_some(),
    }
}

fn find_valid_cut_points(entries: &[SessionEntry], start: usize, end: usize) -> Vec<usize> {
    let mut points = Vec::new();
    for i in start..end {
        match &entries[i] {
            SessionEntry::Message { message: msg, .. } => match msg {
                AgentMessage::User(_) | AgentMessage::Assistant(_) => points.push(i),
                AgentMessage::ToolResult(_) => {} // Never cut on tool result
            },
            _ => {} // Non-message entries are not valid cut points themselves
        }
    }
    points
}

fn find_turn_start_index(entries: &[SessionEntry], entry_index: usize, start: usize) -> Option<usize> {
    for i in (start..=entry_index).rev() {
        match &entries[i] {
            SessionEntry::Message { message: AgentMessage::User(_), .. } => return Some(i),
            _ => {}
        }
    }
    None
}
```

### 6.5 Prepare 方法

```rust
fn prepare(&self, entries: &[SessionEntry]) -> Result<CompactionPreparation, CompactionError> {
    // 1. If last entry is compaction, skip (already compacted)
    if let Some(SessionEntry::Compaction { .. }) = entries.last() {
        return Err(CompactionError::AlreadyCompacted);
    }
    
    // 2. Find previous compaction
    let prev_compaction_idx = entries.iter().rposition(|e| matches!(e, SessionEntry::Compaction { .. }));
    let mut previous_summary = None;
    let mut boundary_start = 0;
    
    if let Some(idx) = prev_compaction_idx {
        if let SessionEntry::Compaction { summary, first_kept_entry_id, .. } = &entries[idx] {
            previous_summary = Some(summary.clone());
            // Find first_kept_entry_id position (it's a Message ID)
            boundary_start = entries.iter().position(|e| {
                matches!(e, SessionEntry::Message { id, .. } if id == first_kept_entry_id)
            }).unwrap_or(idx + 1);
        }
    }
    
    let boundary_end = entries.len();
    let tokens_before = estimate_context_tokens(entries);
    
    // 3. Find cut point
    let cut_point = find_cut_point(entries, boundary_start, boundary_end, self.config.keep_recent_tokens);
    
    // 4. Determine history end
    let history_end = if cut_point.is_split_turn {
        cut_point.turn_start_index.unwrap_or(cut_point.first_kept_entry_index)
    } else {
        cut_point.first_kept_entry_index
    };
    
    // 5. Collect messages to summarize (will be discarded)
    let mut messages_to_summarize = Vec::new();
    for i in boundary_start..history_end {
        if let SessionEntry::Message { message: msg, .. } = &entries[i] {
            messages_to_summarize.push(msg.clone());
        }
    }
    
    // 6. Collect turn prefix messages (if mid-turn split)
    let mut turn_prefix_messages = Vec::new();
    if cut_point.is_split_turn {
        for i in cut_point.turn_start_index.unwrap()..cut_point.first_kept_entry_index {
            if let SessionEntry::Message { message: msg, .. } = &entries[i] {
                turn_prefix_messages.push(msg.clone());
            }
        }
    }
    
    // 7. Extract file operations
    let file_ops = self.file_op_extractor.extract(&messages_to_summarize);
    
    // 8. Get the ID of the first kept message entry
    let first_kept_entry_id = entries[cut_point.first_kept_entry_index]
        .id()
        .unwrap_or_else(uuid::Uuid::new_v4);
    
    Ok(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        is_split_turn: cut_point.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
    })
}
```

### 6.6 Summary 生成（LLM 调用）

```rust
async fn generate_summary(
    &self,
    preparation: &CompactionPreparation,
    signal: &CancellationToken,
) -> Result<String, CompactionError> {
    let max_tokens = (self.config.reserve_tokens as f64 * 0.8) as usize;
    
    if preparation.is_split_turn && !preparation.turn_prefix_messages.is_empty() {
        // Parallel generation: history summary + turn prefix summary
        let history_future = self.generate_history_summary(
            &preparation.messages_to_summarize,
            preparation.previous_summary.clone(),
            max_tokens,
            signal,
        );
        
        let prefix_future = self.generate_turn_prefix_summary(
            &preparation.turn_prefix_messages,
            (self.config.reserve_tokens as f64 * 0.5) as usize,
            signal,
        );
        
        let (history_result, prefix_result) = tokio::try_join!(history_future, prefix_future)?;
        
        Ok(format!(
            "{}\n\n---\n\n**Turn Context (split turn):**\n\n{}",
            history_result, prefix_result
        ))
    } else {
        // Single summary
        self.generate_history_summary(
            &preparation.messages_to_summarize,
            preparation.previous_summary.clone(),
            max_tokens,
            signal,
        ).await
    }
}

async fn generate_history_summary(
    &self,
    messages: &[AgentMessage],
    previous_summary: Option<String>,
    max_tokens: usize,
    signal: &CancellationToken,
) -> Result<String, CompactionError> {
    let base_prompt = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT
    } else {
        SUMMARIZATION_PROMPT
    };
    
    let conversation_text = serialize_messages(messages);
    
    let mut prompt_text = format!("<conversation>\n{}\n</conversation>\n\n", conversation_text);
    if let Some(prev) = previous_summary {
        prompt_text.push_str(&format!("<previous-summary>\n{}\n</previous-summary>\n\n", prev));
    }
    prompt_text.push_str(base_prompt);
    
    let llm_messages = vec![llm_client::Message::User(llm_client::UserMessage {
        content: vec![Content::Text { text: prompt_text }],
        timestamp: SystemTime::now(),
    })];
    
    let ctx = LlmContext {
        system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages: llm_messages,
        tools: None,
    };
    
    let mut stream = self.provider.stream(
        &self.model,
        ctx,
        StreamOptions { max_tokens: Some(max_tokens as u32), ..Default::default() },
        signal.child_token(),
    ).await.map_err(|e| CompactionError::LlmError(e.to_string()))?;
    
    // Consume stream to get final message
    let mut summary_text = String::new();
    
    while let Some(event) = stream.next().await {
        match event {
            Ok(AssistantMessageEvent::TextDelta { delta, .. }) => {
                summary_text.push_str(&delta);
            }
            Ok(AssistantMessageEvent::Done { message, .. }) => {
                // Extract text content from final message
                summary_text = message.content.iter()
                    .filter_map(|c| match c {
                        Content::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                break;
            }
            Ok(AssistantMessageEvent::Error { error, .. }) => {
                return Err(CompactionError::LlmError(error.to_string()));
            }
            _ => {}
        }
    }
    
    if summary_text.is_empty() {
        return Err(CompactionError::LlmError("Summary generation returned empty text".into()));
    }
    
    Ok(summary_text)
}

async fn generate_turn_prefix_summary(
    &self,
    messages: &[AgentMessage],
    max_tokens: usize,
    signal: &CancellationToken,
) -> Result<String, CompactionError> {
    let conversation_text = serialize_messages(messages);
    let prompt_text = format!(
        "<conversation>\n{}\n</conversation>\n\n{}",
        conversation_text, TURN_PREFIX_SUMMARIZATION_PROMPT
    );
    
    let llm_messages = vec![llm_client::Message::User(llm_client::UserMessage {
        content: vec![Content::Text { text: prompt_text }],
        timestamp: SystemTime::now(),
    })];
    
    let ctx = LlmContext {
        system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages: llm_messages,
        tools: None,
    };
    
    let mut stream = self.provider.stream(
        &self.model,
        ctx,
        StreamOptions { max_tokens: Some(max_tokens as u32), ..Default::default() },
        signal.child_token(),
    ).await.map_err(|e| CompactionError::LlmError(e.to_string()))?;
    
    // Consume stream to get final message
    let mut summary_text = String::new();
    
    while let Some(event) = stream.next().await {
        match event {
            Ok(AssistantMessageEvent::TextDelta { delta, .. }) => {
                summary_text.push_str(&delta);
            }
            Ok(AssistantMessageEvent::Done { message, .. }) => {
                summary_text = message.content.iter()
                    .filter_map(|c| match c {
                        Content::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                break;
            }
            Ok(AssistantMessageEvent::Error { error, .. }) => {
                return Err(CompactionError::LlmError(error.to_string()));
            }
            _ => {}
        }
    }
    
    if summary_text.is_empty() {
        return Err(CompactionError::LlmError("Turn prefix summary returned empty text".into()));
    }
    
    Ok(summary_text)
}
```

### 6.7 Prompt 模板

```rust
const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a conversation summarizer. ..."#;

const SUMMARIZATION_PROMPT: &str = r#"Summarize the conversation above into a structured format:
- Overview
- Progress (Done / In Progress)
- Key Decisions
- Current State
- Next Steps
- Important files and functions mentioned

Be concise but preserve exact file paths, function names, and error messages."#;

const UPDATE_SUMMARIZATION_PROMPT: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it"#;

const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix. Be concise. Focus on what's needed to understand the kept suffix."#;
```

### 6.8 序列化 helper

```rust
fn serialize_messages(messages: &[AgentMessage]) -> String {
    let mut output = String::new();
    for msg in messages {
        let (role, text) = match msg {
            AgentMessage::User(user) => ("User", extract_text(&user.content)),
            AgentMessage::Assistant(assistant) => ("Assistant", extract_text(&assistant.content)),
            AgentMessage::ToolResult(result) => ("Tool", extract_text(&result.content)),
        };
        output.push_str(&format!("[{}]: {}\n\n", role, text));
    }
    output
}

fn extract_text(content: &[Content]) -> String {
    content.iter().filter_map(|c| match c {
        Content::Text { text } => Some(text.as_str()),
        _ => None,
    }).collect::<Vec<_>>().join(" ")
}
```

---

## 7. SessionActor (`src/session.rs`)

### 7.1 结构

```rust
pub struct SessionActor {
    // Identity
    tenant_id: String,
    session_id: String,

    // State
    model: String,
    system_prompt: String,
    stream_options: StreamOptions,
    max_retries: u32,
    tools: Vec<AgentToolRef>,
    entries: Arc<Mutex<Vec<SessionEntry>>>,
    is_streaming: bool,

    // DI
    provider: Arc<dyn LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    compaction_actor: Arc<CompactionActor>,
    /// Optional persistence backend for message history
    store: Option<Arc<dyn SessionStore>>,

    // Queues — shared with AgentLoopConfig via Arc
    steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,

    // Error recovery state
    overflow_recovery_attempted: bool,
    retry_attempt: u32,
    max_auto_retries: u32,

    // Control
    abort_token: CancellationToken,

    // Event listeners
    event_listeners: Vec<Arc<dyn AgentEventListener>>,
    
    // Event queue — serial processing
    event_tx: Option<mpsc::Sender<QueuedEvent>>,
    event_processor_handle: Option<JoinHandle<()>>,
}

struct QueuedEvent {
    event: AgentEvent,
    new_messages: Vec<AgentMessage>,
}
```

### 7.2 完整接口

```rust
impl SessionActor {
    pub fn new(
        tenant_id: String,
        session_id: String,
        system_prompt: String,
        model: String,
        provider: Arc<dyn LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        compaction_actor: Arc<CompactionActor>,
        tools: Vec<AgentToolRef>,
        store: Option<Arc<dyn SessionStore>>,
    ) -> Self;
    // 内部: 
    //   - stream_options = StreamOptions::default()
    //   - max_retries = 3
    //   - steer_queue = Arc::new(Mutex::new(Vec::new()))
    //   - follow_up_queue = Arc::new(Mutex::new(Vec::new()))
    //   - event_listeners = Vec::new()
    //   - entries = Arc::new(Mutex::new(Vec::new()))
    //   - on_session_start hook fired via tokio::spawn:
    //       let ctx = SessionCtx { tenant_id, session_id, system_prompt, tools };
    //       tokio::spawn(async move { hook_dispatcher.on_session_start(&ctx).await });

    /// Attempt to restore message history from configured store.
    /// Returns number of messages restored, or 0 if no store / no data.
    pub async fn restore(&mut self) -> Result<usize, AgentError>;

    pub async fn prompt(&mut self, text: String)
        -> Result<Vec<AgentMessage>, AgentError>;

    pub async fn continue_(&mut self)
        -> Result<Vec<AgentMessage>, AgentError>;

    /// Convenience method — run the full agent loop and return only the
    /// concatenated text content of all assistant responses.
    ///
    /// Internally calls `prompt()`. Tool calls are executed normally but
    /// the caller receives only text. Matches pi.dev `complete()`.
    pub async fn complete(&mut self, text: String)
        -> Result<String, AgentError>;

    pub fn abort(&self);

    pub fn steer(&self, message: AgentMessage);
    pub fn follow_up(&self, message: AgentMessage);

    /// Flush pending persistence writes. Async save after prompt() is
    /// fire-and-forget; call flush() before shutdown to guarantee durability.
    pub async fn flush(&self) -> Result<(), AgentError>;

    pub fn entries(&self) -> Vec<SessionEntry>;
    pub fn messages(&self) -> Vec<AgentMessage>;
    pub fn system_prompt(&self) -> &str;
    pub fn set_system_prompt(&mut self, prompt: String);
    pub fn set_model(&mut self, model: String);
    pub fn set_tools(&mut self, tools: Vec<AgentToolRef>);
    pub fn set_stream_options(&mut self, options: StreamOptions);
    pub fn set_max_retries(&mut self, max_retries: u32);
    pub fn is_streaming(&self) -> bool;

    pub fn add_event_listener(&mut self, listener: Arc<dyn AgentEventListener>);
}
```

### 7.3 SessionActor::new 实现

> **工具合并职责**：`tools` 参数由 SessionActor 的**调用方（组装层）**提供。
> 调用方负责从 `ExtensionManager::collect_agent_tools()` 收集扩展工具，与原生 `AgentToolRef` 合并（扩展工具同名覆盖原生），再将合并后的列表传入。
> SessionActor 不持有 ExtensionManager 的引用。

```rust
impl SessionActor {
    pub fn new(
        tenant_id: String,
        session_id: String,
        system_prompt: String,
        model: String,
        provider: Arc<dyn LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        compaction_actor: Arc<CompactionActor>,
        tools: Vec<AgentToolRef>,
        store: Option<Arc<dyn SessionStore>>,
    ) -> Self {
        let entries = Arc::new(Mutex::new(Vec::new()));
        let steer_queue = Arc::new(Mutex::new(Vec::new()));
        let follow_up_queue = Arc::new(Mutex::new(Vec::new()));
        
        let mut actor = Self {
            tenant_id: tenant_id.clone(),
            session_id: session_id.clone(),
            model: model.clone(),
            system_prompt: system_prompt.clone(),
            stream_options: StreamOptions::default(),
            max_retries: 3,
            tools,
            entries,
            is_streaming: false,
            provider,
            hook_dispatcher: hook_dispatcher.clone(),
            compaction_actor,
            steer_queue,
            follow_up_queue,
            store,
            overflow_recovery_attempted: false,
            retry_attempt: 0,
            max_auto_retries: 3,
            abort_token: CancellationToken::new(),
            event_listeners: Vec::new(),
            event_tx: None,
            event_processor_handle: None,
        };
        
        // Initialize event queue
        let event_tx = actor.spawn_event_processor();
        actor.event_tx = Some(event_tx);
        
        // Fire on_session_start hook (observation, fire-and-forget)
        let tool_defs: Vec<Value> = tools.iter()
            .map(|t| serde_json::json!({
                "name": t.name(),
                "description": t.description(),
                "parameters": t.parameters(),
            }))
            .collect();
        let ctx = SessionCtx {
            tenant_id: tenant_id.clone(),
            session_id: session_id.clone(),
            system_prompt,
            tools: tool_defs,
        };
        tokio::spawn(async move {
            let _ = hook_dispatcher.on_session_start(&ctx).await;
        });
        
        actor
    }
}
```

### 7.4 Event 队列实现

```rust
impl SessionActor {
    fn spawn_event_processor(
        &mut self,
    ) -> mpsc::Sender<QueuedEvent> {
        let (tx, mut rx) = mpsc::channel::<QueuedEvent>(1024);
        let listeners = self.event_listeners.clone();
        let hook_dispatcher = self.hook_dispatcher.clone();
        let entries = self.entries.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();

        let handle = tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                // 1. Extension hooks (blocking/chain)
                match &queued.event {
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
                
                // 2. AgentEventListeners
                for listener in &listeners {
                    let _ = listener.on_event(&queued.event).await;
                }
                
                // 3. Persistence (append messages to entries)
                {
                    let mut entries_guard = entries.lock().expect("entries poisoned");
                    for msg in &queued.new_messages {
                        entries_guard.push(SessionEntry::Message {
                            id: uuid::Uuid::new_v4(),
                            message: msg.clone(),
                        });
                    }
                }
                
                // Note: Post-processing (retry/compaction) is handled synchronously
                // by run_with_messages() after AgentLoop::run completes.
            }
        });
        
        self.event_processor_handle = Some(handle);
        tx
    }
}
```

### 7.5 prompt / continue_ 实现

```rust
async fn run_with_messages(&mut self, add_user_msg: Option<String>)
    -> Result<Vec<AgentMessage>, AgentError>
{
    self.is_streaming = true;
    self.abort_token = CancellationToken::new();

    if let Some(text) = add_user_msg {
        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![Content::Text { text }],
            timestamp: SystemTime::now(),
        });
        {
            let mut entries = self.entries.lock().expect("entries poisoned");
            entries.push(SessionEntry::Message {
                id: uuid::Uuid::new_v4(),
                message: user_msg.clone(),
            });
        }
    }

    let messages = {
        let entries = self.entries.lock().expect("entries poisoned");
        SessionContextBuilder::build_context(&*entries)
    };

    // Create event sink that forwards to event queue
    let event_tx = self.event_tx.clone();
    let event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync> = Arc::new(move |event| {
        if let Some(tx) = &event_tx {
            // Bounded channel (1024): try_send drops event when full.
            // Tool progress events (ToolExecutionUpdate) are best-effort;
            // lifecycle events (MessageEnd, TurnEnd) carry persistence data
            // and should never be dropped under normal load.
            let new_messages = match &event {
                AgentEvent::MessageEnd { message } => vec![message.clone()],
                _ => vec![],
            };
            if tx.try_send(QueuedEvent { event, new_messages }).is_err() {
                tracing::warn!("event queue full, dropping event");
            }
        }
    });

    let config = AgentLoopConfig {
        tenant_id: self.tenant_id.clone(),
        session_id: self.session_id.clone(),
        model: self.model.clone(),
        provider: self.provider.clone(),
        hook_dispatcher: self.hook_dispatcher.clone(),
        tools: self.tools.clone(),
        system_prompt: Some(self.system_prompt.clone()),
        stream_options: self.stream_options.clone(),
        max_retries: self.max_retries,
        event_sink,
        steer_queue: self.steer_queue.clone(),
        follow_up_queue: self.follow_up_queue.clone(),
    };

    let new_msgs = match AgentLoop::run(config, messages, self.abort_token.child_token()).await {
        Ok(msgs) => {
            self.is_streaming = false;
            
            // Post-processing: find last assistant and handle retry/compaction
            if let Some(AgentMessage::Assistant(assistant)) = msgs.iter().rfind(|m| matches!(m, AgentMessage::Assistant(_))) {
                if is_retryable_error(assistant) {
                    // Retry returns the new messages from the retry attempt
                    match self.handle_retryable_error(assistant).await {
                        Ok(retry_msgs) => Ok(retry_msgs),
                        Err(e) => Err(e),
                    }
                } else {
                    // Check compaction (non-blocking for return value)
                    let _ = self.check_compaction(assistant).await;
                    Ok(msgs)
                }
            } else {
                Ok(msgs)
            }
        }
        Err(e) => {
            self.is_streaming = false;
            Err(e)
        }
    };
    
    new_msgs
}
```

### 7.6 Auto-compaction 触发

```rust
impl SessionActor {
    /// Returns the context window size for the current model.
    /// Uses a minimal hardcoded mapping for common models.
    /// TODO: Replace with model registry lookup when available.
    fn model_context_window(&self) -> usize {
        match self.model.as_str() {
            // OpenAI
            "gpt-4" | "gpt-4o" => 128_000,
            "gpt-4-turbo" => 128_000,
            "gpt-3.5-turbo" => 16_385,
            // Anthropic
            "claude-3-5-sonnet" | "claude-3-5-sonnet-20241022" => 200_000,
            "claude-3-opus" | "claude-3-opus-20240229" => 200_000,
            "claude-3-haiku" | "claude-3-haiku-20240307" => 200_000,
            // Google
            "gemini-1.5-pro" => 2_000_000,
            "gemini-1.5-flash" => 1_000_000,
            // Default: disable threshold-based compaction
            _ => 0,
        }
    }
}

async fn check_compaction(&mut self, last_assistant: &AssistantMessage) -> Result<(), AgentError> {
    let config = &self.compaction_actor.config;
    if !config.enabled {
        return Ok(());
    }
    
    if last_assistant.stop_reason == StopReason::Aborted {
        return Ok(());
    }
    
    // Skip if assistant message is from before last compaction
    {
        let entries = self.entries.lock().expect("entries poisoned");
        if let Some(SessionEntry::Compaction { timestamp, .. }) = entries.iter().rfind(|e| matches!(e, SessionEntry::Compaction { .. })) {
            if last_assistant.timestamp <= *timestamp {
                return Ok(());
            }
        }
    }
    
    // Case 1: Overflow
    if is_context_overflow(last_assistant) {
        if self.overflow_recovery_attempted {
            return Err(AgentError::CompactionFailed(
                "Context overflow recovery failed after one compact-and-retry attempt".into()
            ));
        }
        
        self.overflow_recovery_attempted = true;
        
        // The error assistant message remains in self.entries (transcript integrity),
        // but is excluded from future LLM context via SessionContextBuilder.
        // No explicit action needed here — the error message stays in entries.
        
        self.run_auto_compaction(CompactReason::Overflow, true).await?;
        return Ok(());
    }
    
    // Case 2: Threshold
    let context_tokens = {
        let entries = self.entries.lock().expect("entries poisoned");
        estimate_context_tokens(&*entries)
    };
    let context_window = self.model_context_window();
    
    if should_compact(context_tokens, context_window, config) {
        self.run_auto_compaction(CompactReason::Threshold, false).await?;
    }
    
    Ok(())
}

fn is_context_overflow(assistant: &AssistantMessage) -> bool {
    assistant.stop_reason == StopReason::Error &&
    assistant.error_message.as_ref().map_or(false, |e| {
        e.to_lowercase().contains("context length") ||
        e.to_lowercase().contains("token limit")
    })
}

fn should_compact(tokens: usize, window: usize, config: &CompactionConfig) -> bool {
    window > 0 && tokens > window.saturating_sub(config.reserve_tokens)
}

async fn run_auto_compaction(
    &mut self,
    reason: CompactReason,
    will_retry: bool,
) -> Result<(), AgentError> {
    // Emit compaction_start
    if let Some(tx) = &self.event_tx {
        let _ = tx.send(QueuedEvent {
            event: AgentEvent::CompactionStart { reason: reason.clone() },
            new_messages: vec![],
        });
    }
    
    // 1. Extension hook
    let entries_guard = self.entries.lock().expect("entries poisoned");
    let preparation = self.compaction_actor.prepare(&*entries_guard)
        .map_err(|e| AgentError::CompactionFailed(e.to_string()))?;
    
    let compact_ctx = CompactCtx {
        tenant_id: self.tenant_id.clone(),
        session_id: self.session_id.clone(),
        preparation,
        entries: (*entries_guard).clone(),
        reason: reason.clone(),
    };
    drop(entries_guard);
    
    let decision = self.hook_dispatcher.on_before_compact(&compact_ctx).await;
    
    let result = match decision {
        CompactDecision::Block { reason } => {
            if let Some(tx) = &self.event_tx {
                let _ = tx.send(QueuedEvent {
                    event: AgentEvent::CompactionEnd {
                        reason: reason.clone(),
                        result: None,
                        aborted: true,
                        will_retry: false,
                        error_message: Some(reason),
                    },
                    new_messages: vec![],
                });
            }
            return Ok(());
        }
        CompactDecision::Replace { result } => result,
        CompactDecision::Continue => {
            let entries_guard = self.entries.lock().expect("entries poisoned");
            self.compaction_actor.compact(&*entries_guard, &self.abort_token.child_token()).await
                .map_err(|e| AgentError::CompactionFailed(e.to_string()))?
        }
    };
    
    // 2. Append compaction entry
    let compaction_entry = SessionEntry::Compaction {
        id: uuid::Uuid::new_v4(),
        summary: result.summary.clone(),
        first_kept_entry_id: result.first_kept_entry_id,
        tokens_before: result.tokens_before,
        details: result.details.clone(),
        from_extension: matches!(decision, CompactDecision::Replace { .. }),
        timestamp: SystemTime::now(),
    };
    
    {
        let mut entries = self.entries.lock().expect("entries poisoned");
        entries.push(compaction_entry);
    }
    
    // 3. Emit compaction_end
    if let Some(tx) = &self.event_tx {
        let _ = tx.send(QueuedEvent {
            event: AgentEvent::CompactionEnd {
                reason: reason.clone(),
                result: Some(result.clone()),
                aborted: false,
                will_retry,
                error_message: None,
            },
            new_messages: vec![],
        });
    }

    // 4. Retry if needed
    if will_retry {
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.continue_().await?;
    }
    
    Ok(())
}
```

---

## 8. 错误恢复状态机 (`src/error_recovery.rs`)

### 8.1 动机

当前设计中，错误恢复逻辑分散在 `SessionActor` 的三个位置：
- `overflow_recovery_attempted: bool` — 溢出恢复标记
- `retry_attempt: u32` + `max_auto_retries: u32` — session 层重试计数
- `handle_retryable_error()` 和 `check_compaction()` 各含独立决策分支

这四个点通过 `continue_()` 形成**隐式递归**：overflow → compact → continue_ → AgentLoop::run → 可能再次 overflow。递归在代码中不可见，通过 `prompt()` 调用栈间接实现。当需要加新恢复策略时，难以判断所有状态字段的当前值。

**解决方案**：将恢复决策抽象为独立的 `RecoveryStateMachine` —— 纯决策引擎，持有全部恢复状态，产出 `RecoveryAction` 枚举值。Side effects（sleep、compact、continue_）由 `SessionActor` 根据返回的 action 执行。

### 8.2 设计原则

1. **只管理 session 层，不碰 LLM 层**：`with_retry()`（RateLimited/Overloaded/Timeout）留在 AgentLoop，对 session 完全透明。状态机只管理 session 可感知的恢复。
2. **纯决策引擎**：`evaluate()` 只产出 `RecoveryAction` —— 不调用 `sleep()`，不调用 `compact()`，不调用 `continue_()`。可纯单元测试。
3. **3 状态足够**：不需要通用状态机框架。一个 struct + 一个方法即可。

### 8.3 类型定义

```rust
/// 状态机对 SessionActor 的决策输出
pub enum RecoveryAction {
    /// 无需恢复（正常流程）
    Continue,
    /// Session 层重试：先 backoff，再 continue_()
    RetryAfterBackoff { delay_ms: u64 },
    /// 溢出恢复：先 compact，再 continue_()
    RetryAfterCompaction { reason: CompactReason },
    /// 放弃恢复：返回错误给调用者
    Abort { reason: String },
}

/// 恢复状态机 — 单一状态持有者
pub struct RecoveryStateMachine {
    /// 溢出恢复是否已尝试（跨 session 生命周期，最多一次）
    overflow_attempted: bool,
    /// session 层重试计数
    retry_count: u32,
    /// 最大 session 层重试次数
    max_retries: u32,
}

impl RecoveryStateMachine {
    pub fn new(max_retries: u32) -> Self {
        Self { overflow_attempted: false, retry_count: 0, max_retries }
    }

    /// 核心方法：给定 AssistantMessage，返回下一步行动。
    /// 状态机自身持有并更新状态。
    pub fn evaluate(&mut self, msg: &AssistantMessage) -> RecoveryAction {
        // 1. 溢出检测（优先级最高）
        if is_context_overflow(msg) {
            if self.overflow_attempted {
                return RecoveryAction::Abort {
                    reason: "Context overflow recovery failed after compact-and-retry".into()
                };
            }
            self.overflow_attempted = true;
            return RecoveryAction::RetryAfterCompaction {
                reason: CompactReason::Overflow
            };
        }

        // 2. Session 层重试判定
        if is_session_retryable(msg) {
            self.retry_count += 1;
            if self.retry_count > self.max_retries {
                self.retry_count = 0;
                return RecoveryAction::Abort {
                    reason: "Max retry attempts exceeded".into()
                };
            }
            let delay_ms = 100 * 2_u64.pow(self.retry_count - 1);
            return RecoveryAction::RetryAfterBackoff { delay_ms };
        }

        RecoveryAction::Continue
    }

    /// 恢复成功后重置计数器（overflow 标记不清零 — 跨 session 生命周期）
    pub fn mark_success(&mut self) {
        self.retry_count = 0;
    }

    /// 取消时全部重置
    pub fn reset(&mut self) {
        self.retry_count = 0;
        self.overflow_attempted = false;
    }
}
```

### 8.4 Session 层 Retryable 判定

```rust
const RETRYABLE_PATTERNS: &[&str] = &[
    "overloaded", "rate limit", "429", "timeout", "network error",
    "service unavailable", "fetch failed", "terminated",
    "500", "502", "503", "504",
];

fn is_session_retryable(msg: &AssistantMessage) -> bool {
    msg.stop_reason == StopReason::Error
        && msg.error_message.as_ref().map_or(false, |e| {
            let lower = e.to_lowercase();
            RETRYABLE_PATTERNS.iter().any(|p| lower.contains(p))
        })
        && !is_context_overflow(msg)  // overflow 走独立恢复路径
}
```

### 8.5 SessionActor 集成

SessionActor 从持有分散状态字段变为持有单一 `RecoveryStateMachine`：

```rust
pub struct SessionActor {
    // 删除：
    //   overflow_recovery_attempted: bool,
    //   retry_attempt: u32,
    //   max_auto_retries: u32,

    // 新增：
    recovery: RecoveryStateMachine,

    // 保持不变：
    entries: Vec<SessionEntry>,
    agent: AgentLoop,
    compaction_actor: Arc<CompactionActor>,
    event_tx: Option<tokio::sync::mpsc::Sender<QueuedEvent>>,
    abort_token: CancellationToken,
    // ...
}
```

Post-processing 段从 ~230 行缩减为 action dispatch：

```rust
// 在 AgentEnd 事件处理完毕后：
let action = self.recovery.evaluate(&assistant);
match action {
    RecoveryAction::RetryAfterCompaction { reason } => {
        emit_event!(CompactionStart { reason });
        self.compact_and_continue(reason).await?;
        emit_event!(CompactionEnd { will_retry: true });
        self.recovery.mark_success();

        let new_msgs = self.continue_().await?;
        emit_event!(AutoRetryEnd { success: true });
        return Ok(new_msgs);
    }
    RecoveryAction::RetryAfterBackoff { delay_ms } => {
        emit_event!(AutoRetryStart { attempt: self.recovery.retry_count, delay_ms });
        tokio::select! {
            _ = sleep(delay_ms) => {}
            _ = self.abort_token.cancelled() => {
                emit_event!(AutoRetryEnd { success: false, error: "cancelled" });
                self.recovery.reset();
                return Ok(vec![]);
            }
        }
        let new_msgs = self.continue_().await?;
        emit_event!(AutoRetryEnd { success: true });
        self.recovery.mark_success();
        return Ok(new_msgs);
    }
    RecoveryAction::Abort { reason } => {
        emit_event!(AutoRetryEnd { success: false, error: reason });
        self.recovery.mark_success();
        return Ok(vec![]);
    }
    RecoveryAction::Continue => {
        self.recovery.mark_success();
        // 阈值压缩检查（纯压缩，无 retry — 不属于恢复状态机）
        self.check_threshold_compaction(&assistant).await?;
    }
}
```

### 8.6 状态转换图

```
          evaluate(assistant_msg)
                   │
    ┌──────────────┼───────────────┐
    ▼              ▼               ▼
 Overflow?    Retryable?        Normal
    │              │               │
 ┌──┴──┐    ┌──────┴──────┐   Continue
 │     │    │             │
1st  again  within       exceeded
 │     │    limit         limit
 │     │     │              │
 │   Abort  Retry-        Abort
 │          After-
 │          Backoff
 │             │
Retry-     sleep + continue_
After-         │
Compaction mark_success()
 │         (retry_count=0)
compact +
continue_
 │
mark_success()
(overflow_attempted stays true)
```

### 8.7 关键规则

1. **LLM 层重试不受影响**：`with_retry()` 仍在 `AgentLoop::call_llm_with_retry`，对 session 透明
2. **overflow recovery 仅一次**：`overflow_attempted` 跨整个 session 生命周期，compaction 后仍然溢出说明当前模型无法处理此 session
3. **retry 计数在成功后重置**：`mark_success()` 清零 `retry_count`，但保留 `overflow_attempted`
4. **error message 保留在 entries，从 agent state 移除**：transcript 完整，LLM 不再看到 error
5. **所有重试可取消**：SessionActor 在 `tokio::select!` 中检查 `CancellationToken`
6. **阈值压缩独立管理**：`check_threshold_compaction()` 不涉及 retry 逻辑，不算恢复状态机职责

### 8.8 测试计划

| 测试 | 输入 | 预期输出 |
|---|---|---|
| `test_overflow_first_time` | overflow=true, attempted=false | `RetryAfterCompaction` |
| `test_overflow_second_time` | overflow=true, attempted=true | `Abort { reason }` |
| `test_retryable_within_limit` | retryable=true, count=0, max=3 | `RetryAfterBackoff { delay_ms: 100 }` |
| `test_retryable_exhausted` | retryable=true, count=3, max=3 → count=4 | `Abort { reason }` |
| `test_mark_success_preserves_overflow` | 先 overflow → mark_success → overflow_attempted=true | overflow_attempted 保持 true, retry_count=0 |
| `test_reset_clears_all` | reset() → 所有字段归零 | 两个字段均为初始值 |

---

## 9. 设计约束与边界说明

### 9.1 事件顺序保证

SessionActor 使用串行事件队列保证以下顺序：

```
For each event:
  1. Extension hooks (blocking/chain) — via HookDispatcher
  2. AgentEventListeners — callbacks to session subscribers
  3. Persistence — append messages to SessionEntry[]
  4. Post-processing — compaction check / retry logic
```

此顺序保证：
- Extension 的 `on_turn_end` 在持久化之前执行，允许 extension 修改消息
- Listeners 在持久化之后收到事件，看到的是最终状态
- Compaction / retry 在一切完成后触发，基于完整的消息历史

### 9.2 串行队列实现

使用 `tokio::sync::mpsc::channel(1024)` + 独立 processing task：

- 每个 SessionActor 有独立的事件处理 task
- 事件按入队顺序严格串行处理
- 容量 1024。满时 `send().await` 阻塞 AgentLoop，形成自然背压——慢事件处理器自动减速生产者，防止 OOM
- 单个 listener/handler 失败不阻塞后续事件（错误被记录并吞掉）
- SessionActor drop 时等待事件队列清空（graceful shutdown）

### 9.3 Hook 超时与路由

| Hook | 路由 | 超时 | 默认行为 |
|---|---|---|---|
| `on_tool_call`, `on_before_compact` | Actor Mailbox + oneshot | 500ms | `Continue` |
| `on_tool_result`, `on_context`, `on_before_agent_start`, `on_before_provider_request`, `on_after_provider_response` | Actor Mailbox + oneshot | 500ms | skip handler (default mutation) |
| `on_turn_end`, `on_agent_end`, `on_session_start` | EventBus broadcast | 100ms | silent drop |

阻断型和链式 hook 的 `oneshot` 超时（500ms）由 **extensions crate 的 ExtensionActor** 负责。agent-core 仅定义 `HookDispatcher` trait，超时逻辑在 `extensions::host::extension_actor::ExtensionHandle` 中。

观测型 hook 的 100ms 超时 + EventBus 广播在 `extensions::host::hook_router::HookRouter` 中实现。

### 9.3.1 ai-provider Provider Hooks 与本模块 HookDispatcher 的分工

ai-provider 的 `StreamOptions` 提供了两种底层 hook（`OnPayloadFn`、`OnResponseFn`），与本模块的 `HookDispatcher` 方法各自操作在不同抽象层：

| 层次 | 抽象层 | Hook | 用途 |
|---|---|---|---|
| agent-core | 结构化 LlmContext | `on_before_provider_request` | 修改 system_prompt/messages/tools/options |
| ai-provider | 原始 JSON payload | `OnPayloadFn` | 注入 provider-specific 字段（Anthropic `user_id`、自定义 header） |
| agent-core | structured AssistantMessage | `on_after_provider_response` | 内容过滤、stop_reason 覆写 |
| ai-provider | HTTP status/headers | `OnResponseFn` | 状态码观测、限流 header 解析 |

**规则**：
- agent-core 层的 hook 由 `HookDispatcher` trait 提供，通过 Extension 系统路由
- ai-provider 层的 hook 通过 `StreamOptions` 注入，由 agent-core 或更上层填装
- 两套 hook 独立触发、不互斥。`OnPayloadFn` 在 provider 实现内部执行，先于 `provider.stream()` 返回；`HookDispatcher::on_before_provider_request` 在 `AgentLoop` 中执行，先于 `provider.stream()` 调用
- 禁止在 `OnPayloadFn` 中调用 `HookDispatcher`（破坏依赖方向）

### 9.4 AgentEvent vs Extension hook

`AgentEvent` 是 agent-core 级事件（`AgentEventListener` 回调），供 session 级消费者使用（持久化层、API gateway 等）。`Extension::on_turn_end` / `on_agent_end` 是 extension 级 hook（EventBus 广播），两者是**不同的通道**。

### 9.5 `std::sync::Mutex` in async context

steer/follow_up 队列使用 `Arc<std::sync::Mutex<Vec>>` 而非 `tokio::sync::Mutex`。理由：临界区极短（`drain(..)` -> push 单个元素），锁持有时间 < 1µs，不会阻塞 async executor。

### 9.6 `.expect()` vs `.unwrap()`

根据 AGENTS.md 错误处理约束，生产代码中 `.unwrap()` 替换为 `.expect("reason")`。Mutex lock 失败表示线程 panic 导致 poison，属于不可恢复错误。

---

## 10. 文件变更清单

| 文件 | 操作 | 说明 |
|---|---|---|
| `src/session_entry.rs` | **新增** | SessionEntry enum, CompactionDetails, SessionContextBuilder |
| `src/compaction.rs` | **完全重写** | CompactionActor 完整实现（cut point, LLM summary, mid-turn split, file ops） |
| `src/file_ops.rs` | **新增** | FileOperationExtractor trait + DefaultFileOperationExtractor |
| `src/context.rs` | **修改** | 新增 6 个 ctx struct (BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx, CompactCtx, + 已有 ctx 的代码实现) |
| `src/mutations.rs` | **修改** | 新增 4 个 mutation/enum (BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation, CompactDecision) |
| `src/hook_dispatcher.rs` | **修改** | 新增 4 个方法: on_before_agent_start, on_before_provider_request, on_after_provider_response, on_before_compact |
| `src/events.rs` | **修改** | 新增 CompactionStart/End, AutoRetryStart/End 变体 |
| `src/session.rs` | **扩展** | 事件队列、auto-compaction、错误恢复、retry 状态 |
| `src/error_recovery.rs` | **新增** | Retryable error 判定、session 层重试逻辑 |
| `src/loop.rs` | **修改** | AgentLoop::run 注入 on_before_agent_start; run_turn 注入 on_before_provider_request, on_after_provider_response; execute_tools 注入 tool_execution_* 系列 |
| `src/lib.rs` | **修改** | 导出新模块 |
| `src/error.rs` | **修改** | 新增 `CompactionFailed` 变体 |

---

## 11. 测试计划

| 测试 | 验证点 |
|---|---|
| `test_normal_stop` | prompt → LLM 返回 StopReason::Stop → agent_end 事件 |
| `test_tool_call_roundtrip` | prompt → ToolUse → tool 执行 → LLM 再次返回 Stop |
| `test_multiple_turns` | 2 轮 ToolUse → Stop，验证 turn_index 递增 |
| `test_parallel_tool_execution` | 两个 Parallel 工具并发计时，验证并行 |
| `test_sequential_tool_execution` | 工具按顺序执行 |
| `test_retry_on_rate_limit` | Mock provider: RateLimited ×2 → 第 3 次成功 |
| `test_retry_exhausted` | Mock provider: RateLimited ×3 → 返回错误 |
| `test_steer_injection` | steer("msg") → prompt → 验证 steer 消息出现在 LLM context |
| `test_follow_up_loop` | follow_up("msg") → prompt → 验证外层循环重启 |
| `test_continue_session` | prompt → continue_ → 验证消息历史连续无重复 user msg |
| `test_event_callback` | 注册 listener → 验证 AgentStart, TurnStart, MessageEnd 被调用 |
| `test_event_queue_order` | 验证 extension → listener → persistence 顺序 |
| `test_cancellation` | prompt 期间 abort → 验证 Cancelled 错误 |
| `test_unknown_tool` | LLM 调用不存在的 tool → ToolResultMessage { is_error: true } |
| `test_hook_block` | on_tool_call 返回 Block → 工具不执行 |
| `test_compaction_cut_point` | 验证合法 cut point 不在 toolResult 处 |
| `test_compaction_mid_turn_split` | 验证 mid-turn split 时生成两个 summary 并合并 |
| `test_compaction_incremental_summary` | 验证 previous_summary 被正确传递和更新 |
| `test_compaction_file_ops` | 验证 read/write/edit tool calls 被提取到 details |
| `test_compaction_overflow_recovery` | Overflow → compact → retry → success |
| `test_compaction_overflow_exhausted` | Overflow → compact → retry → overflow again → error |
| `test_compaction_threshold` | 达到 threshold → compact → 不 retry |
| `test_compaction_extension_block` | on_before_compact 返回 Block → 取消 compaction |
| `test_compaction_extension_replace` | on_before_compact 返回 Replace → 使用 extension 结果 |
| `test_auto_retry_success` | Error → retry → success → retry_attempt 重置 |
| `test_auto_retry_exhausted` | Error × N → 返回错误 |
| `test_retryable_pattern_match` | 匹配各种 retryable error message |
| `test_session_rebuild_after_compaction` | compaction 后 agent state 正确重建 |
| `test_compaction_event_order` | compaction_start → extension hook → compaction_end 顺序 |
| `test_orphan_tool_call_injected` | 历史消息有未 resolve ToolCall → 注入 `_orphan: true` ToolResult |
| `test_no_orphan_when_resolved` | 所有 ToolCall 都有 ToolResult → 消息不变 |
| `test_orphan_inserted_correct_position` | 多个 assistant 中有第二个含孤儿 → 插入在正确位置 |
| `test_thinking_blocks_persisted` | ThinkingDelta/End 事件 → Content::Thinking 写入 assistant content |
| `test_complete_returns_text` | `complete("hello")` → 返回拼接的 text content |
| `test_complete_with_tool_use` | 带工具 mock → tool 执行完成，只返回最终 text |
| `test_before_agent_start_injects_messages` | on_before_agent_start 返回 Some(messages) → 替换 initial_messages |
| `test_before_agent_start_overrides_system_prompt` | on_before_agent_start 返回 Some(prompt) → 替换 system_prompt |
| `test_before_provider_request_modifies_messages` | on_before_provider_request 修改 messages → LLM 收到修改后的 context |
| `test_before_provider_request_clears_system_prompt` | mutation.system_prompt = Some(None) → system_prompt 被清空 |
| `test_after_provider_response_modifies_content` | on_after_provider_response 修改 content → assistant_msg.content 被替换 |
| `test_after_provider_response_modifies_stop_reason` | on_after_provider_response 修改 stop_reason → stop_reason 被替换 |

---

## 12. 未来工作

### 12.1 跨 Provider 消息正规化（对标 pi.dev `transform-messages.ts`）

跨 provider 消息兼容性分两层：

**1. 内置层（`llm_client::transform_messages`，AgentLoop step 2.6）**

处理正确性关键变换——缺失会导致 API 拒绝：
- Tool call ID 截断（Anthropic ≤ 64 chars → `short_hash`）
- Image block 降级（非 vision model → 文本占位符）
- Orphan tool call padding（缺失 AssistantMessage → 插入占位）

**2. Extension 层（`on_before_provider_request` / `on_context`）**

处理策略级变换——可安全缺失、可定制：
- Thinking block 保留策略（同 provider 保留，跨 provider 剥离）
- 消息过滤/注入（per-tenant 定制规则）
- Provider-specific metadata 调整

Extension 不需要复制内置层的逻辑。内置层保证 API 调用的基本正确性，Extension 层在此之上叠加策略。

### 12.2 Prompt Caching 策略

Anthropic `cache_control` / OpenAI `prompt_cache_key` 可大幅降低 token 成本。agent-core 的 `LlmProvider` trait 通过 `StreamOptions` 接收 per-request 配置，provider 适配器层可自行实现缓存标记。agent-core loop 不负责此逻辑。

### 12.3 Faux Provider 增强

pi.dev 的 `faux.ts`（499 行）提供完整内存 mock provider：脚本化响应队列、token 估算（~4 chars/token）、可配速 `tokensPerSecond`。当前 agent-core 测试用 ad-hoc mock provider。增强版属于 `ai-provider` 的 `test-utils` feature。
