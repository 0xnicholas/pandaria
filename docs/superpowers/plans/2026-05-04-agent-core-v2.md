# agent-core v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 从当前 agent-core 骨架（已有基础类型、单层 AgentLoop、基础 SessionActor、自由函数 Compaction）演进为符合 spec 的完整实现：双层循环、事件系统、错误恢复、完整 hook 集成。

**Architecture:** 增量式演进，不破坏现有 API。先新增独立模块（events, error_recovery），再重构核心循环（AgentLoop → 双层），最后集成到 SessionActor。

**Tech Stack:** Rust 2024 edition, tokio, async-trait, thiserror, tracing, futures, tokio-util, serde_json.

**Spec Reference:** `docs/specs/2026-05-02-agent-core.md`

**Current State (已有实现):**
- `types.rs`: AgentMessage, SessionEntry(u64 id), CompactionEntry, AgentTool trait, AgentToolResult
- `context.rs`: 10 个 ctx 类型（字段部分未对齐 spec）
- `mutations.rs`: 7 个 mutation/decision 类型
- `hook_dispatcher.rs`: 完整 trait，10 个方法（含默认实现）
- `tool.rs`: ToolExecutor，完整 pipeline（on_tool_call → execute → on_tool_result）
- `loop.rs`: AgentLoop 单层循环，基础 stream 消费，无重试
- `session.rs`: SessionActor，prompt/steer/follow_up/abort，无事件队列
- `compaction.rs`: 自由函数实现（token 估算、cut point、summary 生成、file ops）
- `error.rs`: 6 个变体，缺 CompactionFailed
- `store.rs`: SessionStore trait
- `util.rs`: catch_panic
- **测试**: 61 个测试全部通过

**缺失核心模块:** events.rs, error_recovery.rs, provider_opts.rs

---

## File Map

### 新增文件
| 文件 | 职责 |
|---|---|
| `src/events.rs` | AgentEvent 枚举（16 个变体）+ AgentEventListener trait |
| `src/error_recovery.rs` | RecoveryStateMachine + RecoveryAction + retryable 判定 |
| `src/provider_opts.rs` | ProviderStreamOptions（StreamOptions 安全子集） |

### 修改文件
| 文件 | 变更 |
|---|---|
| `src/loop.rs` | 重构为双层循环 + AgentLoopConfig + resolve_orphan_tool_calls + call_llm_with_retry + 完整 hook 集成 + 事件发射 |
| `src/session.rs` | 添加事件队列、RecoveryStateMachine、complete/continue_、auto-compaction 集成 |
| `src/context.rs` | 字段对齐：ProviderRequestCtx 加 turn_index/tools/options，ProviderResponseCtx 加 turn_index/attempt/messages_before |
| `src/compaction.rs` | 封装 CompactionActor 结构体（保留现有自由函数作为内部实现） |
| `src/error.rs` | 添加 CompactionFailed 变体 |
| `src/lib.rs` | 导出新模块 |

---

## Phase 1: 事件系统 (P0 — ~30 min)

> 前置条件：无。独立模块，不依赖其他变更。

### Task 1.1: AgentEvent 枚举

**Files:**
- Create: `crates/agent-core/src/events.rs`
- Modify: `crates/agent-core/src/lib.rs`

**Steps:**

- [ ] **Step 1: 创建 events.rs**

```rust
use crate::error::AgentError;
use crate::types::AgentMessage;
use llm_client::ToolResultMessage;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd { messages: Vec<AgentMessage> },
    TurnStart { turn_index: u64 },
    TurnEnd { turn_index: u64, messages: Vec<AgentMessage> },
    MessageStart { message_index: u64 },
    MessageUpdate { message_index: u64, content_delta: String },
    MessageEnd { message: AgentMessage },
    ToolExecutionStart { tool_call_id: String, tool_name: String },
    ToolExecutionUpdate { tool_call_id: String, content: String },
    ToolExecutionEnd { tool_call_id: String, result: ToolResultMessage },
    CompactionStart { reason: crate::context::CompactReason },
    CompactionEnd {
        reason: crate::context::CompactReason,
        result: Option<crate::compaction::CompactionResult>,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },
    AutoRetryStart { attempt: u32, max_attempts: u32, delay_ms: u64 },
    AutoRetryEnd { success: bool, error: Option<String> },
    Error { error: AgentError },
}

#[async_trait::async_trait]
pub trait AgentEventListener: Send + Sync {
    async fn on_event(&self, event: &AgentEvent);
}
```

- [ ] **Step 2: 添加到 lib.rs**

在 `src/lib.rs` 中：
```rust
pub mod events;
pub use events::{AgentEvent, AgentEventListener};
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p agent-core`
Expected: PASS

---

### Task 1.2: CompactReason 移动到 context.rs

> 当前 events.rs 引用了 `crate::context::CompactReason`，但 context.rs 中缺少这个类型。

**Files:**
- Modify: `crates/agent-core/src/context.rs`

**Steps:**

- [ ] **Step 1: 在 context.rs 末尾添加 CompactReason**

```rust
#[derive(Debug, Clone)]
pub enum CompactReason {
    Overflow,
    Threshold,
    Manual,
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p agent-core`
Expected: PASS

---

## Phase 2: ProviderStreamOptions (P0 — ~15 min)

### Task 2.1: 创建 ProviderStreamOptions

**Files:**
- Create: `crates/agent-core/src/provider_opts.rs`
- Modify: `crates/agent-core/src/lib.rs`

**Steps:**

- [ ] **Step 1: 创建 provider_opts.rs**

```rust
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct ProviderStreamOptions {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub reasoning: Option<llm_client::ReasoningLevel>,
    pub max_retries: Option<u32>,
    pub timeout: Option<Duration>,
}

impl ProviderStreamOptions {
    pub fn from_options(options: &llm_client::StreamOptions) -> Self {
        Self {
            max_tokens: options.max_tokens,
            temperature: options.temperature,
            top_p: options.top_p,
            reasoning: options.reasoning.clone(),
            max_retries: options.max_retries,
            timeout: options.timeout,
        }
    }
}
```

- [ ] **Step 2: 添加到 lib.rs**

```rust
pub mod provider_opts;
pub use provider_opts::ProviderStreamOptions;
```

- [ ] **Step 3: 更新 ProviderRequestCtx**

在 `src/context.rs` 中，修改 `ProviderRequestCtx`：
```rust
pub struct ProviderRequestCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub model: String,
    pub turn_index: u64,
    pub system_prompt: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub tools: Option<Vec<llm_client::ToolDef>>,
    pub options: ProviderStreamOptions,  // 替换原来的 Option<serde_json::Value>
}
```

- [ ] **Step 4: 更新 ProviderRequestMutation**

在 `src/mutations.rs` 中：
```rust
pub struct ProviderRequestMutation {
    pub system_prompt: Option<Option<String>>,
    pub messages: Option<Vec<AgentMessage>>,
    pub tools: Option<Option<Vec<llm_client::ToolDef>>>,
    pub options: Option<ProviderStreamOptions>,
}
```

- [ ] **Step 5: 验证编译**

Run: `cargo check -p agent-core`
Expected: PASS（可能需要修复 hook_dispatcher.rs 中的引用）

---

## Phase 3: 错误恢复状态机 (P0 — ~30 min)

### Task 3.1: RecoveryStateMachine

**Files:**
- Create: `crates/agent-core/src/error_recovery.rs`
- Modify: `crates/agent-core/src/lib.rs`

**Steps:**

- [ ] **Step 1: 创建 error_recovery.rs**

```rust
use llm_client::{AssistantMessage, StopReason};

pub enum RecoveryAction {
    Continue,
    RetryAfterBackoff { delay_ms: u64 },
    RetryAfterCompaction { reason: crate::context::CompactReason },
    Abort { reason: String },
}

pub struct RecoveryStateMachine {
    pub overflow_attempted: bool,
    pub retry_count: u32,
    pub max_retries: u32,
}

impl RecoveryStateMachine {
    pub fn new(max_retries: u32) -> Self {
        Self {
            overflow_attempted: false,
            retry_count: 0,
            max_retries,
        }
    }

    pub fn evaluate(&mut self, msg: &AssistantMessage) -> RecoveryAction {
        if is_context_overflow(msg) {
            if self.overflow_attempted {
                return RecoveryAction::Abort {
                    reason: "Context overflow recovery failed after compact-and-retry".into(),
                };
            }
            self.overflow_attempted = true;
            return RecoveryAction::RetryAfterCompaction {
                reason: crate::context::CompactReason::Overflow,
            };
        }

        if is_session_retryable(msg) {
            self.retry_count += 1;
            if self.retry_count > self.max_retries {
                self.retry_count = 0;
                return RecoveryAction::Abort {
                    reason: "Max retry attempts exceeded".into(),
                };
            }
            let delay_ms = 100 * 2_u64.pow(self.retry_count - 1);
            return RecoveryAction::RetryAfterBackoff { delay_ms };
        }

        RecoveryAction::Continue
    }

    pub fn mark_success(&mut self) {
        self.retry_count = 0;
    }

    pub fn reset(&mut self) {
        self.retry_count = 0;
        self.overflow_attempted = false;
    }
}

fn is_context_overflow(msg: &AssistantMessage) -> bool {
    msg.stop_reason == StopReason::Error
        && msg.error_message.as_ref().map_or(false, |e| {
            let lower = e.to_lowercase();
            lower.contains("context length") || lower.contains("token limit")
        })
}

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
        && !is_context_overflow(msg)
}
```

- [ ] **Step 2: 添加到 lib.rs**

```rust
pub mod error_recovery;
pub use error_recovery::{RecoveryStateMachine, RecoveryAction};
```

- [ ] **Step 3: 写测试**

在 `src/error_recovery.rs` 末尾添加：
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{Api, Usage};
    use std::time::SystemTime;

    fn make_msg(stop_reason: StopReason, error: Option<&str>) -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: Api { provider: "test".to_string(), model: "test".to_string() },
            usage: Usage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 0 },
            stop_reason,
            response_id: None,
            error_message: error.map(|s| s.to_string()),
            timestamp: SystemTime::now(),
        }
    }

    #[test]
    fn test_overflow_first_time() {
        let mut r = RecoveryStateMachine::new(3);
        let action = r.evaluate(&make_msg(StopReason::Error, Some("context length exceeded")));
        assert!(matches!(action, RecoveryAction::RetryAfterCompaction { .. }));
    }

    #[test]
    fn test_overflow_second_time_aborts() {
        let mut r = RecoveryStateMachine::new(3);
        r.evaluate(&make_msg(StopReason::Error, Some("context length exceeded")));
        let action = r.evaluate(&make_msg(StopReason::Error, Some("context length exceeded")));
        assert!(matches!(action, RecoveryAction::Abort { .. }));
    }

    #[test]
    fn test_retryable_backoff() {
        let mut r = RecoveryStateMachine::new(3);
        let action = r.evaluate(&make_msg(StopReason::Error, Some("rate limit")));
        assert!(matches!(action, RecoveryAction::RetryAfterBackoff { delay_ms: 100 }));
    }

    #[test]
    fn test_retryable_exhausted() {
        let mut r = RecoveryStateMachine::new(1);
        r.evaluate(&make_msg(StopReason::Error, Some("overloaded")));
        let action = r.evaluate(&make_msg(StopReason::Error, Some("overloaded")));
        assert!(matches!(action, RecoveryAction::Abort { .. }));
    }

    #[test]
    fn test_normal_continue() {
        let mut r = RecoveryStateMachine::new(3);
        let action = r.evaluate(&make_msg(StopReason::Stop, None));
        assert!(matches!(action, RecoveryAction::Continue));
    }
}
```

- [ ] **Step 4: 验证测试通过**

Run: `cargo test -p agent-core error_recovery`
Expected: 5 tests PASS

---

## Phase 4: AgentLoop 核心重构 (P0 — ~4h，风险最高)

> 这是最大的变更。将单层循环重构为双层循环（外层 follow-up + 内层 turn），集成所有缺失的 hook、重试、孤儿解析、事件发射。

### Task 4.1: 新增辅助函数和类型

**Files:**
- Modify: `crates/agent-core/src/loop.rs`

**Steps:**

- [ ] **Step 1: 在 loop.rs 顶部添加新类型和辅助函数**

在现有 imports 之后添加：

```rust
use std::sync::{Arc, Mutex};
use crate::context::{BeforeAgentStartCtx, ProviderRequestCtx, ProviderResponseCtx};
use crate::mutations::{BeforeAgentStartMutation, ProviderRequestMutation, ProviderResponseMutation};
use crate::events::AgentEvent;
use crate::provider_opts::ProviderStreamOptions;

/// Configuration for AgentLoop, passed in at construction.
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
    pub steer_queue: Arc<Mutex<Vec<AgentMessage>>>,
    pub follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    pub event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync>,
}

#[derive(Debug, Clone)]
pub enum TurnResult {
    ToolUse,
    Stop,
    Error(AgentError),
}
```

- [ ] **Step 2: 添加工具函数**

```rust
fn build_tool_defs(tools: &[AgentToolRef]) -> Option<Vec<llm_client::ToolDef>> {
    if tools.is_empty() {
        None
    } else {
        Some(
            tools
                .iter()
                .map(|t| llm_client::ToolDef {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.parameters(),
                })
                .collect(),
        )
    }
}

fn build_tool_value_defs(tools: &[AgentToolRef]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name(),
                "description": t.description(),
                "parameters": t.parameters(),
            })
        })
        .collect()
}

fn apply_provider_request_mutation(
    ctx: &mut LlmContext,
    opts: &mut StreamOptions,
    mutation: ProviderRequestMutation,
) {
    if let Some(sp) = mutation.system_prompt {
        ctx.system_prompt = sp;
    }
    if let Some(msgs) = mutation.messages {
        ctx.messages = msgs;
    }
    if let Some(tools) = mutation.tools {
        ctx.tools = tools;
    }
    if let Some(options) = mutation.options {
        if let Some(mt) = options.max_tokens { opts.max_tokens = Some(mt); }
        if let Some(temp) = options.temperature { opts.temperature = Some(temp); }
        if let Some(tp) = options.top_p { opts.top_p = Some(tp); }
        if let Some(reasoning) = options.reasoning { opts.reasoning = Some(reasoning); }
        if let Some(mr) = options.max_retries { opts.max_retries = Some(mr); }
        if let Some(timeout) = options.timeout { opts.timeout = Some(timeout); }
    }
}

fn apply_provider_response_mutation(
    msg: &mut llm_client::AssistantMessage,
    mutation: ProviderResponseMutation,
) {
    if let Some(content) = mutation.content {
        msg.content = content;
    }
    if let Some(stop_reason) = mutation.stop_reason {
        msg.stop_reason = stop_reason;
    }
}

/// Scan messages for orphan ToolCalls and inject synthetic error results.
pub fn resolve_orphan_tool_calls(messages: &mut Vec<AgentMessage>) {
    use std::collections::HashSet;

    let mut tool_call_ids: Vec<(usize, String)> = Vec::new();
    let mut resolved_ids: HashSet<String> = HashSet::new();

    for (i, msg) in messages.iter().enumerate() {
        match msg {
            AgentMessage::Assistant(a) => {
                for content in &a.content {
                    if let llm_client::Content::ToolCall(tc) = content {
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

    let mut orphans: Vec<(usize, String, String)> = tool_call_ids
        .into_iter()
        .filter(|(_, id)| !resolved_ids.contains(id))
        .map(|(idx, id)| {
            let tool_name = match &messages[idx] {
                AgentMessage::Assistant(a) => a.content.iter().find_map(|c| match c {
                    llm_client::Content::ToolCall(tc) if tc.id == id => Some(tc.name.clone()),
                    _ => None,
                }),
                _ => None,
            }
            .unwrap_or_else(|| "unknown".to_string());
            (idx, id, tool_name)
        })
        .collect();

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

- [ ] **Step 3: 验证编译**

Run: `cargo check -p agent-core`
Expected: PASS（可能有一些未使用的警告，正常）

---

### Task 4.2: 重写 AgentLoop 结构和方法

**Files:**
- Modify: `crates/agent-core/src/loop.rs`

**Steps:**

- [ ] **Step 1: 替换 AgentLoop 结构体**

```rust
pub struct AgentLoop {
    config: AgentLoopConfig,
}

impl AgentLoop {
    pub fn new(config: AgentLoopConfig) -> Self {
        Self { config }
    }
```

- [ ] **Step 2: 实现外层 run 方法**

替换现有的 `pub async fn run(...)`：

```rust
    pub async fn run(
        &self,
        initial_messages: Vec<AgentMessage>,
        signal: CancellationToken,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        // on_before_agent_start hook
        let agent_start_ctx = BeforeAgentStartCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            system_prompt: self.config.system_prompt.clone(),
            messages: initial_messages.clone(),
            tools: build_tool_value_defs(&self.config.tools),
            model: self.config.model.clone(),
        };
        let agent_start_mutation = self.config.hook_dispatcher.on_before_agent_start(&agent_start_ctx).await;
        let system_prompt = agent_start_mutation.system_prompt.or_else(|| self.config.system_prompt.clone());
        let mut messages = agent_start_mutation.messages.unwrap_or(initial_messages);
        let mut new_messages: Vec<AgentMessage> = Vec::new();
        let mut turn_index: u64 = 0;
        let mut message_index: u64 = 0;

        (self.config.event_sink)(AgentEvent::AgentStart);

        loop {
            // Drain steer queue
            {
                let mut q = self.config.steer_queue.lock().expect("steer queue poisoned");
                messages.extend(q.drain(..));
            }

            // Inner turn loop
            loop {
                let result = self.run_turn(
                    &mut messages,
                    &mut new_messages,
                    &mut turn_index,
                    &mut message_index,
                    &system_prompt,
                    &signal,
                ).await;

                match result {
                    TurnResult::ToolUse => continue,
                    TurnResult::Stop => break,
                    TurnResult::Error(e) => {
                        (self.config.event_sink)(AgentEvent::Error { error: e.clone() });
                        (self.config.event_sink)(AgentEvent::AgentEnd { messages: messages.clone() });
                        let _ = self.config.hook_dispatcher.on_agent_end(&AgentEndCtx {
                            tenant_id: self.config.tenant_id.clone(),
                            session_id: self.config.session_id.clone(),
                            messages: messages.clone(),
                        }).await;
                        return Err(e);
                    }
                }
            }

            // Drain follow_up queue
            {
                let mut q = self.config.follow_up_queue.lock().expect("follow_up queue poisoned");
                let follow_ups: Vec<_> = q.drain(..).collect();
                if follow_ups.is_empty() { break; }
                messages.extend(follow_ups.clone());
                new_messages.extend(follow_ups);
            }
        }

        (self.config.event_sink)(AgentEvent::AgentEnd { messages: messages.clone() });
        let _ = self.config.hook_dispatcher.on_agent_end(&AgentEndCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            messages: messages.clone(),
        }).await;

        Ok(new_messages)
    }
```

- [ ] **Step 3: 实现内层 run_turn 方法**

```rust
    async fn run_turn(
        &self,
        messages: &mut Vec<AgentMessage>,
        new_messages: &mut Vec<AgentMessage>,
        turn_index: &mut u64,
        message_index: &mut u64,
        system_prompt: &Option<String>,
        signal: &CancellationToken,
    ) -> TurnResult {
        *turn_index += 1;
        (self.config.event_sink)(AgentEvent::TurnStart { turn_index: *turn_index });

        // 1. on_context hook
        let after_context_messages = messages.clone();
        let ctx_ctx = ContextCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            messages: messages.clone(),
        };
        let ctx_mutation = match catch_panic(self.config.hook_dispatcher.on_context(&ctx_ctx)).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("on_context hook failed: {}", e);
                ContextMutation::default()
            }
        };
        let mut transformed = ctx_mutation.messages.unwrap_or_else(|| messages.clone());

        // 1.5 Resolve orphan tool calls
        resolve_orphan_tool_calls(&mut transformed);

        // 2. Build LlmContext
        let mut stream_opts = self.config.stream_options.clone();
        let mut ctx = LlmContext {
            system_prompt: system_prompt.clone(),
            messages: transformed,
            tools: build_tool_defs(&self.config.tools),
        };

        // 2.5 on_before_provider_request hook
        let provider_req_ctx = ProviderRequestCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            model: self.config.model.clone(),
            turn_index: *turn_index,
            system_prompt: ctx.system_prompt.clone(),
            messages: ctx.messages.clone(),
            tools: ctx.tools.clone(),
            options: ProviderStreamOptions::from_options(&self.config.stream_options),
        };
        let provider_req_mutation = match catch_panic(self.config.hook_dispatcher.on_before_provider_request(&provider_req_ctx)).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("on_before_provider_request hook failed: {}", e);
                ProviderRequestMutation::default()
            }
        };
        apply_provider_request_mutation(&mut ctx, &mut stream_opts, provider_req_mutation);

        // 3. Call LLM with retry
        let (retry_count, mut assistant_msg) = match self.call_llm_with_retry(
            ctx, stream_opts, *message_index, signal
        ).await {
            Ok(result) => result,
            Err(e) => {
                (self.config.event_sink)(AgentEvent::Error { error: e.clone() });
                return TurnResult::Error(e);
            }
        };

        // 3.5 on_after_provider_response hook
        let provider_resp_ctx = ProviderResponseCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            model: self.config.model.clone(),
            turn_index: *turn_index,
            attempt: retry_count,
            messages_before: after_context_messages,
            content: assistant_msg.content.clone(),
            stop_reason: assistant_msg.stop_reason.clone(),
        };
        let provider_resp_mutation = match catch_panic(self.config.hook_dispatcher.on_after_provider_response(&provider_resp_ctx)).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("on_after_provider_response hook failed: {}", e);
                ProviderResponseMutation::default()
            }
        };
        apply_provider_response_mutation(&mut assistant_msg, provider_resp_mutation);

        // 4. Emit MessageEnd
        *message_index += 1;
        (self.config.event_sink)(AgentEvent::MessageEnd {
            message: AgentMessage::Assistant(assistant_msg.clone()),
        });
        new_messages.push(AgentMessage::Assistant(assistant_msg.clone()));
        messages.push(AgentMessage::Assistant(assistant_msg.clone()));

        // 5. Extract tool calls
        let tool_calls: Vec<&llm_client::ToolCall> = assistant_msg.content
            .iter()
            .filter_map(|c| match c {
                llm_client::Content::ToolCall(tc) => Some(tc),
                _ => None,
            })
            .collect();

        if tool_calls.is_empty() {
            match assistant_msg.stop_reason {
                StopReason::Error | StopReason::Aborted | StopReason::Length => {
                    let err_msg = assistant_msg.error_message.clone()
                        .unwrap_or_else(|| format!("LLM returned stop reason: {:?}", assistant_msg.stop_reason));
                    return TurnResult::Error(AgentError::LlmResponseError(err_msg));
                }
                _ => {}
            }
            (self.config.event_sink)(AgentEvent::TurnEnd { turn_index: *turn_index, messages: messages.clone() });
            let _ = catch_panic(self.config.hook_dispatcher.on_turn_end(&TurnEndCtx {
                tenant_id: self.config.tenant_id.clone(),
                session_id: self.config.session_id.clone(),
                turn_index: *turn_index,
                messages: messages.clone(),
            })).await;
            return TurnResult::Stop;
        }

        // 6. Execute tools
        let tool_results = self.execute_tools(tool_calls, signal).await;
        let mut all_terminate = !tool_results.is_empty();
        for result in &tool_results {
            new_messages.push(AgentMessage::ToolResult(result.clone()));
            messages.push(AgentMessage::ToolResult(result.clone()));
            let terminated = result.details.as_ref()
                .and_then(|d| d.get("_terminate"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !terminated { all_terminate = false; }
        }

        (self.config.event_sink)(AgentEvent::TurnEnd { turn_index: *turn_index, messages: messages.clone() });
        let _ = catch_panic(self.config.hook_dispatcher.on_turn_end(&TurnEndCtx {
            tenant_id: self.config.tenant_id.clone(),
            session_id: self.config.session_id.clone(),
            turn_index: *turn_index,
            messages: messages.clone(),
        })).await;

        if all_terminate {
            return TurnResult::Stop;
        }

        if assistant_msg.stop_reason == StopReason::ToolUse {
            TurnResult::ToolUse
        } else {
            match assistant_msg.stop_reason {
                StopReason::Error | StopReason::Aborted | StopReason::Length => {
                    let err_msg = assistant_msg.error_message.clone()
                        .unwrap_or_else(|| format!("LLM returned stop reason: {:?}", assistant_msg.stop_reason));
                    TurnResult::Error(AgentError::LlmResponseError(err_msg))
                }
                _ => TurnResult::Stop,
            }
        }
    }
```

- [ ] **Step 4: 实现 call_llm_with_retry**

```rust
    async fn call_llm_with_retry(
        &self,
        ctx: LlmContext,
        stream_opts: StreamOptions,
        message_index: u64,
        signal: &CancellationToken,
    ) -> Result<(u32, llm_client::AssistantMessage), AgentError> {
        for attempt in 0..self.config.max_retries {
            if signal.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            match self.config.provider.stream(
                &self.config.model,
                ctx.clone(),
                stream_opts.clone(),
                signal.child_token(),
            ).await {
                Ok(mut stream) => {
                    (self.config.event_sink)(AgentEvent::MessageStart { message_index });
                    
                    let mut assistant_content: Vec<llm_client::Content> = Vec::new();
                    let mut text_accum: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
                    let mut thinking_accum: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
                    let mut api = llm_client::Api {
                        provider: self.config.provider.provider_name().to_string(),
                        model: self.config.model.clone(),
                    };
                    let mut usage = llm_client::Usage {
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                        total_tokens: 0,
                    };
                    let mut stop_reason = StopReason::Stop;
                    let mut error_message: Option<String> = None;

                    while let Some(event) = stream.next().await {
                        if signal.is_cancelled() {
                            return Err(AgentError::Cancelled);
                        }
                        match event {
                            llm_client::AssistantMessageEvent::Start { .. } => {}
                            llm_client::AssistantMessageEvent::TextStart { .. } => {}
                            llm_client::AssistantMessageEvent::TextDelta { content_index, delta, .. } => {
                                text_accum.entry(content_index).or_default().push_str(&delta);
                                (self.config.event_sink)(AgentEvent::MessageUpdate { message_index, content_delta: delta });
                            }
                            llm_client::AssistantMessageEvent::TextEnd { content_index, text, .. } => {
                                let accumulated = text_accum.remove(&content_index).unwrap_or(text);
                                assistant_content.push(llm_client::Content::Text {
                                    text: accumulated,
                                    text_signature: None,
                                });
                            }
                            llm_client::AssistantMessageEvent::ThinkingStart { .. } => {}
                            llm_client::AssistantMessageEvent::ThinkingDelta { content_index, delta, .. } => {
                                thinking_accum.entry(content_index).or_default().push_str(&delta);
                            }
                            llm_client::AssistantMessageEvent::ThinkingEnd { content_index, thinking, .. } => {
                                let accumulated = thinking_accum.remove(&content_index).unwrap_or(thinking);
                                assistant_content.push(llm_client::Content::Thinking {
                                    thinking: accumulated,
                                    thinking_signature: None,
                                    redacted: false,
                                });
                            }
                            llm_client::AssistantMessageEvent::ToolCallStart { .. } => {}
                            llm_client::AssistantMessageEvent::ToolCallDelta { .. } => {}
                            llm_client::AssistantMessageEvent::ToolCallEnd { tool_call, .. } => {
                                assistant_content.push(llm_client::Content::ToolCall(tool_call));
                            }
                            llm_client::AssistantMessageEvent::Done { reason, message } => {
                                assistant_content = message.content;
                                api = message.api;
                                usage = message.usage;
                                stop_reason = reason;
                                return Ok((attempt, llm_client::AssistantMessage {
                                    content: assistant_content,
                                    provider: api.provider.clone(),
                                    model: api.model.clone(),
                                    api,
                                    usage,
                                    stop_reason,
                                    response_id: None,
                                    error_message: None,
                                    timestamp: std::time::SystemTime::now(),
                                }));
                            }
                            llm_client::AssistantMessageEvent::Error { error } => {
                                error_message = error.error_message.clone();
                                stop_reason = error.stop_reason.clone();
                                // Check if retryable
                                if attempt < self.config.max_retries - 1 {
                                    let err_str = error.error_message.clone().unwrap_or_default().to_lowercase();
                                    if err_str.contains("rate limit") || err_str.contains("overloaded") || err_str.contains("timeout") {
                                        let delay = std::time::Duration::from_millis(100 * 2_u64.pow(attempt));
                                        tokio::time::sleep(delay).await;
                                        continue;
                                    }
                                }
                                return Err(AgentError::LlmError(error));
                            }
                        }
                    }
                    
                    return Err(AgentError::LlmResponseError("stream ended without terminal event".to_string()));
                }
                Err(llm_client::LlmError::RateLimited) | Err(llm_client::LlmError::Overloaded) => {
                    if attempt < self.config.max_retries - 1 {
                        let delay = std::time::Duration::from_millis(100 * 2_u64.pow(attempt));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(AgentError::LlmError(llm_client::LlmError::RateLimited));
                }
                Err(e) => return Err(AgentError::LlmError(e)),
            }
        }
        
        Err(AgentError::LlmResponseError("all retries exhausted".to_string()))
    }
```

- [ ] **Step 5: 实现 execute_tools（替换现有工具执行逻辑）**

```rust
    async fn execute_tools(
        &self,
        tool_calls: Vec<&llm_client::ToolCall>,
        signal: &CancellationToken,
    ) -> Vec<llm_client::ToolResultMessage> {
        let mut results = Vec::new();

        let (sequential_calls, parallel_calls): (Vec<_>, Vec<_>) = tool_calls
            .into_iter()
            .partition(|tc| {
                self.config.tools.iter()
                    .find(|t| t.name() == tc.name)
                    .map(|t| t.execution_mode() == ToolExecutionMode::Sequential)
                    .unwrap_or(true)
            });

        for tc in sequential_calls {
            let result = self.execute_single_tool(tc, signal).await;
            results.push(result);
        }

        if !parallel_calls.is_empty() {
            let futures: Vec<_> = parallel_calls.iter()
                .map(|tc| self.execute_single_tool(tc, signal))
                .collect();
            let parallel_results = futures::future::join_all(futures).await;
            results.extend(parallel_results);
        }

        results
    }

    async fn execute_single_tool(
        &self,
        tc: &llm_client::ToolCall,
        _signal: &CancellationToken,
    ) -> llm_client::ToolResultMessage {
        (self.config.event_sink)(AgentEvent::ToolExecutionStart {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
        });

        let tool = self.config.tools.iter().find(|t| t.name() == tc.name).cloned();
        let result = match tool {
            Some(tool) => {
                let executor = ToolExecutor::new(
                    self.config.tenant_id.clone(),
                    self.config.session_id.clone(),
                    self.config.hook_dispatcher.clone(),
                    tool,
                );
                let on_progress = |update: crate::types::AgentToolProgressUpdate| {
                    (self.config.event_sink)(AgentEvent::ToolExecutionUpdate {
                        tool_call_id: tc.id.clone(),
                        content: update.content.clone(),
                    });
                };
                executor.execute_tool_call(tc, Some(&on_progress)).await
            }
            None => Err(AgentError::ToolNotFound(tc.name.clone())),
        };

        let result_msg = match result {
            Ok(msg) => msg,
            Err(e) => llm_client::ToolResultMessage {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                content: vec![],
                details: Some(serde_json::json!({"error": e.to_string()})),
                is_error: true,
                timestamp: std::time::SystemTime::now(),
            },
        };

        (self.config.event_sink)(AgentEvent::ToolExecutionEnd {
            tool_call_id: tc.id.clone(),
            result: result_msg.clone(),
        });
        result_msg
    }
```

- [ ] **Step 6: 验证编译**

Run: `cargo check -p agent-core`
Expected: PASS（可能需要逐步修复编译错误）

---

### Task 4.3: 更新 loop.rs 测试

**Files:**
- Modify: `crates/agent-core/src/loop.rs` (tests 模块)

**Steps:**

- [ ] **Step 1: 更新所有测试以使用 AgentLoopConfig**

将现有测试中的：
```rust
let loop_ = AgentLoop::new(tenant_id, session_id, model, provider, dispatcher, tools);
let results = loop_.run(system_prompt, messages, signal).await;
```

替换为：
```rust
let config = AgentLoopConfig {
    tenant_id: "t1".to_string(),
    session_id: "s1".to_string(),
    model: "test".to_string(),
    provider,
    hook_dispatcher: dispatcher,
    tools: vec![],
    system_prompt: Some("You are helpful.".to_string()),
    stream_options: StreamOptions::default(),
    max_retries: 3,
    steer_queue: Arc::new(Mutex::new(vec![])),
    follow_up_queue: Arc::new(Mutex::new(vec![])),
    event_sink: Arc::new(|event| {
        tracing::debug!("event: {:?}", event);
    }),
};
let loop_ = AgentLoop::new(config);
let results = loop_.run(messages, signal).await;
```

- [ ] **Step 2: 添加孤儿解析测试**

```rust
#[test]
fn test_resolve_orphan_tool_calls() {
    use llm_client::{ToolCall, AssistantMessage, Api, Usage};
    
    let mut messages = vec![
        AgentMessage::Assistant(AssistantMessage {
            content: vec![llm_client::Content::ToolCall(ToolCall {
                id: "call_1".to_string(),
                name: "test_tool".to_string(),
                arguments: serde_json::json!({}),
                thought_signature: None,
            })],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: Api { provider: "test".to_string(), model: "test".to_string() },
            usage: Usage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 0 },
            stop_reason: StopReason::ToolUse,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        }),
    ];

    resolve_orphan_tool_calls(&mut messages);
    assert_eq!(messages.len(), 2);
    match &messages[1] {
        AgentMessage::ToolResult(tr) => {
            assert_eq!(tr.tool_call_id, "call_1");
            assert!(tr.details.as_ref().unwrap()["_orphan"].as_bool().unwrap());
        }
        _ => panic!("expected tool result"),
    }
}

#[test]
fn test_no_orphan_when_resolved() {
    use llm_client::{ToolCall, AssistantMessage, Api, Usage};
    
    let mut messages = vec![
        AgentMessage::Assistant(AssistantMessage {
            content: vec![llm_client::Content::ToolCall(ToolCall {
                id: "call_1".to_string(),
                name: "test_tool".to_string(),
                arguments: serde_json::json!({}),
                thought_signature: None,
            })],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: Api { provider: "test".to_string(), model: "test".to_string() },
            usage: Usage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None, total_tokens: 0 },
            stop_reason: StopReason::ToolUse,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        }),
        AgentMessage::ToolResult(llm_client::ToolResultMessage {
            tool_call_id: "call_1".to_string(),
            tool_name: "test_tool".to_string(),
            content: vec![],
            details: None,
            is_error: false,
            timestamp: std::time::SystemTime::now(),
        }),
    ];

    resolve_orphan_tool_calls(&mut messages);
    assert_eq!(messages.len(), 2);
}
```

- [ ] **Step 3: 运行所有 loop 测试**

Run: `cargo test -p agent-core loop_`
Expected: ALL PASS

---

## Phase 5: CompactionActor 封装 (P1 — ~1h)

> 当前 compaction.rs 已有完整的自由函数实现。本 phase 将其封装为 CompactionActor 结构体，保持向后兼容。

### Task 5.1: 创建 CompactionActor

**Files:**
- Modify: `crates/agent-core/src/compaction.rs`

**Steps:**

- [ ] **Step 1: 在 compaction.rs 顶部添加 CompactionActor**

在现有代码之后、测试之前添加：

```rust
use std::sync::Arc;

/// Actor wrapper around the compaction logic.
pub struct CompactionActor {
    pub config: CompactionSettings,
    provider: Arc<dyn llm_client::LlmProvider>,
    model: String,
}

impl CompactionActor {
    pub fn new(
        config: CompactionSettings,
        provider: Arc<dyn llm_client::LlmProvider>,
        model: String,
    ) -> Self {
        Self { config, provider, model }
    }

    /// Prepare compaction data from session entries.
    pub fn prepare(&self, entries: &[crate::types::SessionEntry]) -> Result<CompactionPreparation, AgentError> {
        prepare_compaction(entries, &self.config)
            .ok_or_else(|| AgentError::CompactionFailed("Failed to prepare compaction".to_string()))
    }

    /// Execute compaction using prepared data.
    pub async fn compact(
        &self,
        entries: &[crate::types::SessionEntry],
        custom_instructions: Option<&str>,
        signal: CancellationToken,
    ) -> Result<CompactionResult, AgentError> {
        let preparation = self.prepare(entries)?;
        compact(
            &preparation,
            self.provider.as_ref(),
            &self.model,
            self.config.reserve_tokens,
            custom_instructions,
            signal,
        ).await
    }
}
```

- [ ] **Step 2: 添加到 lib.rs**

```rust
pub use compaction::{CompactionActor, CompactionSettings, CompactionResult, CompactionPreparation};
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p agent-core`
Expected: PASS

---

## Phase 6: SessionActor 重构 (P0 — ~3h)

> 集成事件队列、RecoveryStateMachine、complete/continue_、auto-compaction。

### Task 6.1: 添加 CompactionFailed 错误变体

**Files:**
- Modify: `crates/agent-core/src/error.rs`

**Steps:**

- [ ] **Step 1: 添加 CompactionFailed**

```rust
    #[error("compaction failed: {0}")]
    CompactionFailed(String),
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p agent-core`
Expected: PASS

---

### Task 6.2: 重构 SessionActor

**Files:**
- Modify: `crates/agent-core/src/session.rs`

**Steps:**

- [ ] **Step 1: 更新 SessionActor 结构体**

添加字段：
```rust
use crate::events::{AgentEvent, AgentEventListener};
use crate::error_recovery::RecoveryStateMachine;
use crate::compaction::CompactionActor;

pub struct SessionActor {
    // ... existing fields ...
    
    // New fields
    compaction_actor: Option<Arc<CompactionActor>>,
    recovery: RecoveryStateMachine,
    event_listeners: Vec<Arc<dyn AgentEventListener>>,
    event_tx: Option<tokio::sync::mpsc::Sender<QueuedEvent>>,
    event_processor_handle: Option<tokio::task::JoinHandle<()>>,
    is_streaming: bool,
}

struct QueuedEvent {
    event: AgentEvent,
    new_messages: Vec<AgentMessage>,
}
```

- [ ] **Step 2: 实现事件队列**

```rust
impl SessionActor {
    fn spawn_event_processor(&mut self) -> tokio::sync::mpsc::Sender<QueuedEvent> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<QueuedEvent>(1024);
        let listeners = self.event_listeners.clone();
        let hook_dispatcher = self.hook_dispatcher.clone();
        let tenant_id = self.tenant_id.clone();
        let session_id = self.session_id.clone();

        let handle = tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                // Extension hooks for turn_end / agent_end
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

                // AgentEventListeners
                for listener in &listeners {
                    let _ = listener.on_event(&queued.event).await;
                }
            }
        });

        self.event_processor_handle = Some(handle);
        tx
    }
```

- [ ] **Step 3: 更新 SessionActor::new**

添加 compaction_actor 参数，初始化新字段：

```rust
    pub fn new(
        tenant_id: String,
        session_id: String,
        system_prompt: String,
        model: String,
        context_window: u64,
        provider: Arc<dyn llm_client::LlmProvider>,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        tools: Vec<AgentToolRef>,
        store: Option<Arc<dyn SessionStore>>,
        compaction_settings: Option<crate::compaction::CompactionSettings>,
        compaction_actor: Option<Arc<CompactionActor>>,
    ) -> Self {
        // ... existing init code ...
        
        let mut actor = Self {
            // ... existing fields ...
            compaction_actor,
            recovery: RecoveryStateMachine::new(3),
            event_listeners: Vec::new(),
            event_tx: None,
            event_processor_handle: None,
            is_streaming: false,
        };

        let event_tx = actor.spawn_event_processor();
        actor.event_tx = Some(event_tx);

        actor
    }
```

- [ ] **Step 4: 实现 complete 和 continue_**

```rust
    pub async fn complete(&mut self, text: String) -> Result<String, AgentError> {
        let messages = self.prompt(text).await?;
        let text_content: Vec<String> = messages.iter().filter_map(|m| {
            if let AgentMessage::Assistant(a) = m {
                Some(a.content.iter().filter_map(|c| match c {
                    llm_client::Content::Text { text, .. } => Some(text.clone()),
                    _ => None,
                }).collect::<Vec<_>>().join(" "))
            } else {
                None
            }
        }).collect();
        Ok(text_content.join("\n"))
    }

    pub async fn continue_(&mut self) -> Result<Vec<AgentMessage>, AgentError> {
        self.run_with_messages(None).await
    }
```

- [ ] **Step 5: 重构 prompt 为 run_with_messages**

将现有 `prompt` 的核心逻辑提取到 `run_with_messages`，并集成 AgentLoopConfig：

```rust
    pub async fn prompt(&mut self, text: String) -> Result<Vec<AgentMessage>, AgentError> {
        let user_msg = AgentMessage::User(llm_client::UserMessage {
            content: vec![llm_client::Content::Text { text, text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        });
        self.push_message(user_msg);
        self.run_with_messages(None).await
    }

    async fn run_with_messages(&mut self, _add_user_msg: Option<String>) -> Result<Vec<AgentMessage>, AgentError> {
        self.is_streaming = true;
        self.abort_token = CancellationToken::new();

        // Drain steer queue
        {
            let mut q = self.steer_queue.lock().expect("steer queue poisoned");
            if !q.is_empty() {
                let steer_msgs: Vec<_> = q.drain(..).collect();
                for msg in steer_msgs {
                    self.push_message(msg);
                }
            }
        }

        let messages = self.messages().to_vec();

        // Build event sink
        let event_tx = self.event_tx.clone();
        let event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync> = Arc::new(move |event| {
            if let Some(tx) = &event_tx {
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
            stream_options: llm_client::StreamOptions::default(),
            max_retries: 3,
            event_sink,
            steer_queue: Arc::new(Mutex::new(vec![])),  // SessionActor 自己管理 steer
            follow_up_queue: Arc::new(Mutex::new(vec![])),  // SessionActor 自己管理 follow_up
        };

        let new_msgs = match AgentLoop::new(config).run(messages, self.abort_token.child_token()).await {
            Ok(msgs) => {
                self.is_streaming = false;
                
                // Post-processing: recovery and compaction
                if let Some(AgentMessage::Assistant(assistant)) = msgs.iter().rfind(|m| matches!(m, AgentMessage::Assistant(_))) {
                    let action = self.recovery.evaluate(assistant);
                    match action {
                        crate::error_recovery::RecoveryAction::RetryAfterCompaction { .. } => {
                            // TODO: integrate compaction
                            self.recovery.mark_success();
                        }
                        crate::error_recovery::RecoveryAction::RetryAfterBackoff { delay_ms } => {
                            tokio::select! {
                                _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                                _ = self.abort_token.cancelled() => {
                                    self.recovery.reset();
                                    return Ok(vec![]);
                                }
                            }
                            self.recovery.mark_success();
                            return self.continue_().await;
                        }
                        crate::error_recovery::RecoveryAction::Abort { .. } => {
                            self.recovery.mark_success();
                            return Ok(vec![]);
                        }
                        crate::error_recovery::RecoveryAction::Continue => {
                            self.recovery.mark_success();
                        }
                    }
                }
                
                for msg in &msgs {
                    self.push_message(msg.clone());
                }
                Ok(msgs)
            }
            Err(e) => {
                self.is_streaming = false;
                Err(e)
            }
        };

        // Drain follow_up queue and loop
        {
            let mut q = self.follow_up_queue.lock().expect("follow_up queue poisoned");
            if !q.is_empty() {
                let follow_ups: Vec<_> = q.drain(..).collect();
                for msg in follow_ups {
                    self.push_message(msg);
                }
                return self.continue_().await;
            }
        }

        new_msgs
    }
```

- [ ] **Step 6: 添加 setter 和 listener 方法**

```rust
    pub fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    pub fn add_event_listener(&mut self, listener: Arc<dyn AgentEventListener>) {
        self.event_listeners.push(listener);
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn set_tools(&mut self, tools: Vec<AgentToolRef>) {
        self.tools = tools;
    }

    pub fn set_stream_options(&mut self, options: llm_client::StreamOptions) {
        // self.stream_options = options;  // 需要添加 stream_options 字段
    }

    pub fn set_max_retries(&mut self, max_retries: u32) {
        // self.max_retries = max_retries;  // 需要添加字段
    }
```

- [ ] **Step 7: 更新测试**

所有 `SessionActor::new` 调用需要添加 `compaction_actor: None` 参数。

- [ ] **Step 8: 验证编译和测试**

Run: `cargo test -p agent-core`
Expected: ALL PASS

---

## Phase 7: lib.rs 整合 (P1 — ~15 min)

### Task 7.1: 更新 lib.rs 导出

**Files:**
- Modify: `crates/agent-core/src/lib.rs`

**Steps:**

- [ ] **Step 1: 添加新模块导出**

```rust
pub mod compaction;
pub mod context;
pub mod error;
pub mod error_recovery;
pub mod events;
pub mod hook_dispatcher;
#[path = "loop.rs"]
pub mod loop_;
pub mod mutations;
pub mod provider_opts;
pub mod session;
pub mod store;
pub mod tool;
pub mod types;

pub(crate) mod util;

pub use compaction::{CompactionActor, CompactionSettings, CompactionResult, CompactionPreparation};
pub use context::*;
pub use error::AgentError;
pub use error_recovery::{RecoveryAction, RecoveryStateMachine};
pub use events::{AgentEvent, AgentEventListener};
pub use hook_dispatcher::HookDispatcher;
pub use loop_::{AgentLoop, AgentLoopConfig, TurnResult, resolve_orphan_tool_calls};
pub use mutations::*;
pub use provider_opts::ProviderStreamOptions;
pub use session::SessionActor;
pub use store::SessionStore;
pub use tool::ToolExecutor;
pub use types::*;
```

- [ ] **Step 2: 最终验证**

Run: `cargo test -p agent-core`
Expected: ALL PASS

Run: `cargo check -p agent-core`
Expected: PASS (no errors)

---

## Phase 8: 测试补全 (P1 — ~2h)

### Task 8.1: AgentLoop 集成测试

**Files:**
- Create: `crates/agent-core/tests/loop_integration_tests.rs`

**Steps:**

- [ ] **Step 1: 测试双层循环（follow_up）**

```rust
use agent_core::*;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn test_follow_up_triggers_second_turn() {
    // 创建一个 provider：第一次返回 Stop，第二次也返回 Stop
    // 在 follow_up_queue 中注入一条消息
    // 验证 AgentLoop 执行了两轮外层循环
}
```

- [ ] **Step 2: 测试 steer 注入**

```rust
#[tokio::test]
async fn test_steer_injection_before_turn() {
    // 验证 steer_queue 中的消息在 turn 开始前被注入
}
```

- [ ] **Step 3: 测试 LLM 重试**

```rust
#[tokio::test]
async fn test_rate_limit_retry_with_backoff() {
    // Mock provider 前两次返回 RateLimited，第三次成功
    // 验证最终成功且有一定的延迟
}
```

- [ ] **Step 4: 测试事件发射顺序**

```rust
#[tokio::test]
async fn test_event_sequence() {
    // 注册一个事件监听器，收集所有事件
    // 验证顺序：AgentStart → TurnStart → MessageStart → ... → TurnEnd → AgentEnd
}
```

---

### Task 8.2: SessionActor 集成测试

**Files:**
- Create: `crates/agent-core/tests/session_integration_tests.rs`

**Steps:**

- [ ] **Step 1: 测试 complete() 只返回文本**

```rust
#[tokio::test]
async fn test_complete_returns_only_text() {
    // 调用 complete()
    // 验证返回 String，不是 Vec<AgentMessage>
}
```

- [ ] **Step 2: 测试错误恢复（backoff retry）**

```rust
#[tokio::test]
async fn test_session_auto_retry_after_rate_limit() {
    // Mock provider 返回 rate limit error
    // 验证 SessionActor 自动重试
}
```

- [ ] **Step 3: 测试事件监听器注册**

```rust
#[tokio::test]
async fn test_event_listener_receives_events() {
    // 注册自定义监听器
    // 验证收到 AgentStart 和 AgentEnd
}
```

---

## Phase 优先级与时间估计

| Phase | 内容 | 优先级 | 预估时间 | 阻塞关系 |
|---|---|---|---|---|
| Phase 1 | 事件系统 (AgentEvent) | **P0** | ~30 min | 无 |
| Phase 2 | ProviderStreamOptions | **P0** | ~15 min | 无 |
| Phase 3 | 错误恢复状态机 | **P0** | ~30 min | 无 |
| Phase 4 | AgentLoop 重构（双层循环 + hooks + 重试） | **P0** | ~4h | 依赖 Phase 1-3 |
| Phase 5 | CompactionActor 封装 | **P1** | ~1h | 无 |
| Phase 6 | SessionActor 重构 | **P0** | ~3h | 依赖 Phase 1-5 |
| Phase 7 | lib.rs 整合 | **P1** | ~15 min | 依赖 Phase 1-6 |
| Phase 8 | 测试补全 | **P1** | ~2h | 依赖 Phase 1-7 |

**P0 总计**: ~8.5h | **P1 总计**: ~3.25h | **全部总计**: ~11.75h

**风险点:**
1. **Phase 4 (AgentLoop 重构)** — 最大变更量，所有现有测试需要更新签名
2. **Phase 6 (SessionActor)** — 数据模型变更，steer/follow_up 改为 Arc<Mutex<>>
3. **事件队列** — 需要确保不会因为慢监听器阻塞 AgentLoop

**联合开发顺序:**
```
第 1 步: Phase 1-3 并行（独立模块，可并行开发）
第 2 步: Phase 4（AgentLoop 重构，依赖 Phase 1-3）
第 3 步: Phase 5-6 并行（CompactionActor 和 SessionActor 相对独立）
第 4 步: Phase 7-8（整合和测试）
```
