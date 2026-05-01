# agent-core 详细模块规格

> 状态: 🔲 待实现 | 依赖: llm-client | 被依赖: extensions

## 模块职责

Agent loop 核心运行时。驱动 LLM tool use 协议的双层循环，管理 session 生命周期，定义 HookDispatcher 依赖反转边界，AgentEvent 事件系统。

## 引用类型速查

本节列出 spec 中引用的外部类型，来自 `llm-client` 或 `agent-core` 已有代码。

| 类型 | 来源 | 关键字段/变体 |
|---|---|---|
| `AgentMessage` | `agent_core::types` | type alias of `llm_client::Message`; variants: `User(UserMessage)`, `Assistant(AssistantMessage)`, `ToolResult(ToolResultMessage)` |
| `AgentToolRef` | `agent_core::types` | `Arc<dyn AgentTool>` |
| `AgentTool` trait | `agent_core::types` | `name()`, `description()`, `parameters()`, `execution_mode()` → `ToolExecutionMode`, `execute(id, params)` → `AgentToolResult` |
| `AgentToolResult` | `agent_core::types` | `content: Vec<Content>`, `details: Option<Value>`, `is_error: bool` |
| `ToolExecutionMode` | `agent_core::types` | `Sequential`, `Parallel` |
| `ToolCall` | `llm_client` | `id: String`, `name: String`, `arguments: Value` |
| `Content` | `llm_client` | `Text { text }`, `Image { data, mime_type }`, `Thinking { thinking }`, `ToolCall(ToolCall)` |
| `ToolDef` | `llm_client` | `name`, `description`, `parameters: Value` |
| `UserMessage` | `llm_client` | `content: Vec<Content>`, `timestamp` |
| `AssistantMessage` | `llm_client` | `content: Vec<Content>`, `api`, `usage`, `stop_reason`, `response_id`, `error_message`, `timestamp` |
| `ToolResultMessage` | `llm_client` | `tool_call_id`, `tool_name`, `content`, `details`, `is_error`, `timestamp` |
| `StopReason` | `llm_client` | `Stop`, `Length`, `ToolUse`, `Error`, `Aborted` |
| `LlmContext` | `llm_client` | `system_prompt: Option<String>`, `messages: Vec<Message>`, `tools: Option<Vec<ToolDef>>` |
| `LlmProvider` | `llm_client` | `stream(model, context, options, signal)` → `AssistantMessageEventStream` |
| `StreamOptions` | `llm_client` | `max_tokens`, `temperature`, `top_p` |
| `LlmError` | `llm_client` | `RateLimited`, `Overloaded`, `InvalidRequest`, `ProviderError`, `Timeout`, `Cancelled`, `Serialization` |
| `HookDispatcher` trait | `agent_core::hook_dispatcher` | `on_tool_call()`, `on_tool_result()`, `on_context()`, `on_turn_end()`, `on_agent_end()`, `on_session_start()` |
| `HookDecision` | `agent_core::mutations` | `Continue`, `Block { reason }` |
| `ToolResultMutation` | `agent_core::mutations` | `content: Option<Vec<Content>>`, `details: Option<Value>`, `is_error: Option<bool>` |
| `ToolExecutor` | `agent_core::tool` | `new(Arc<HookDispatcher>, AgentToolRef)`, `execute_tool_call(&ToolCall)` → `Result<ToolResultMessage, AgentError>` |
| `AgentError` | `agent_core::error` | `ToolNotFound`, `ToolExecutionFailed`, `HookDispatchError`, `LlmError`, `Cancelled` |
| `CancellationToken` | `tokio_util::sync` | `new()`, `child_token()`, `cancel()`, `is_cancelled()` |

---

## 1. 新增类型

### 1.1 AgentEvent (`src/events.rs`)

agent-core 级事件，通过 `AgentEventListener` 回调 trait 传递给订阅者。

```rust
use llm_client::ToolResultMessage;
use crate::types::AgentMessage;
use crate::error::AgentError;

/// Events emitted by AgentLoop during execution.
/// Subscribed via AgentEventListener trait.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Agent loop started
    AgentStart,
    /// Agent loop ended
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
    /// A new turn started
    TurnStart {
        turn_index: u64,
    },
    /// A turn ended
    TurnEnd {
        turn_index: u64,
        messages: Vec<AgentMessage>,
    },
    /// An assistant message started (streaming begins)
    MessageStart {
        message_index: u64,
    },
    /// Streaming delta for current assistant message
    MessageUpdate {
        message_index: u64,
        content_delta: String,
    },
    /// An assistant message completed
    MessageEnd {
        message: AgentMessage,
    },
    /// Tool execution started
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
    },
    /// Tool execution streaming update
    ToolExecutionUpdate {
        tool_call_id: String,
        content: String,
    },
    /// Tool execution completed
    ToolExecutionEnd {
        tool_call_id: String,
        result: ToolResultMessage,
    },
    /// Non-fatal error
    Error {
        error: AgentError,
    },
}
```

### 1.2 AgentEventListener (`src/events.rs`)

回调 trait，会话级订阅者通过此接口接收 agent 事件。

```rust
use async_trait::async_trait;

#[async_trait]
pub trait AgentEventListener: Send + Sync {
    /// Called for every agent event.
    /// Listener errors are logged but not propagated to the agent loop.
    /// **Constraint (v0.1):** listeners must not panic. v0.2 will add catch_unwind.
    async fn on_event(&self, event: &AgentEvent);
}
```

### 1.3 AgentLoopConfig (`src/loop.rs` — 重构)

```rust
use std::sync::{Arc, Mutex};

pub struct AgentLoopConfig {
    pub model: String,
    pub provider: Arc<dyn LlmProvider>,
    pub hook_dispatcher: Arc<dyn HookDispatcher>,
    pub tools: Vec<AgentToolRef>,
    pub system_prompt: Option<String>,
    pub stream_options: StreamOptions,
    pub max_retries: u32,
    pub event_listeners: Vec<Arc<dyn AgentEventListener>>,
    /// Steer queue — drained before each outer loop iteration.
    /// Shared with SessionActor via Arc.
    pub steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    /// Follow-up queue — drained after agent would stop.
    /// Shared with SessionActor via Arc.
    pub follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
}
```

---

## 2. AgentLoop 双层循环

### 2.1 外层 — follow-up

```
run(config, initial_messages, signal) → Vec<AgentMessage>

messages = initial_messages
new_messages = []
turn_index: u64 = 0
message_index: u64 = 0

emit_event(&config, AgentStart)

loop {
  // —— drain steer queue ——
  {
    let mut q = config.steer_queue.lock().expect("steer queue poisoned");
    messages.extend(q.drain(..));
  }

  // —— inner turn loop ——
  loop {
    result = run_turn(&mut messages, &mut new_messages, &mut turn_index,
                      &mut message_index, &config, signal)
    match result {
      TurnResult::ToolUse => continue,  // more tool calls expected
      TurnResult::Stop => break,
      TurnResult::Error(e) => {
        emit_event(&config, Error { error: e.clone() })
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
    continue  // restart inner loop
  }

  break
}

emit_event(&config, AgentEnd { messages })
return Ok(new_messages)
```

### 2.2 内层 — turn

```
run_turn(messages, new_messages, turn_index, message_index, config, signal) → TurnResult

*turn_index += 1

emit_event(config, TurnStart { turn_index: *turn_index })

// 1. Transform context via hook chain
transformed = config.hook_dispatcher.on_context(messages.clone())

// 2. Build LlmContext
ctx = LlmContext {
  system_prompt: config.system_prompt,
  messages: transformed,
  tools: build_tool_defs(&config.tools),
}

// 3. Call LLM with retry
assistant_msg = call_llm_with_retry(ctx, config, *message_index, signal)?

// 4. Emit message_end
*message_index += 1
emit_event(config, MessageEnd { message: assistant_msg.clone() })
new_messages.push(assistant_msg.clone())
messages.push(assistant_msg.clone())

// 5. Extract ToolCalls
tool_calls = extract_tool_calls(&assistant_msg.content)
if tool_calls.is_empty() {
  emit_event(config, TurnEnd { turn_index: *turn_index, messages: messages.clone() })
  return TurnResult::Stop
}

// 6. Execute tools (partitioned by mode)
tool_results = execute_tools(tool_calls, config, signal)
for result in &tool_results {
  new_messages.push(AgentMessage::ToolResult(result.clone()))
  messages.push(AgentMessage::ToolResult(result.clone()))
}

emit_event(config, TurnEnd { turn_index: *turn_index, messages: messages.clone() })

if assistant_msg.stop_reason == StopReason::ToolUse {
  return TurnResult::ToolUse  // expect another turn
} else {
  return TurnResult::Stop
}
```

#### Helper: `build_tool_defs`

```rust
fn build_tool_defs(tools: &[AgentToolRef]) -> Option<Vec<llm_client::ToolDef>> {
    if tools.is_empty() {
        return None;
    }
    Some(tools.iter().map(|t| llm_client::ToolDef {
        name: t.name().to_string(),
        description: t.description().to_string(),
        parameters: t.parameters(),
    }).collect())
}
```

#### Helper: `emit_event`

```rust
/// Emit an event to all registered listeners.
/// Errors from individual listeners are logged and swallowed.
fn emit_event(config: &AgentLoopConfig, event: AgentEvent) {
    for listener in &config.event_listeners {
        let _ = listener.on_event(&event).await;
    }
}
```

#### Data types

```rust
enum TurnResult {
    ToolUse,           // more tool calls expected
    Stop,              // no more tool calls, final
    Error(AgentError),
}

fn extract_tool_calls(content: &[Content]) -> Vec<ToolCall> {
    content.iter().filter_map(|c| match c {
        Content::ToolCall(tc) => Some(tc.clone()),
        _ => None,
    }).collect()
}
```

### 2.3 LLM 调用 + 指数退避重试

```
call_llm_with_retry(ctx: LlmContext, config: &AgentLoopConfig, message_index: u64, signal: &CancellationToken) → Result<AssistantMessage, AgentError>

for attempt in 0..config.max_retries:
  result = config.provider.stream(model, ctx.clone(), stream_options, signal.child_token())

  match result:
    Ok(stream) => {
      // Consume stream → AssistantMessage
      emit_event(config, MessageStart { message_index })
      for each delta in stream:
        match delta:
          TextDelta(text) => {
            emit_event(config, MessageUpdate { message_index, content_delta: text })
            accumulate text into content
          }
          ToolCallDelta(tc) => accumulate tc into content
          Done { content, api, usage, stop_reason } => {
            return AssistantMessage { content, api, usage, stop_reason, ... }
          }
          Error { message } => return Err(AgentError::LlmError(...))
      }
    }
    Err(RateLimited | Overloaded) if attempt < config.max_retries - 1 => {
      sleep(2^attempt * 100ms)
      continue
    }
    Err(other) => return Err(other.into())
```

重试策略：
- 仅对 `RateLimited` 和 `Overloaded` 重试
- 后退间隔: 100ms → 200ms → 400ms
- 每次重试需要 clone ctx（ctx 被 stream() 消耗）
- 跨重试 span 记录 `retry_count`

---

## 3. 并行工具执行

```rust
fn execute_tools(
    tool_calls: Vec<ToolCall>,
    config: &AgentLoopConfig,
    signal: &CancellationToken,
) -> Vec<ToolResultMessage> {
    let mut results = Vec::new();

    // Partition by tool execution mode
    let (sequential_calls, parallel_calls): (Vec<_>, Vec<_>) = tool_calls
        .into_iter()
        .partition(|tc| {
            config.tools.iter()
                .find(|t| t.name() == tc.name)
                .map(|t| t.execution_mode() == ToolExecutionMode::Sequential)
                .unwrap_or(true)  // unknown tools default sequential (safe: validated by ToolExecutor)
        });

    // Phase 1: Sequential — one at a time
    for tc in sequential_calls {
        let result = execute_single_tool(&tc, config, signal).await;
        results.push(result);
    }

    // Phase 2: Parallel — concurrent via join_all
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
    emit_event(config, ToolExecutionStart { tool_call_id: tc.id.clone(), tool_name: tc.name.clone() });

    let tool = config.tools.iter().find(|t| t.name() == tc.name).cloned();
    let result = match tool {
        Some(tool) => {
            let executor = ToolExecutor::new(config.hook_dispatcher.clone(), tool);
            executor.execute_tool_call(tc).await
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

    emit_event(config, ToolExecutionEnd { tool_call_id: tc.id.clone(), result: result_msg.clone() });
    result_msg
}
```

---

## 4. SessionActor

### 4.1 结构

```rust
pub struct SessionActor {
    // State
    model: String,
    system_prompt: String,
    tools: Vec<AgentToolRef>,
    messages: Vec<AgentMessage>,
    is_streaming: bool,

    // DI
    provider: Arc<dyn LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,

    // Queues — shared with AgentLoopConfig via Arc
    steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,

    // Control
    abort_token: CancellationToken,

    // Event listeners
    event_listeners: Vec<Arc<dyn AgentEventListener>>,
}
```

### 4.2 完整接口

```rust
impl SessionActor {
    // --- Lifecycle ---
    pub fn new(
        system_prompt: String,
        model: String,
        provider: Arc<dyn LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        tools: Vec<AgentToolRef>,
    ) -> Self;
    // 内部: steer_queue = Arc::new(Mutex::new(Vec::new()))
    //       follow_up_queue = Arc::new(Mutex::new(Vec::new()))

    /// Send a user message and run the agent loop.
    /// Returns all NEW messages generated this run (excludes prior history).
    pub async fn prompt(&mut self, text: String)
        -> Result<Vec<AgentMessage>, AgentError>;

    /// Continue from the current transcript without adding a user message.
    /// Use after session restore or compaction.
    pub async fn continue_(&mut self)
        -> Result<Vec<AgentMessage>, AgentError>;

    /// Cancel current run
    pub fn abort(&self);

    // --- Message injection ---
    /// Queue a message to inject before the next LLM call in the current run.
    /// Pushes to steer_queue (Arc<Mutex<Vec>>), shared with AgentLoopConfig.
    pub fn steer(&mut self, message: AgentMessage);

    /// Queue a message to inject after agent would stop (triggers new outer loop).
    /// Pushes to follow_up_queue (Arc<Mutex<Vec>>), shared with AgentLoopConfig.
    pub fn follow_up(&mut self, message: AgentMessage);

    // --- State management ---
    pub fn messages(&self) -> &[AgentMessage];
    pub fn system_prompt(&self) -> &str;
    pub fn set_system_prompt(&mut self, prompt: String);
    pub fn set_model(&mut self, model: String);
    pub fn set_tools(&mut self, tools: Vec<AgentToolRef>);
    pub fn is_streaming(&self) -> bool;

    // --- Event subscription ---
    pub fn add_event_listener(&mut self, listener: Arc<dyn AgentEventListener>);
}
```

### 4.3 prompt / continue_ 实现

两者共享内部方法 `run_with_messages()`:

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
        self.messages.push(user_msg.clone());
    }

    let config = AgentLoopConfig {
        model: self.model.clone(),
        provider: self.provider.clone(),
        hook_dispatcher: self.hook_dispatcher.clone(),
        tools: self.tools.clone(),
        system_prompt: Some(self.system_prompt.clone()),
        stream_options: StreamOptions::default(),
        max_retries: 3,
        event_listeners: self.event_listeners.clone(),
        steer_queue: self.steer_queue.clone(),
        follow_up_queue: self.follow_up_queue.clone(),
    };

    let new_msgs = match AgentLoop::run(config, self.messages.clone(), self.abort_token.child_token()).await {
        Ok(msgs) => {
            self.messages.extend(msgs.clone());
            self.is_streaming = false;
            Ok(msgs)
        }
        Err(e) => {
            self.is_streaming = false;
            Err(e)
        }
    }
}
```

---

## 5. CompactionActor（占位）

```rust
/// Simple token-based message truncation.
/// v0.1 uses character count estimation (1 token ≈ 4 chars).
/// FIXME: `{:?}` debug format includes type names and field labels,
/// overestimating token count ~3x. Replace with Display impl or real tokenizer.
pub struct CompactionActor;

impl CompactionActor {
    pub fn compact(messages: &[AgentMessage], max_tokens: u64) -> Vec<AgentMessage> {
        let mut kept: Vec<AgentMessage> = Vec::new();
        let mut estimated_tokens: u64 = 0;

        // Keep newest messages, drop oldest
        for msg in messages.iter().rev() {
            let chars = format!("{:?}", msg).len() as u64;
            if estimated_tokens + chars / 4 > max_tokens {
                break;
            }
            estimated_tokens += chars / 4;
            kept.push(msg.clone());
        }

        kept.reverse();
        kept
    }
}
```

---

## 6. 设计约束与边界说明

### Hook 超时

阻断型和链式 hook 的 `oneshot` 超时（500ms）由 **extensions crate 的 ExtensionActor** 负责，不在此 crate 实现。agent-core 仅定义 `HookDispatcher` trait，超时逻辑在 `extensions::host::extension_actor::ExtensionHandle` 中。

观测型 hook（`on_turn_end`, `on_agent_end`, `on_session_start`）通过 `HookDispatcher` trait 调用，100ms 超时 + EventBus 广播在 `extensions::host::hook_router::HookRouter` 中实现。

### AgentEvent vs Extension hook

`AgentEvent` 是 agent-core 级事件（`AgentEventListener` 回调），供 session 级消费者使用（持久化层、API gateway 等）。`Extension::on_turn_end` / `on_agent_end` 是 extension 级 hook（EventBus 广播），两者是**不同的通道**，在 `HookRouter::on_turn_end()` 中分别发射：

```rust
// HookRouter::on_turn_end (in extensions crate)
async fn on_turn_end(&self, ctx: &TurnEndCtx) {
    // Extension hook → EventBus broadcast
    self.event_bus.emit(ObsEvent::TurnEnd(ctx.clone()));
    // agent-core events are handled separately by SessionActor
}
```

### `std::sync::Mutex` in async context

steer/follow_up 队列使用 `Arc<std::sync::Mutex<Vec>>` 而非 `tokio::sync::Mutex`。理由：临界区极短（`drain(..)` -> push 单个元素），锁持有时间 < 1µs，不会阻塞 async executor。这是性能优化，不违反 ADR-004（避免共享可变状态跨 session）。

### `.expect()` vs `.unwrap()`

根据 AGENTS.md 错误处理约束，生产代码中 `.unwrap()` 替换为 `.expect("reason")`。Mutex lock 失败表示线程 panic 导致 poison，属于不可恢复错误，`.expect()` 是正确选择。

---

## 7. 文件变更清单

| 文件 | 操作 | 说明 |
|---|---|---|
| `src/events.rs` | 新增 | AgentEvent 枚举 + AgentEventListener trait |
| `src/loop.rs` | 重写 | 双层循环 + retry + 并行工具 + event emit + steer/followUp pull |
| `src/session.rs` | 重写 | 完整接口（continue_, 事件订阅, steer/followUp 集成） |
| `src/compaction.rs` | 新增 | CompactionActor 占位（token 估计算法） |
| `src/lib.rs` | 修改 | 导出 events, compaction 模块 |
| `src/error.rs` | 修改 | 补充 error 变体（如有需要） |

---

## 8. 测试计划

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
| `test_compaction_truncation` | 20 条消息，max_tokens=50 → 只保留最近 ~5 条 |
| `test_cancellation` | prompt 期间 abort → 验证 Cancelled 错误 |
| `test_unknown_tool` | LLM 调用不存在的 tool → ToolResultMessage { is_error: true } |
| `test_hook_block` | on_tool_call 返回 Block → 工具不执行 |
