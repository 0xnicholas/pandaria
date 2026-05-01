# llm-client 详细模块规格

**Date:** 2026-05-02
**Status:** Draft
**Reference:** pi.dev LLM layer (`packages/ai/`), AGENTS.md (ADR-001, ADR-005)

---

## 模块定位

服务端 crate 依赖链的叶子节点（不被任何其他 crate 依赖）。为 `agent-core` 提供：

- LLM 消息类型系统（Message, Content, ToolCall, ToolDef, Usage, StopReason）
- 流式事件协议（AssistantMessageEvent / AssistantMessageEventStream）
- Provider 抽象 trait（LlmProvider）
- 指数退避重试工具

## 依赖方向

```
agent-core → llm-client
```

禁止反向依赖。

---

## 1. 文件结构

```
crates/llm-client/
  Cargo.toml
  README.md
  src/
    lib.rs              # re-exports
    types.rs            # Message, Content, ToolCall, ToolDef, Usage, StopReason, Api
    error.rs            # LlmError (thiserror)
    context.rs          # LlmContext
    streaming.rs        # AssistantMessageEvent, AssistantMessageEventStream
    provider.rs         # LlmProvider trait, StreamOptions, ReasoningLevel
    retry.rs            # with_retry() utility
    util.rs             # extract_tool_calls, build_tool_defs
    providers/
      mod.rs            # Provider registry
      anthropic.rs      # Anthropic Messages API
      openai.rs         # OpenAI Chat Completions
      google.rs         # Google Gemini generateContent
  tests/
    types_serde.rs      # JSON round-trip tests
    streaming_tests.rs  # EventStream behavior
    anthropic_tests.rs  # Mock HTTP integration tests
    openai_tests.rs     # Mock HTTP integration tests
```

---

## 2. 依赖

```toml
[dependencies]
tokio = { workspace = true }
async-trait = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
reqwest = { version = "0.12", features = ["stream", "json"] }
eventsource-stream = "0.6"
tokio-util = { version = "0.7", features = ["io"] }
secrecy = { version = "0.10", features = ["serde"] }

[dev-dependencies]
wiremock = "0.6"
tokio-test = "0.4"
```

---

## 3. 类型系统

### 3.1 Content 枚举

```rust
use serde::{Deserialize, Serialize};

/// Content block within a message.
/// Mirrors pi.dev's content types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Content {
    #[serde(rename = "text")]
    Text {
        text: String,
    },
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default)]
        redacted: bool,
    },
    #[serde(rename = "image")]
    Image {
        /// Base64 encoded image data
        data: String,
        /// MIME type, e.g. "image/png"
        mime_type: String,
    },
}
```

### 3.2 ToolCall

```rust
/// A tool call embedded in an assistant message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}
```

### 3.3 ToolDef

```rust
/// A tool definition exposed to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's parameters
    pub parameters: serde_json::Value,
}
```

### 3.4 消息类型

```rust
use std::time::SystemTime;

/// A user message from the conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserMessage {
    pub role: String,               // "user"
    pub content: Vec<Content>,
    #[serde(with = "system_time_serde")]
    pub timestamp: SystemTime,
}

/// An assistant message (LLM response).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantMessage {
    pub role: String,               // "assistant"
    pub content: Vec<Content>,
    pub api: Api,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(with = "system_time_serde")]
    pub timestamp: SystemTime,
}

/// A tool result message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResultMessage {
    pub role: String,               // "toolResult"
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(with = "system_time_serde")]
    pub timestamp: SystemTime,
}

/// Unified message type covering all roles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
}
```

### 3.5 Api / Usage / StopReason

```rust
/// Metadata about the LLM provider call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Api {
    pub provider: String,   // "anthropic" | "openai" | "google"
    pub model: String,      // e.g. "claude-sonnet-4-20250514"
}

/// Token usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
}

/// Reason the LLM stopped generating.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}
```

### 3.6 序列化辅助

```rust
/// Custom serde for SystemTime → Unix timestamp (seconds).
mod system_time_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(time: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let dur = time.duration_since(UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH — clock is incorrect");
        dur.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let secs: u64 = Deserialize::deserialize(d)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}
```

---

## 4. 流式事件协议

### 4.1 AssistantMessageEvent

所有 provider 将其原生流式事件标准化为此统一事件集。

```rust
/// Unified streaming event from any LLM provider.
/// Each event carries a `partial` snapshot of the AssistantMessage at that instant.
#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    // ── 生命周期 ──
    /// Stream started. `partial` contains initial metadata (api, model, response_id, usage).
    Start {
        partial: AssistantMessage,
    },

    // ── 文本流（按 content_index 区分多个 text block）──
    TextStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    TextDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    TextEnd {
        content_index: usize,
        text: String,           // full accumulated text
        partial: AssistantMessage,
    },

    // ── 推理流（thinking / extended reasoning）──
    ThinkingStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    ThinkingEnd {
        content_index: usize,
        thinking: String,        // full thinking text
        partial: AssistantMessage,
    },

    // ── 工具调用流（参数流式增量 + 最终 parsed ToolCall）──
    ToolCallStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    /// Raw JSON fragment for in-progress tool call argument accumulation.
    ToolCallDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    /// Tool call complete with parsed arguments.
    ToolCallEnd {
        content_index: usize,
        tool_call: ToolCall,
        partial: AssistantMessage,
    },

    // ── 终止事件 ──
    /// Generation completed successfully.
    Done {
        reason: StopReason,
        message: AssistantMessage,
    },
    /// Generation errored or was aborted. `error` contains the error_message and stop_reason.
    Error {
        error: AssistantMessage,
    },
}
```

### 4.2 AssistantMessageEventStream

```rust
/// Internal implementation: wraps `tokio::sync::mpsc::Receiver<AssistantMessageEvent>`.
/// Provider implementations push events into the channel via the sender side.
pub struct AssistantMessageEventStream {
    rx: tokio::sync::mpsc::Receiver<AssistantMessageEvent>,
    final_message: Option<Result<AssistantMessage, LlmError>>,
    terminated: bool,
}

impl AssistantMessageEventStream {
    /// Create a new stream with internal channel.
    /// Returns (stream, sender) — sender is passed to the provider.
    pub fn new(buffer: usize) -> (Self, tokio::sync::mpsc::Sender<AssistantMessageEvent>);

    /// Await the next event. Returns None when the stream ends.
    pub async fn next(&mut self) -> Option<AssistantMessageEvent>;

    /// Consume the stream and return the final AssistantMessage.
    ///
    /// Behavior:
    /// - If the stream has already been partially consumed via `next()`, `to_message()`
    ///   continues draining remaining events until `Done` or `Error`.
    /// - If `Done` is received: returns `Ok(message)`.
    /// - If `Error` is received: returns `Err(LlmError)` with the error content.
    /// - If the stream ends without `Done` or `Error` (channel closed): returns
    ///   `Err(LlmError::StreamError)`.
    /// - This method blocks until the underlying provider task sends a terminal event.
    pub async fn to_message(mut self) -> Result<AssistantMessage, LlmError>;
}
```

---

## 5. LlmProvider trait

```rust
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Abstraction over LLM provider backends.
/// v0.1 implementations: Anthropic, OpenAI, Google.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Return the API identifier for this provider.
    fn api(&self) -> Api;

    /// Stream a response from the LLM.
    ///
    /// Errors returned from this method indicate the request could not be initiated
    /// (connection failure, auth failure). Runtime errors during streaming are
    /// delivered as `AssistantMessageEvent::Error`.
    async fn stream(
        &self,
        model: &str,
        context: &LlmContext,
        options: &StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError>;
}
```

---

## 6. LlmContext / StreamOptions

```rust
/// The full context passed to the LLM for inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmContext {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Option<Vec<ToolDef>>,
}

/// Configuration for a single streaming request.
#[derive(Debug, Clone)]
pub struct StreamOptions {
    /// Override the environment-resolved API key.
    pub api_key: Option<secrecy::SecretString>,
    /// HTTP request timeout. Default: 60s.
    pub timeout: std::time::Duration,
    /// Maximum output tokens.
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0–2).
    pub temperature: Option<f32>,
    /// Top-p nucleus sampling (0–1).
    pub top_p: Option<f32>,
    /// Extended reasoning level.
    pub reasoning: Option<ReasoningLevel>,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            api_key: None,
            timeout: std::time::Duration::from_secs(60),
            max_tokens: None,
            temperature: None,
            top_p: None,
            reasoning: None,
        }
    }
}

/// API key resolution priority:
///
/// 1. `StreamOptions::api_key` (explicit override)
/// 2. Environment variable per provider:
///    - Anthropic: `ANTHROPIC_API_KEY`
///    - OpenAI:   `OPENAI_API_KEY`
///    - Google:   `GOOGLE_API_KEY`
/// 3. If neither source provides a key, `stream()` returns `Err(LlmError::AuthError)`.
///
/// Keys are stored in `secrecy::SecretString` to prevent accidental logging.

/// Extended reasoning / thinking level.
/// Maps to provider-specific parameters (Anthropic thinking budget, OpenAI reasoning_effort, Google thinking level).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningLevel {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}
```

---

## 7. 错误处理

### 7.1 LlmError

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("rate limited: {0}")]
    RateLimited(String),

    #[error("provider overloaded: {0}")]
    Overloaded(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("provider error: {0}")]
    ProviderError(String),

    #[error("request timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("authentication failed: {0}")]
    AuthError(String),

    #[error("context overflow: {0}")]
    ContextOverflow(String),

    #[error("cancelled")]
    Cancelled,

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("stream error: {0}")]
    StreamError(String),
}
```

### 7.2 安全约束

- **API Key 禁止出现在日志/tracing/错误消息/panic 中。** 使用 `secrecy::SecretString` 包装 key。SecretString 的 `Debug` impl 输出 `[REDACTED]`。
- Provider 实现中解析 HTTP 响应体时，不得在 error message 中包含原始响应（响应体可能包含 key）。
- tracing span 仅记录 `provider`, `model`, `retry_count`，不记录任何 auth header。

### 7.3 错误编码约定

借鉴 pi.dev：provider 实现 **不抛出** 运行时错误。所有发生在流式传输期间的错误（HTTP 断连、JSON 解析失败、stop_reason = error）都编码为 `AssistantMessageEvent::Error`，通过 stream channel 传递。仅当请求完全无法发起时才从 `stream()` 返回 `Err(LlmError)`。

```rust
// Provider 内部伪代码
async fn stream(...) -> Result<AssistantMessageEventStream, LlmError> {
    let (stream, tx) = AssistantMessageEventStream::new(32);

    let client = self.client.clone();
    tokio::spawn(async move {
        let result = try_stream(client, model, context, options, &tx, signal).await;
        if let Err(e) = result {
            // 编码为 Error 事件，不抛出
            let _ = tx.send(AssistantMessageEvent::Error {
                error: error_to_assistant_message(e),
            }).await;
        }
    });

    Ok(stream)
}
```

---

## 8. 指数退避重试

```rust
use std::future::Future;
use std::time::Duration;
use tracing::instrument;

/// Retry an LLM request with exponential backoff.
///
/// Retry triggers:
///   - LlmError::RateLimited
///   - LlmError::Overloaded
///   - LlmError::Timeout
///
/// Backoff: 100ms → 200ms → 400ms (2^attempt * 100ms).
///
/// Maximum 3 retries total (4 attempts including the initial call).
#[instrument(skip(operation), fields(retry_count))]
pub async fn with_retry<F, Fut>(
    operation: F,
    max_retries: u32,
) -> Result<Fut::Output, LlmError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<AssistantMessageEventStream, LlmError>>
{
    let mut attempt = 0;

    loop {
        match operation().await {
            Ok(stream) => {
                if attempt > 0 {
                    tracing::Span::current().record("retry_count", attempt);
                    tracing::info!(retry_count = attempt, "LLM request succeeded after retries");
                }
                return Ok(stream);
            }
            Err(e) if attempt < max_retries && is_retryable(&e) => {
                let delay = Duration::from_millis(100 * 2u64.pow(attempt));
                tracing::Span::current().record("retry_count", attempt + 1);
                tracing::warn!(
                    retry_count = attempt + 1,
                    delay_ms = delay.as_millis(),
                    error = %e,
                    "retrying LLM request"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_retryable(e: &LlmError) -> bool {
    matches!(e, LlmError::RateLimited(_) | LlmError::Overloaded(_) | LlmError::Timeout(_))
}
```

---

## 9. Provider 实现细节

### 9.1 AnthropicProvider

```
Base URL: https://api.anthropic.com/v1/messages
Header:   x-api-key, anthropic-version: 2023-06-01

请求转换:
  system_prompt → system: [{ text, cache_control: { type: "ephemeral" } }]
  messages → messages: [{ role, content }]
    - User: content → [{ type: "text", text }, { type: "image", source: { data, media_type } }]
    - Assistant: content → [{ type: "text", text }, { type: "tool_use", ... }]
    - ToolResult → { role: "user", content: [{ type: "tool_result", tool_use_id, content }] }
  tools → tools: [{ name, description, input_schema }]
  cache_control: 最后一个 system text 块 + 最后一条 message 附加 cache_control

流式解析 (SSE):
  message_start        → 提取 usage, response_id → Start 事件
  content_block_start  → 创建 content block
    - "text"            → TextStart
    - "tool_use"        → ToolCallStart (name, id)
    - "thinking"        → ThinkingStart
    - "redacted_thinking" → ThinkingStart(redacted=true)
  content_block_delta  →
    - text_delta        → TextDelta
    - input_json_delta  → ToolCallDelta（累积到 partialJson）
    - thinking_delta    → ThinkingDelta
    - signature_delta   → 更新 thinking/thought signature
  content_block_stop    →
    - text              → TextEnd
    - tool_use          → partialJson 解析为 ToolCall → ToolCallEnd
    - thinking          → ThinkingEnd
  message_delta         → 更新 usage, stop_reason
  message_stop          → Done(对应 stop_reason)

Stop reason 映射:
  end_turn   → StopReason::Stop
  max_tokens → StopReason::Length
  tool_use   → StopReason::ToolUse
  refusal    → StopReason::Error
```

### 9.2 OpenAiProvider

```
Base URL: https://api.openai.com/v1/chat/completions
Header:   Authorization: Bearer <key>

请求转换:
  system_prompt → messages[0]: { role: "system", content: text }
  messages → messages: [{ role, content }]
    - User: content → [{ type: "text", text }, { type: "image_url", image_url: { url: "data:..." } }]
    - Assistant: content → [{ type: "text", text }, { tool_calls: [...] }]
    - ToolResult → { role: "tool", tool_call_id, content }
  tools → tools: [{ type: "function", function: { name, description, parameters } }]
  reasoning → reasoning_effort (if ReasoningLevel set)

流式解析 (SSE):
  response.created     → Start
  choice.delta.content → TextDelta（第一个 delta 前自动发 TextStart）
  choice.delta.tool_calls[i] →
    - id + function.name 出现 → ToolCallStart
    - function.arguments   → ToolCallDelta
  choice.delta.reasoning_content (或 reasoning/reasoning_text) → ThinkingDelta
  choice.finish_reason  → Done

Stop reason 映射:
  stop        → StopReason::Stop
  length      → StopReason::Length
  tool_calls  → StopReason::ToolUse
  content_filter → StopReason::Error
```

### 9.3 GoogleProvider

```
Base URL: https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent
Header:   x-goog-api-key 或 Authorization: Bearer

请求转换:
  system_prompt → systemInstruction: { parts: [{ text }] }
  messages → contents: [{ role, parts }]
    - User → role: "user", parts: [{ text }]
    - Assistant → role: "model", parts: [{ text } | { functionCall } | { thought: true, text }]
    - ToolResult → role: "user", parts: [{ functionResponse: { name, response } }]
  tools → tools: [{ functionDeclarations: [{ name, description, parametersJsonSchema }] }]

流式解析 (JSON stream):
  每个 chunk 是一个 GenerateContentResponse
  candidates[0].content.parts[] →
    - { text, thought: true } → ThinkingDelta
    - { text }               → TextDelta
    - { functionCall }       → ToolCallEnd（一次性完整到达，非增量）

Stop reason 映射:
  STOP        → StopReason::Stop
  MAX_TOKENS  → StopReason::Length
  ERROR 等     → StopReason::Error
```

---

## 10. 工具函数

```rust
/// Extract tool calls from a Vec<Content>.
/// Returns all Content::ToolCall entries.
pub fn extract_tool_calls(content: &[Content]) -> Vec<ToolCall>;

/// Build a Vec<ToolDef> from a tool definition iterator.
pub fn build_tool_defs(tools: &[impl AsRef<ToolDef>]) -> Vec<ToolDef>;
```

---

## 11. 测试计划

### 11.1 类型序列化测试 (`tests/types_serde.rs`)

| 测试 | 验证点 |
|---|---|
| `test_user_message_roundtrip` | UserMessage JSON serde 往返 |
| `test_assistant_message_roundtrip` | AssistantMessage + ToolCall + Thinking 往返 |
| `test_tool_result_roundtrip` | ToolResultMessage 往返 |
| `test_message_enum_serialization` | Message enum tag 序列化（role 字段） |
| `test_stop_reason_serde` | StopReason snake_case 序列化 |
| `test_tool_def_serialization` | ToolDef JSON Schema 往返 |
| `test_content_text_variant` | Content::Text 序列化 |
| `test_content_tool_call_variant` | Content::ToolCall 序列化 |
| `test_content_thinking_variant` | Content::Thinking（含/不含 signature） |
| `test_content_image_variant` | Content::Image 序列化 |

### 11.2 流式事件测试 (`tests/streaming_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_event_stream_push_next` | push → next 正常消费事件序列 |
| `test_event_stream_to_message_done` | Done 事件 → to_message 返回 Ok(message) |
| `test_event_stream_to_message_error` | Error 事件 → to_message 返回 Err |
| `test_event_stream_next_returns_none_after_done` | Done 后 next() 返回 None |
| `test_event_stream_content_index_tracking` | 多个 content_index 事件交替正常 |

### 11.3 Provider Mock 测试 (`tests/{provider}_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_anthropic_basic_text_stream` | Mock SSE → TextStart/TextDelta/TextEnd/Done |
| `test_anthropic_tool_call_streaming` | input_json_delta 累积 → ToolCallEnd |
| `test_anthropic_thinking_streaming` | thinking_delta → ThinkingEnd |
| `test_openai_basic_text_stream` | Mock SSE → TextStart/TextDelta/TextEnd/Done |
| `test_openai_tool_call_streaming` | tool_calls delta → ToolCallEnd |
| `test_openai_reasoning_stream` | reasoning_content → ThinkingEnd |
| `test_google_basic_text_stream` | Mock JSON stream → TextStart/TextDelta/TextEnd/Done |
| `test_google_tool_call` | functionCall → ToolCallEnd |

### 11.4 重试测试 (`tests/retry_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_retry_success_after_rate_limit` | 2 次 RateLimited → 第 3 次成功 |
| `test_retry_exhausted` | 3 次 RateLimited → 返回 LlmError |
| `test_no_retry_on_invalid_request` | InvalidRequest 不触发重试 |
| `test_no_retry_on_auth_error` | AuthError 不触发重试 |
| `test_exponential_backoff_timing` | 延迟 100ms → 200ms → 400ms |

### 11.5 安全测试 (`tests/security_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_api_key_not_in_error_display` | LlmError::display() 不含 api_key |
| `test_secret_string_debug_redacted` | SecretString::debug() 输出 [REDACTED] |
| `test_provider_error_no_raw_body` | Provider 错误消息不含原始 HTTP body |

---

## 12. 与 agent-core 的接口契约

agent-core spec 中引用的 llm-client 类型必须准确匹配：

| agent-core 引用 | llm-client 类型 |
|---|---|
| `use llm_client::ToolResultMessage` | `types.rs` → `ToolResultMessage` |
| `Content::Text { text }` | `types.rs` → `Content::Text` |
| `Content::ToolCall(ToolCall { ... })` | `types.rs` → `Content::ToolCall` |
| `UserMessage { role, content, timestamp }` | `types.rs` → `UserMessage` |
| `LlmContext { system_prompt, messages, tools }` | `context.rs` → `LlmContext` |
| `StopReason::ToolUse` | `types.rs` → `StopReason::ToolUse` |
| `ToolDef { name, description, parameters }` | `types.rs` → `ToolDef` |
| `Arc<dyn LlmProvider>` | `provider.rs` → `LlmProvider` trait |
| `StreamOptions::default()` | `provider.rs` → `StreamOptions` |
| `AssistantMessageEventStream` | `streaming.rs` → `AssistantMessageEventStream` |
| `LlmError::RateLimited(String)` | `error.rs` → `LlmError::RateLimited` |

---

## 13. 关键设计决策

| 决策 | 理由 |
|---|---|
| 细粒度事件流 (text_start/delta/end) | 与 pi.dev 对齐。UI 可增量渲染，无需等待完整响应。`content_index` 允许多 block 并行组装。 |
| `partial` 字段携带完整快照 | agent-core 无需自己维护消息状态。每个事件自描述，简化事件消费方逻辑。 |
| 错误编码到 stream 中，不抛出 | pi.dev 的 Venice 式设计。防止未处理异常传播，stream 消费方统一错误处理路径。 |
| `SecretString` 包装 API Key | 满足 AGENTS.md 安全约束：key 不出现在 Debug/Display/tracing/panic 中。secrecy crate 是 Rust 生态标准方案。 |
| 仅对 RateLimited/Overloaded/Timeout 重试 | 遵循 AGENTS.md 的指数退避约束。非幂等错误（AuthError、InvalidRequest）不重试。 |
| 不使用 heavy SDK（如 async-openai）于所有 provider | Anthropic/Google 使用裸 reqwest + SSE 解析，避免引入庞大的 SDK 依赖。OpenAI 可以使用 async-openai 自带的 stream 功能。 |
| 编译期 provider 注册 | 当前阶段仅内部 Extension，不需要动态加载。未来扩展 WASM/RPC 时，LlmProvider trait 已经是抽象边界。 |
