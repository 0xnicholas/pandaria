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
- 模型注册表（Model, ModelRegistry, 成本计算）
- 上下文溢出检测（is_context_overflow, 19 provider regex 模式）
- Tool call 参数校验（validate_tool_arguments, JSON Schema + 强制转换）
- Provider 兼容层（OpenAiCompat, AnthropicCompat, auto-detection）
- Prompt cache key 管理（CacheRetention, session-based cache）
- Provider hook（on_payload / on_response 回调）
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
    types.rs            # Message, Content, ToolCall, ToolDef, Usage, StopReason, Api, LlmContext
    error.rs            # LlmError (thiserror)
    streaming.rs        # AssistantMessageEvent, AssistantMessageEventStream
    provider.rs         # LlmProvider trait, StreamOptions, ReasoningLevel
    retry.rs            # with_retry() utility
    util.rs             # extract_tool_calls, build_tool_defs
    models.rs           # Model, ModelRegistry, TokenCost, calculate_cost, supports_xhigh
    models_data.rs      # compile-time model definitions (phf_map!)
    overflow.rs         # is_context_overflow, 19 provider-specific regex patterns
    validation.rs       # validate_tool_arguments, JSON Schema coercion + validation
    compat.rs           # OpenAiCompat, AnthropicCompat, detect_*, merge_*
    cache.rs            # CacheRetention, CacheConfig
    hooks.rs            # OnPayloadFn, OnResponseFn, ProviderResponse
    repair.rs           # repair_json, StreamingJsonParser, sanitize_unicode
    transform.rs        # transform_messages, 跨 provider 消息标准化
    providers/
      mod.rs            # Provider registry
      anthropic.rs      # Anthropic Messages API
      openai.rs         # OpenAI Chat Completions
      google.rs         # Google Gemini generateContent
      mistral.rs        # Mistral Chat Completions (P3)
      bedrock.rs        # AWS Bedrock ConverseStream (P3)
  tests/
    types_serde.rs      # JSON round-trip tests
    streaming_tests.rs  # EventStream behavior
    retry_tests.rs      # with_retry() retry policy
    security_tests.rs   # API key leak prevention
    overflow_tests.rs   # Context overflow detection per-provider patterns
    validation_tests.rs # Tool argument validation + coercion
    models_tests.rs     # ModelRegistry lookup + cost calculation
    compat_tests.rs     # OpenAiCompat / AnthropicCompat auto-detection
    repair_tests.rs     # JSON repair + streaming parser
    anthropic_tests.rs  # Mock HTTP integration tests
    openai_tests.rs     # Mock HTTP integration tests
    google_tests.rs     # Mock HTTP integration tests
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
eventsource-stream = "0.2"
tokio-util = { version = "0.7", features = ["io"] }
secrecy = { version = "0.10", features = ["serde"] }
regex = { workspace = true }
phf = { version = "0.11", features = ["macros"] }
jsonschema = "0.28"

[dev-dependencies]
wiremock = "0.6"
tokio-test = "0.4"
```

**新增 workspace dependency：**

```toml
# Cargo.toml (workspace)
regex = "1"
phf = { version = "0.11", features = ["macros"] }
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
        /// Provider-specific text signature (OpenAI Responses message metadata).
        /// Mirrors pi-mono's `TextContent.textSignature`.
        #[serde(skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        /// Opaque signature for multi-turn thinking continuity (Anthropic/Google).
        /// Mirrors pi-mono's `ThinkingContent.thinkingSignature`.
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
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
    /// Google-specific opaque thought signature for multi-turn reasoning continuity.
    /// Mirrors pi-mono's `ToolCall.thoughtSignature`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
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
///
/// The `role` field in serialized JSON is provided by the `#[serde(tag = "role")]`
/// on the `Message` enum (see below). The struct itself does not carry a `role` field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserMessage {
    pub content: Vec<Content>,
    #[serde(with = "system_time_serde")]
    pub timestamp: SystemTime,
}

/// An assistant message (LLM response).
///
/// Distinction between `provider` and `api.provider`:
/// - `provider`: the entity hosting the model (e.g., "openai", "google", "openrouter")
/// - `api.provider`: redundantly the same as this field, kept for backward compat
/// In pi-mono these are separate concepts (Provider vs Api).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantMessage {
    pub content: Vec<Content>,
    /// The entity hosting/routing the model (e.g., "anthropic", "openai", "openrouter").
    /// Distinct from the API protocol identifier in `api.provider`.
    pub provider: String,
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
/// Uses serde tag-discrimination — the `role` field is synthesized from
/// the enum variant during serialization/deserialization, not stored as a struct field.
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
    /// Total tokens consumed: input + output + cache_creation + cache_read.
    /// Mirrors pi-mono's `Usage.totalTokens`.
    #[serde(default)]
    pub total_tokens: u64,
}

impl Usage {
    /// Compute total_tokens from the component fields.
    pub fn compute_total(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens.unwrap_or(0)
            + self.cache_read_input_tokens.unwrap_or(0)
    }
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

> **Image 内容说明**：`Content::Image` 块仅出现在 user 和 tool result 消息中（由用户上传或工具产出），不通过 LLM 流式生成。因此 `AssistantMessageEvent` 不包含 `ImageStart/Delta/End` 变体。

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
    /// Wrapped in Option to enable explicit drop/cancellation in drain().
    rx: Option<tokio::sync::mpsc::Receiver<AssistantMessageEvent>>,
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

> **实现状态说明 (§4)**：当前代码中的 `AssistantMessageEvent` 和 `AssistantMessageEventStream` 为简化版 MVP：
> - `AssistantMessageEvent` 仅有 5 个扁平变体（`Start`, `TextDelta { text }`, `ToolCallDelta { tool_call }`, `Done { content, api, usage, stop_reason }`, `Error { message }`），不含 `content_index` 和 `partial` 字段
> - `AssistantMessageEventStream` 是 `Pin<Box<dyn futures::Stream<...>>>` 类型别名，非 mpsc 封装的 concrete struct
>
> 本 spec 定义的是目标设计。实现时需升级到带 `content_index` + `partial` 的细粒度事件和 mpsc 版本的 Stream struct，以支持多 block 并行组装和 `to_message()` 便捷消费。当前简化版已足够驱动单轮 agent loop。

---

## 5. LlmProvider trait

```rust
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Abstraction over LLM provider backends.
/// v0.1 implementations: Anthropic, OpenAI, Google.
///
/// Parameters (`context`, `options`) are taken by value — ownership is transferred
/// to the provider implementation, which can then move them into a spawned task
/// without cloning.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// The provider's canonical name (e.g., "anthropic", "openai", "google").
    fn provider_name(&self) -> &str;

    /// List of model IDs this provider knows about.
    /// The provider does NOT own the model registry; this is a convenience
    /// for listing models that this Provider implementation can handle.
    /// For metadata (context_window, cost, etc.), use `ModelRegistry`.
    fn models(&self) -> Vec<String>;

    /// Return an `Api` identifier by combining `provider_name()` with the given model.
    fn api_for(&self, model: &str) -> Api {
        Api {
            provider: self.provider_name().to_string(),
            model: model.to_string(),
        }
    }

    /// Stream a response from the LLM.
    ///
    /// Errors returned from this method indicate the request could not be initiated
    /// (connection failure, auth failure). Runtime errors during streaming are
    /// delivered as `AssistantMessageEvent::Error`.
    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: StreamOptions,
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
#[derive(Clone)]
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
    /// Per-level token budgets for reasoning (token-budget providers: Anthropic pre-4.6, Google Vertex).
    /// Overrides default budgets when set.
    pub thinking_budgets: Option<ThinkingBudgets>,
    /// Maximum retry attempts per request. Default: 3.
    pub max_retries: u32,
    /// Maximum acceptable retry delay before failing the request early. Default: 60s.
    pub max_retry_delay_ms: u64,
    /// Custom HTTP headers to append to provider requests (e.g., tracing headers).
    pub headers: Option<HashMap<String, String>>,
    /// Provider-specific metadata (e.g., Anthropic `user_id` for abuse tracking).
    pub metadata: Option<HashMap<String, String>>,
    /// Prompt cache retention policy. Default: Short.
    pub cache_retention: CacheRetention,
    /// Session-scoped cache key. When set, providers use this for prompt caching
    /// (prompt_cache_key in OpenAI, session_id in Codex, implicit in Anthropic cache_control).
    pub session_id: Option<String>,
    /// Hook: invoked after building provider-specific request params, before sending HTTP request.
    /// Receives the JSON payload (serde_json::Value) and the Model for inspection/mutation.
    /// Return true if the payload was modified.
    pub on_payload: Option<OnPayloadFn>,
    /// Hook: invoked after HTTP response arrives, before consuming the stream body.
    /// Receives status, headers, and Model for observability/audit purposes.
    pub on_response: Option<OnResponseFn>,
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
            thinking_budgets: None,
            max_retries: 3,
            max_retry_delay_ms: 60_000,
            headers: None,
            metadata: None,
            cache_retention: CacheRetention::default(),
            session_id: None,
            on_payload: None,
            on_response: None,
        }
    }
}

impl std::fmt::Debug for StreamOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamOptions")
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("top_p", &self.top_p)
            .field("reasoning", &self.reasoning)
            .field("thinking_budgets", &self.thinking_budgets)
            .field("max_retries", &self.max_retries)
            .field("max_retry_delay_ms", &self.max_retry_delay_ms)
            .field("headers", &self.headers)
            .field("metadata", &self.metadata)
            .field("cache_retention", &self.cache_retention)
            .field("session_id", &self.session_id)
            .field("on_payload", &self.on_payload.as_ref().map(|_| "OnPayloadFn"))
            .field("on_response", &self.on_response.as_ref().map(|_| "OnResponseFn"))
            .finish()
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

/// Per-level token budgets for providers that use fixed token counts
/// instead of effort levels (Anthropic pre-4.6, Google Vertex).
///
/// Defaults (when not set in StreamOptions):
///   Minimal: 1024, Low: 2048, Medium: 8192, High: 16384
///   XHigh: not applicable (token-budget providers clamp to High)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingBudgets {
    pub minimal: Option<u32>,
    pub low: Option<u32>,
    pub medium: Option<u32>,
    pub high: Option<u32>,
}

/// Adjust max_tokens to accommodate thinking budget for token-budget providers.
///
/// Calculate `max_tokens` sent to the API as: base_max_tokens + thinking_budget.
/// If the model's limit is too tight, squeeze the thinking budget to guarantee
/// at least 1024 output tokens. Mirrors pi-mono's `adjustMaxTokensForThinking`.
///
/// Returns `(total_max_tokens, thinking_budget_tokens)`.
pub fn adjust_max_tokens_for_thinking(
    base_max_tokens: u32,
    model_max_tokens: u32,
    reasoning_level: ReasoningLevel,
    custom_budgets: Option<&ThinkingBudgets>,
) -> (u32, u32);
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

impl LlmError {
    /// Whether this error is retryable (RateLimited, Overloaded, or Timeout).
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited(_) | Self::Overloaded(_) | Self::Timeout(_))
    }
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
/// If max_retry_delay_ms is set and the computed delay exceeds it,
/// the request fails immediately (avoids unreasonably long waits).
///
/// Maximum retry count configurable via `max_retries` (default: 3).
#[instrument(skip(operation), fields(retry_count))]
pub async fn with_retry<F, Fut>(
    operation: F,
    max_retries: u32,
    max_retry_delay_ms: Option<u64>,
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
            Err(e) if attempt < max_retries && e.is_retryable() => {
                let delay = Duration::from_millis(100 * 2u64.pow(attempt));
                // Cap delay to max_retry_delay_ms if set
                if let Some(max_delay) = max_retry_delay_ms {
                    if delay.as_millis() as u64 > max_delay {
                        tracing::warn!(
                            retry_count = attempt + 1,
                            delay_ms = delay.as_millis(),
                            max_retry_delay_ms = max_delay,
                            "retry delay exceeds max, failing request"
                        );
                        return Err(e);
                    }
                }
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
```

---

## 9. Provider 实现细节

### 9.1 AnthropicProvider

```
// —— 内部函数签名 ——
fn build_params(model: &str, context: &LlmContext, options: &StreamOptions) -> serde_json::Value;
// 将 LlmContext + StreamOptions 转换为 Anthropic Messages API JSON payload。
// 每个 provider 有各自的 build_params 实现。

// —— 实现细节 —
Base URL: https://api.anthropic.com/v1/messages
Header:   x-api-key, anthropic-version: 2023-06-01
Beta:     anthropic-beta: interleaved-thinking-2025-05-14, fine-grained-tool-streaming-2025-05-14

请求转换:
  system_prompt → system: [{ text, cache_control: { type: "ephemeral" } }]
  messages → messages: [{ role, content }]
    - User: content → [{ type: "text", text }, { type: "image", source: { data, media_type } }]
    - Assistant: content → [{ type: "text", text }, { type: "tool_use", ... }]
    - ToolResult → { role: "user", content: [{ type: "tool_result", tool_use_id, content }] }
  tools → tools: [{ name, description, input_schema }]
  cache_control: 最后一个 system text 块 + 最后一条 message 附加 cache_control

Cache Control Body 构造:
  根据 StreamOptions::cache_retention 决定 cache_control 参数:
    None  → 不发送 cache_control
    Short → cache_control: { type: "ephemeral" }
    Long  → cache_control: { type: "ephemeral", ttl: "1h" }

  cache_control 附加到以下位置:
    1. system prompt 的每个 text block: { type: "text", text: "...", cache_control: {...} }
    2. 最后一个 tool definition: { name, ..., cache_control: {...} }
    3. 最后一条 user message 的最后一个 content block: { type: "text", text: "...", cache_control: {...} }

Thinking / Reasoning 参数构造:
  if !options.reasoning → thinking: { type: "disabled" } （禁用思考）
  
  自适应思维 (Opus 4.6+, Sonnet 4.6+):
    reasoning_level → effort mapping:
      Minimal → "low", Low → "low", Medium → "medium", High → "high", XHigh → "xhigh"
    params: thinking: { type: "adaptive", display: "summarized" }
    params: output_config: { effort: "low"|"medium"|"high"|"xhigh"|"max" }

  预算思维 (pre-4.6 模型):
    通过 adjust_max_tokens_for_thinking() 计算:
      (max_tokens, thinking_budget) = adjust(base_max, model_max, level, custom_budgets)
    params: max_tokens: max_tokens
    params: thinking: { type: "enabled", budget_tokens: thinking_budget, display: "summarized" }

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

Fine-grained tool streaming:
  通过 anthropic-beta header 启用。tool_use 的 input 字段在 content_block_delta
  中逐 fragment 到达（input_json_delta → ToolCallDelta），而非一次性完整 JSON。
  content_block_stop 时累积的 partialJson 解析为 ToolCall → ToolCallEnd。

Interleaved thinking:
  通过 anthropic-beta header 启用。thinking block 可与 text/tool_use block 交错出现，
  而非仅出现在 assistant message 开头。content_index 按 block 出现顺序递增。

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
  cache_retention = None → 不发送 prompt_cache_key
  cache_retention = Short + api.openai.com → prompt_cache_key: session_id
  cache_retention = Long → prompt_cache_retention: "24h" + session_id/x-client-request-id headers

Thinking format (根据 compat.thinking_format):
  "openai"     → reasoning_effort: "minimal"|"low"|"medium"|"high"
  "openrouter" → reasoning: { effort: "low"|"medium"|"high" }
  "deepseek"   → thinking: { type: "enabled" } + reasoning_effort: "high"|"max"
                  (reasoning_effort_map 映射: minimal/low/medium/high → "high", xhigh → "max")
  "zai"        → enable_thinking: true|false
  "qwen"       → enable_thinking: true|false
  "qwen-chat-template" → chat_template_kwargs: { enable_thinking, preserve_thinking: true }
  XHigh 仅在 supports_xhigh(model_id) 通过时允许，否则 clamp 到 High

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

### 9.4 MistralProvider

```
Base URL: https://api.mistral.ai/v1/chat/completions
Header:   Authorization: Bearer <key>

请求转换:
  system_prompt → messages[0]: { role: "system", content: text }
  messages → messages: [{ role, content }]
    - User: content → [{ type: "text", text }]
    - Assistant: content → [{ type: "text", text }, { tool_calls: [...] }]
    - ToolResult → { role: "tool", tool_call_id, content }
  tools → tools: [{ type: "function", function: { name, description, parameters } }]
  reasoning → promptMode: "reasoning" + reasoningEffort

流式解析 (SSE，与 OpenAI Completions 同构):
  choice.delta.content       → TextDelta（第一个 delta 前自动发 TextStart）
  choice.delta.tool_calls[i] → id + function.name 出现 → ToolCallStart
                             → function.arguments      → ToolCallDelta
  choice.finish_reason       → Done

Tool call ID 标准化:
  Mistral 对 tool call ID 有严格长度限制（≤36 chars），使用 shortHash() 截断。

Stop reason 映射:
  stop        → StopReason::Stop
  length      → StopReason::Length
  tool_calls  → StopReason::ToolUse
  error       → StopReason::Error
```

### 9.5 AwsBedrockProvider

```
Base URL: 由 AWS SigV4 签名确定（ConverseStream API）
Auth:     SigV4（通过 aws-sdk-bedrockruntime）或 Bearer token（AWS_BEARER_TOKEN_BEDROCK env var）

请求转换:
  system_prompt → system: [{ text }]
  messages → messages: [{ role, content }]
    - User → { role: "user", content: [{ text }, { image: { format, source: { bytes } } }] }
    - Assistant → { role: "assistant", content: [{ text }, { toolUse: { toolUseId, name, input } }] }
    - ToolResult → { role: "user", content: [{ toolResult: { toolUseId, content } }] }
  tools → toolConfig: { tools: [{ toolSpec: { name, description, inputSchema } }] }
  reasoning → reasoningConfig: { reasoningType: "enabled", budgetTokens }

流式解析 (ConverseStream 事件):
  与 Anthropic SSE 类似:
    messageStart        → Start
    contentBlockStart   → TextStart/ToolCallStart
    contentBlockDelta   → TextDelta/ToolCallDelta
    contentBlockStop    → TextEnd/ToolCallEnd
    messageStop         → Done

Cache 支持:
  通过 CachePoint + CacheTTL 标记 system prompt 和最后一条 assistant message。
  仅部分模型（如 Claude 3.5+）支持。

思考/推理:
  自适应推理: performanceConfig: { adaptiveThinking: { display: "summarized" } }
  预算推理: reasoningConfig: { reasoningType: "enabled", budgetTokens: N, display: "summarized" }

Stop reason 映射:
  end_turn   → StopReason::Stop
  max_tokens → StopReason::Length
  tool_use   → StopReason::ToolUse
  refusal    → StopReason::Error
```

**未来扩展：** Azure, Vertex, Gemini CLI, Codex Responses 等 provider 可在后续版本加入，
复用 llm-client 的统一事件协议，只需实现 `LlmProvider` trait 和消息请求/响应转换。

---

## 10. 工具函数

```rust
/// Extract tool calls from a Vec<Content>.
/// Returns all Content::ToolCall entries.
pub fn extract_tool_calls(content: &[Content]) -> Vec<ToolCall>;

/// Build a Vec<ToolDef> from a tool definition slice.
pub fn build_tool_defs(tools: &[ToolDef]) -> Vec<ToolDef>;
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
| `UserMessage { content, timestamp }` | `types.rs` → `UserMessage` |
| `LlmContext { system_prompt, messages, tools }` | `types.rs` → `LlmContext` |
| `StopReason::ToolUse` | `types.rs` → `StopReason::ToolUse` |
| `ToolDef { name, description, parameters }` | `types.rs` → `ToolDef` |
| `Arc<dyn LlmProvider>` | `provider.rs` → `LlmProvider` trait |
| `StreamOptions::default()` | `provider.rs` → `StreamOptions` |
| `AssistantMessageEventStream` | `streaming.rs` → `AssistantMessageEventStream` |
| `LlmError::RateLimited(String)` | `error.rs` → `LlmError::RateLimited` |
| `Model { id, context_window, max_tokens, .. }` | `models.rs` → `Model` |
| `get_model(provider, model_id)` | `models.rs` → `get_model()` |
| `calculate_cost(model, usage)` | `models.rs` → `calculate_cost()` |
| `is_context_overflow(..)` | `overflow.rs` → `is_context_overflow()` |
| `validate_tool_call(tools, tool_call)` | `validation.rs` → `validate_tool_call()` |
| `CacheRetention::Long` | `cache.rs` → `CacheRetention` |
| `OnPayloadFn` / `OnResponseFn` | `hooks.rs` → callback types |

### 12.1 澄清说明

- **事件字段名差异**: agent-core spec §2.3 的伪代码使用了简化的事件字段名（如 `TextDelta(text)` → `text`, `Done { content, api, usage, stop_reason }`）。实现时以本 spec §4.1 的 `AssistantMessageEvent` 定义为准——每个事件携带 `content_index` 和 `partial` 字段。伪代码是为了可读性有意省略这些字段。
- **重试边界**: `with_retry()`（§8）包装 `provider.stream()` 调用本身——处理连接级重试（RateLimited/Overloaded/Timeout）。agent-core spec §2.3 的 `call_llm_with_retry()` 覆盖更广：从 `provider.stream()` 到流消费完成。两者职责不同，最终实现时 agent-core 可能直接使用 `with_retry()` 而后独立消费 stream，或在 stream 消费层实现自己的重试逻辑。实现阶段统一。
- **`extract_tool_calls`**: llm-client 提供此工具函数（§10），agent-core 可选择直接使用或自行提取（如 §2.2 伪代码中的 `extract_tool_calls(&assistant_msg.content)`）。实现阶段统一使用 llm-client 的导出函数。

---

## 13. 关键设计决策

| 决策 | 理由 |
|---|---|
| 细粒度事件流 (text_start/delta/end) | 与 pi.dev 对齐。UI 可增量渲染，无需等待完整响应。`content_index` 允许多 block 并行组装。 |
| `partial` 字段携带完整快照 | agent-core 无需自己维护消息状态。每个事件自描述，简化事件消费方逻辑。 |
| 错误编码到 stream 中，不抛出 | pi.dev 的 Venice 式设计。防止未处理异常传播，stream 消费方统一错误处理路径。 |
| `SecretString` 包装 API Key | 满足 AGENTS.md 安全约束：key 不出现在 Debug/Display/tracing/panic 中。secrecy crate 是 Rust 生态标准方案。 |
| 仅对 RateLimited/Overloaded/Timeout 重试 | 遵循 AGENTS.md 的指数退避约束。非幂等错误（AuthError、InvalidRequest）不重试。 |
| 不使用 heavy SDK，统一用 reqwest + SSE | 三个 provider 均通过 reqwest + SSE/JSON 流实现。避免引入 Anthropic SDK、async-openai 等庞大依赖。保持 crate 轻量。 |
| 编译期 provider 注册 | 当前阶段仅内部 Extension，不需要动态加载。未来扩展 WASM/RPC 时，LlmProvider trait 已经是抽象边界。 |
| 编译期 model registry（LazyLock） | 模型定义是静态数据，`LazyLock<HashMap>` 提供惰性初始化，首次访问时 O(1) 或 O(n) 构建。避免 `phf_map!` 的 const 构造限制（String/Vec 不可在 const context 中使用）。 |
| `regex` 全局静态编译 | 上下文溢出检测需要 19 个 regex 模式。使用 `LazyLock<Regex>` 编译一次，所有检测复用。 |
| `jsonschema` + best-effort 强制转换 | 参考 pi-mono 的 coercion-first 策略：先尝试类型强制转换（"42"→42），再校验，减少 LLM 类型错误导致的 tool call 失败。 |
| `OnPayloadFn` 用 `serde_json::Value` 传递 | 各 provider 的请求参数是异构类型（Anthropic/OpenAI/Google 各有其结构），`serde_json::Value` 提供统一接口，与 pi-mono 的 `unknown` 一致。 |
| Compant 自动检测 + 显式覆盖合并 | 避免每个模型手动配置 18 个 compat 字段。基于 `base_url` 做 provider 分类，model.compat 仅需覆盖差异字段。 |

### 13.1 实现状态（Spec vs Code Gap）

| 组件 | 当前代码状态 | Spec 目标 | 评注 |
|---|---|---|---|
| `AssistantMessageEvent` | 12 变体细粒度事件，含 `content_index`/`partial` | 12 变体 | 已对齐 |
| `AssistantMessageEventStream` | mpsc-based concrete struct with `to_message()` | mpsc struct + `next()`/`to_message()`/`drain()` | 已对齐 |
| `StreamOptions` | 15 字段（含 cache_retention, session_id, hooks） | 15 字段 | 已对齐 |
| `LlmError` | 10 变体（Timeout 带 Duration） | 10 变体 | 已对齐 |
| 核心类型（Content/Message/Usage） | 所有 signature 字段已补齐 | 完整 | 已对齐 |
| `ModelRegistry` | `LazyLock<HashMap>` 实现，18 模型 | 18 模型，O(1) 查找 | 已对齐（存储方式差异：LazyLock vs phf） |
| `is_context_overflow` | 19 regex + 3 exclusion + 静默溢出 | 19 regex + 3 exclusion | 已对齐 |
| `validate_tool_arguments` | jsonschema + 7 种 coercion + union schema | jsonschema + coercion | 已对齐 |
| Compat 层 | 20 字段 OpenAiCompat + detect/merge | 20 字段 | 已对齐 |
| `CacheRetention` | None/Short/Long + env var 解析 | 同 | 已对齐 |
| Hooks | OnPayloadFn/OnResponseFn + 3 个 provider 集成 | 同 | 已对齐 |
| Anthropic Provider | 完整 SSE 解析 + cache_control body + thinking params + beta headers | §9.1 完整 spec | 已对齐 |
| OpenAI Provider | SSE 解析 + reasoning delta + prompt_cache_key | §9.2 | 已对齐（thinking_format multi-provider 待补） |
| Google Provider | JSON 流解析 + x-goog-api-key header | §9.3 | 已对齐 |
| Message Transformation | `transform_messages()` 含 ID 截断/图片降级/thinking 移除 | §25 | `pad_orphan_tool_results` 和 `short_hash` stubs 待补 |
| Mistral/Bedrock Provider | 未实现 | §9.4-9.5 | P3 优先级 |
| OAuth | 未实现 | §26 | P3 优先级 |
| `tests/` integration tests | 内联 `#[cfg(test)]` 单元测试（87 tests） | wiremock HTTP mock tests | 未创建 |

---

## 14. Model Registry (`models.rs`, `models_data.rs`)

### 14.1 类型定义

```rust
use std::collections::HashMap;

/// 输入模态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Modality { Text, Image }

/// 每百万 token 成本（美元）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TokenCost {
    /// $ per 1M input tokens
    pub input: f64,
    /// $ per 1M output tokens
    pub output: f64,
    /// $ per 1M cache-read tokens
    pub cache_read: f64,
    /// $ per 1M cache-write tokens
    pub cache_write: f64,
}

/// 静态模型元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    /// API protocol: "anthropic-messages" | "openai-completions" | "google-generative-ai"
    pub api: String,
    /// Provider: "anthropic" | "openai" | "google" | ...
    pub provider: String,
    pub base_url: String,
    pub reasoning: bool,
    pub input_modalities: Vec<Modality>,
    pub cost: TokenCost,
    pub context_window: u32,
    pub max_tokens: u32,
    pub headers: Option<HashMap<String, String>>,
    pub compat: ModelCompat,
}

/// Per-API 兼容性标记，按 api 字段 tag-discriminated
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "api")]
pub enum ModelCompat {
    #[serde(rename = "openai-completions")]
    OpenAI(OpenAiCompat),
    #[serde(rename = "anthropic-messages")]
    Anthropic(AnthropicCompat),
    #[serde(other)]
    None,
}
```

### 14.2 注册表与查找

```rust
/// 内置模型注册表。
/// 内部使用 `LazyLock<HashMap<String, Model>>` 实现惰性初始化。
/// 模型数据定义在 `models_data.rs`，通过 `fn build_models()` 在首次访问时构建。
pub struct ModelRegistry;

impl ModelRegistry {
    /// 返回内置注册表（全局静态单例）
    pub fn builtin() -> &'static Self;

    /// 通过 provider + model_id 查找（返回 owned clone）。
    /// LazyLock<HashMap> 中的数据的生命周期受限于 HashMap，
    /// 无法返回 &'static 引用，故返回 owned Model。
    pub fn get(&self, provider: &str, model_id: &str) -> Option<Model>;

    /// 返回某 provider 下的所有模型
    pub fn models_for_provider(&self, provider: &str) -> Vec<Model>;

    /// 返回所有已知 provider 名称
    pub fn providers(&self) -> Vec<String>;
}

/// 便捷函数，直接通过内置注册表查找
pub fn get_model(provider: &str, model_id: &str) -> Option<Model>;
pub fn models_for_provider(provider: &str) -> Vec<Model>;
pub fn providers() -> Vec<String>;
```

### 14.3 成本计算与工具函数

```rust
/// 根据 Usage 和 Model 的单价计算实际美元成本。
/// 公式: cost = (token_count / 1_000_000) * price_per_million
pub fn calculate_cost(model: &Model, usage: &Usage) -> TokenCost;

/// 比较两个模型是否相同（同 provider + 同 id）
pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool;

/// 检查模型是否支持 xhigh 推理（基于 model.id 子串匹配）
///
/// 规则:
///   - gpt-5.2 / gpt-5.3 / gpt-5.4 / gpt-5.5 / deepseek-v4-pro → true
///   - opus-4-6 / opus-4.6 / opus-4-7 / opus-4.7 → true
///   - 其他 → false
pub fn supports_xhigh(model_id: &str) -> bool;
```

### 14.4 内置模型覆盖范围（`models_data.rs`）

`phf_map!` 编译期构建，覆盖 v0.1 阶段 18 个核心模型：

| Provider | 模型数量 | 示例 model_id |
|---|---|---|
| anthropic | 6 | `claude-sonnet-4-20250514`, `claude-opus-4-7`, `claude-haiku-4-7` |
| openai | 8 | `gpt-5.2`, `gpt-5.3`, `gpt-5.4`, `gpt-5.5`, `gpt-5.1-codex` |
| google | 4 | `gemini-2.5-pro`, `gemini-2.5-flash`, `gemini-3.0-flash` |

未来扩展（OpenRouter, DeepSeek, Groq 等）时只需追加 `models_data.rs` 条目，无需改动 `ModelRegistry` 结构。

---

## 15. Context Overflow Detection (`src/overflow.rs`)

### 15.1 API

```rust
use regex::Regex;
use std::sync::LazyLock;

/// 检测 LLM 错误是否由上下文溢出导致。
///
/// 两种检测路径：
///   1. **Error-based**: stop_reason == Error 且 error_message 匹配溢出 regex，
///      但先排除 NON_OVERFLOW 模式（rate limit / throttling）
///   2. **Silent overflow**: stop_reason == Stop 但 input_tokens + cache_read_tokens > context_window
///      (处理 z.ai / Ollama 等静默截断场景)
pub fn is_context_overflow(
    error_message: Option<&str>,
    stop_reason: &StopReason,
    context_window: Option<u32>,
    input_tokens: u64,
    cache_read_tokens: u64,
) -> bool;
```

### 15.2 溢出检测模式矩阵（19 个 regex）

所有模式编译为 `static` 的 `LazyLock<Vec<Regex>>`，首次调用时编译，后续复用。

| # | Provider | Pattern | 典型错误消息 |
|---|---|---|---|
| 1 | Anthropic | `prompt is too long` | `"prompt is too long: 213462 tokens > 200000"` |
| 2 | Anthropic | `request_too_large` | `'413 {"error":{"type":"request_too_large"}}'` |
| 3 | Bedrock | `input is too long for requested model` | Bedrock overflow |
| 4 | OpenAI | `exceeds the context window` | Completions & Responses API |
| 5 | Google | `input token count.*exceeds the maximum` | `"exceeds the maximum number of tokens allowed"` |
| 6 | xAI | `maximum prompt length is \d+` | `"This model's maximum prompt length is 131072..."` |
| 7 | Groq | `reduce the length of the messages` | `"Please reduce the length of the messages"` |
| 8 | OpenRouter | `maximum context length is \d+ tokens` | OpenRouter 标准错误 |
| 9 | GitHub Copilot | `exceeds the limit of \d+` | Copilot overflow |
| 10 | llama.cpp | `exceeds the available context size` | `"try increasing it"` |
| 11 | LM Studio | `greater than the context length` | LM Studio 本地推理 |
| 12 | MiniMax | `context window exceeds limit` | `"invalid params, context window exceeds limit"` |
| 13 | Kimi | `exceeded model token limit` | Kimi For Coding |
| 14 | Mistral | `too large for model with \d+` | Mistral 溢出 |
| 15 | z.ai | `model_context_window_exceeded` | 非标准 finish_reason |
| 16 | Ollama | `prompt too long.*context length` | Ollama 本地推理 |
| 17 | Cerebras | `4(00\|13)\s*(?:status code)?\s*\(no body\)` | Cerebras 无 body 错误 |
| 18 | Generic | `context[_ ]length[_ ]exceeded` | 通用回退 |
| 19 | Generic | `too many tokens\|token limit exceeded` | 通用回退 |

### 15.3 排除模式（3 个 regex，防止误判）

| Pattern | 排除场景 |
|---|---|
| `^(Throttling error\|Service unavailable):` | Bedrock throttling |
| `rate limit` | 通用速率限制 |
| `too many requests` | HTTP 429 |

### 15.4 使用方式（agent-core 侧）

```rust
// agent loop stream 消费中收到 error assistant message:
if is_context_overflow(
    msg.error_message.as_deref(),
    &msg.stop_reason,
    Some(model.context_window),
    msg.usage.input_tokens,
    msg.usage.cache_read_input_tokens.unwrap_or(0),
) {
    // 触发 compaction
    trigger_compaction().await;
} else if let Some(msg) = &assistant_msg.error_message {
    // agent-core 自行判断是否可重试（如检查 overloaded/rate limit 等模式）
    if is_retryable_error_message(msg) {
        retry().await;
    }
}
```

---

## 16. Tool Call Argument Validation (`src/validation.rs`)

### 16.1 类型定义

```rust
use thiserror::Error;

/// 校验错误
#[derive(Debug, Error)]
pub enum ValidationError {
    /// 工具定义中找不到该 tool
    #[error("tool '{0}' not found")]
    ToolNotFound(String),

    /// JSON Schema 校验失败
    #[error("validation failed for tool '{tool}':\n{errors}\n\nReceived arguments:\n{received}")]
    SchemaViolation {
        tool: String,
        errors: Vec<ValidationMessage>,
        received: serde_json::Value,
    },
}

#[derive(Debug, Clone)]
pub struct ValidationMessage {
    /// JSON 路径，如 "count" 或 "files[0].path"
    pub path: String,
    pub message: String,
}
```

### 16.2 API

```rust
/// 验证 tool call arguments，先做 best-effort 强制转换，再校验 JSON Schema。
///
/// # 强制转换（与 pi-mono 对齐）
///
/// LLM 可能返回弱类型参数（例如 integer 字段给了 "42" 字符串）。
/// 本函数在 `serde_json::Value` 层面对参数做类型修复后再校验 JSON Schema：
///
/// | Schema 声明类型 | 实际 Value 类型 | 转换行为 | 示例 |
/// |---|---|---|---|
/// | integer/number | String | 解析为数字 | `Value::String("42")` → `Value::Number(42)` |
/// | boolean | String | 解析为布尔 | `Value::String("true")` → `Value::Bool(true)` |
/// | string | Number | 转为字符串 | `Value::Number(42.0)` → `Value::String("42")` |
///
/// 注意：强制转换只修改可安全转换的字段，不可转换的保留原值交给 schema 校验报错。
pub fn validate_tool_arguments(
    tool: &ToolDef,
    tool_call: &ToolCall,
) -> Result<serde_json::Value, ValidationError>;

/// 按名称查找 tool 并调用 validate_tool_arguments
pub fn validate_tool_call(
    tools: &[ToolDef],
    tool_call: &ToolCall,
) -> Result<serde_json::Value, ValidationError>;
```

### 16.3 Schema 编译缓存

`jsonschema::JSONSchema` 编译开销较大，内部使用 `std::sync::LazyLock<Mutex<HashMap<String, JSONSchema>>>` 按 tool name 缓存已编译的 schema。

### 16.4 Coercion 实现细节

```rust
/// 遍历 JSON Schema 的定义树（properties, items, allOf, anyOf, oneOf, additionalProperties），
/// 在叶子节点对参数值做类型强制转换。
///
/// 使用 jsonschema crate 的 schema 遍历能力。
fn coerce_arguments(args: &mut serde_json::Value, schema: &serde_json::Value);
```

**Coercion 规则矩阵（完整）：**

| Schema 类型 | 实际 Value 类型 | 转换行为 | 示例 |
|---|---|---|---|
| integer | String | parse 为 i64 | `Value::String("42")` → `Value::Number(42)` |
| number | String | parse 为 f64 | `Value::String("3.14")` → `Value::Number(3.14)` |
| boolean | String | parse 为 bool | `Value::String("true")` → `Value::Bool(true)` |
| string | Number | 格式化为字符串 | `Value::Number(42.0)` → `Value::String("42")` |
| number | Boolean | 转换为 1/0 | `Value::Bool(true)` → `Value::Number(1)` |
| boolean | Number | 0→false, 非0→true | `Value::Number(1)` → `Value::Bool(true)` |
| any | Null | 默认值填充 | `Value::Null` → type-dependent default |

**Union 复合 Schema 处理（allOf / anyOf / oneOf）：**

```
for each union member in [allOf, anyOf, oneOf]:
    cloned = structuredClone(args)
    coerce_arguments(&mut cloned, &member)
    if member.is_valid(&cloned):
        *args = cloned
        break
```

对每个 union member 克隆参数值，独立尝试 coercion，首个通过校验的结果替换原始 args。所有 member 均失败则保留原始 args，由最终 schema 校验报告错误。

**additionalProperties 递归：** 对 `additionalProperties` schema 定义，将所有未在 `properties` 中显式声明的字段递归应用 coercion。

### 16.5 校验失败消息格式（示例）

```
validation failed for tool 'read_file':
  - path: must be a string
  - max_lines: expected number, got string

Received arguments:
{
  "path": 123,
  "max_lines": "abc"
}
```

### 16.6 使用方式（agent-core 侧）

```rust
// 与 pi-mono agent-loop 对齐：
// 在 execute_tool_calls 中，每个 tool_call 执行前校验：
match validate_tool_call(&context.tools, &tool_call) {
    Ok(validated_args) => {
        // 传递给 tool.execute(tool_call.id, validated_args)
    }
    Err(ValidationError::SchemaViolation { .. }) => {
        // 返回 is_error = true 的 ToolResultMessage，
        // 内容包含格式化的校验失败信息，LLM 看到后可自修正
    }
    Err(ValidationError::ToolNotFound(_)) => {
        // LLM hallucinated a tool, 返回错误
    }
}
```

---

## 17. Provider Compatibility Layer (`src/compat.rs`)

### 17.1 OpenAI Chat Completions 兼容性

```rust
use std::collections::HashMap;

/// OpenAI Chat Completions API 兼容性覆盖。
/// 所有字段为 Option：None 表示使用 auto-detected 默认值，Some(v) 表示显式覆盖。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenAiCompat {
    pub supports_store: Option<bool>,
    pub supports_developer_role: Option<bool>,
    pub supports_reasoning_effort: Option<bool>,
    /// ThinkingLevel → provider-specific effort string
    /// Keys: "minimal", "low", "medium", "high", "xhigh" (all lowercase, matching ReasoningLevel serialization)
    /// 例: DeepSeek 映射 "minimal"→"high", "xhigh"→"max"
    pub reasoning_effort_map: Option<HashMap<String, String>>,
    pub supports_usage_in_streaming: Option<bool>,
    pub max_tokens_field: Option<MaxTokensField>,
    pub requires_tool_result_name: Option<bool>,
    pub requires_assistant_after_tool_result: Option<bool>,
    pub requires_thinking_as_text: Option<bool>,
    pub requires_reasoning_content_on_assistant_messages: Option<bool>,
    pub thinking_format: Option<ThinkingFormat>,
    pub supports_strict_mode: Option<bool>,
    pub cache_control_format: Option<CacheControlFormat>,
    pub send_session_affinity_headers: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
    /// Whether z.ai supports top-level `tool_stream: true` for incremental tool call deltas.
    pub zai_tool_stream: Option<bool>,
    /// OpenRouter provider routing preferences.
    pub open_router_routing: Option<OpenRouterRouting>,
    /// Vercel AI Gateway routing preferences.
    pub vercel_gateway_routing: Option<VercelGatewayRouting>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MaxTokensField {
    #[serde(rename = "max_completion_tokens")]
    MaxCompletionTokens,
    #[serde(rename = "max_tokens")]
    MaxTokens,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThinkingFormat {
    OpenAI,           // reasoning_effort 字段
    OpenRouter,       // reasoning: { effort } 嵌套字段
    DeepSeek,         // thinking: { type: "enabled"/"disabled" } + reasoning_effort
    Zai,              // enable_thinking: bool (顶层)
    Qwen,             // enable_thinking: bool (顶层)
    QwenChatTemplate, // chat_template_kwargs: { enable_thinking, preserve_thinking }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CacheControlFormat { Anthropic }

/// OpenRouter provider routing configuration.
/// Mirrors pi-mono's `OpenRouterRouting`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenRouterRouting {
    pub allow_fallbacks: Option<bool>,
    pub order: Option<Vec<String>>,
    pub only: Option<Vec<String>>,
    pub sort: Option<String>,
    pub max_price: Option<f64>,
    pub quantizations: Option<Vec<String>>,
}

/// Vercel AI Gateway routing configuration.
/// Mirrors pi-mono's `VercelGatewayRouting`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VercelGatewayRouting {
    pub only: Option<Vec<String>>,
    pub order: Option<Vec<String>>,
}
```

### 17.2 Anthropic Messages 兼容性

```rust
/// Anthropic Messages API 兼容性覆盖
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnthropicCompat {
    pub supports_eager_tool_input_streaming: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
}
```

### 17.3 自动检测与合并

```rust
/// 根据 provider 名称 + base_url 自动检测 OpenAI compat 字段。
///
/// 检测逻辑：
///   - cerebras / xai / deepseek / zai / opencode / cloudflare: is_non_standard = true
///     → supports_store = false, supports_developer_role = false
///   - deepseek: thinking_format = DeepSeek, requires_reasoning_content_on_assistant_messages = true
///   - grok (xai): supports_reasoning_effort = false
///   - chutes.ai: max_tokens_field = MaxTokens (而非 max_completion_tokens)
///   - openrouter + model_id starts_with("anthropic/"): cache_control_format = Anthropic
///   - zai: thinking_format = Zai, supports_reasoning_effort = false
///   - openrouter: thinking_format = OpenRouter
///   - qwen: thinking_format = Qwen or QwenChatTemplate
pub fn detect_openai_compat(provider: &str, base_url: &str, model_id: &str) -> OpenAiCompat;

/// Anthropic compat 自动检测。当前所有已知 Anthropic-family provider 默认全部支持。
pub fn detect_anthropic_compat(_provider: &str, _base_url: &str) -> AnthropicCompat;

/// 合并显式 compat 覆盖到 auto-detected 基线。
/// 合并规则：explicit 中 Option<Some(v)> 的字段覆盖 baseline，Option<None> 的字段保留 baseline。
pub fn merge_openai_compat(baseline: &OpenAiCompat, explicit: &OpenAiCompat) -> OpenAiCompat;
pub fn merge_anthropic_compat(baseline: &AnthropicCompat, explicit: &AnthropicCompat) -> AnthropicCompat;
```

### 17.4 Provider 集成方式

各 provider 实现（`openai.rs`, `anthropic.rs`）在 `stream()` 方法中：

```
1. 从 model (str id) 查找 Model 元数据：get_model(provider_name, model)
   注意: stream() 接收 model: &str，非 Model 引用。
   Provider 内部通过 ModelRegistry::builtin().get() 自行查找。
2. detected = detect_*_compat(model.provider, model.base_url, model.id)
3. resolved = merge_*_compat(detected, model.compat)
4. 根据 resolved 构建 provider-specific 请求参数：
   - 如果 thinking_format == DeepSeek: 发送 thinking: { type: "enabled" } 而非 reasoning_effort
   - 如果 max_tokens_field == MaxTokens: 用 max_tokens 而非 max_completion_tokens
   - 如果 cache_control_format == Anthropic: 对 system prompt / last message 附加 cache_control
```

---

## 18. Prompt Cache Key Management (`src/cache.rs`)

### 18.1 类型定义

```rust
/// 缓存保留策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheRetention {
    /// 不使用 prompt cache
    None,
    /// 短周期缓存（provider 默认，通常 5 分钟）
    Short,
    /// 长周期缓存（Anthropic: ttl=1h, OpenAI: prompt_cache_retention=24h）
    Long,
}

impl CacheRetention {
    /// 解析优先级:
    ///   1. explicit 非 None → 直接使用
    ///   2. PI_CACHE_RETENTION == "long" → Long
    ///   3. 默认 → Short
    pub fn resolve(explicit: Option<Self>) -> Self;
}

impl Default for CacheRetention {
    fn default() -> Self { Self::Short }
}
```

### 18.2 各 Provider 缓存行为

| Provider | `cache_retention = None` | Short | Long |
|---|---|---|---|
| **Anthropic** | 不设置 `cache_control` | `cache_control: { type: "ephemeral" }` | `cache_control: { type: "ephemeral", ttl: "1h" }` |
| **OpenAI Completions** | 不发送 `prompt_cache_key` | 仅 `api.openai.com` host 发送 `prompt_cache_key = sessionId` | `prompt_cache_retention: "24h"` + `session_id`/`x-client-request-id` headers |
| **OpenAI Responses** | 不发送 `prompt_cache_key` | `prompt_cache_key = sessionId` | `prompt_cache_retention: "24h"` + `session_id` header |
| **Google** | 无额外设置 | 默认 cache（SDK 内置） | 默认 cache（SDK 内置） |

### 18.3 CacheControl / Anthropic cache_control 标记位置

```rust
/// Cache control parameters sent to Anthropic Messages API.
#[derive(Debug, Clone, Serialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,  // "ephemeral"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>, // e.g. "1h" for long retention, absent for short
}
```

Provider 实现内部（anthropic.rs）:
fn apply_cache_control(
    system_blocks: &mut Vec<Value>,
    tools: &mut Vec<Value>,
    last_message: &mut Value,
    cache_control: &CacheControl,
) {
    // 1. 所有 system prompt text block 附加 cache_control
    for block in system_blocks.iter_mut() {
        block["cache_control"] = json!(cache_control);
    }
    // 2. 最后一个 tool definition 附加 cache_control
    if let Some(last_tool) = tools.last_mut() {
        last_tool["cache_control"] = json!(cache_control);
    }
    // 3. 最后一条 user message 的最后一个 content block 附加 cache_control
    if let Some(content) = last_message["content"].as_array_mut() {
        if let Some(last_block) = content.last_mut() {
            last_block["cache_control"] = json!(cache_control);
        }
    }
}
```

### 18.4 session_id 用途

`session_id` 在 `StreamOptions` 中可选设置，用于：

- **OpenAI**: 作为 `prompt_cache_key`，同一 session 内的请求可命中之前缓存的 prompt prefix
- **Anthropic**: 不使用 session_id（使用 `cache_control` breakpoint 标记替代）

---

## 19. Provider Hooks (`src/hooks.rs`)

### 19.1 类型定义

```rust
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// HTTP 响应元数据（在消费 body 前传递）
#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
}

/// 请求负载钩子：在发送 HTTP 请求前调用。
/// 参数:
///   - `payload`: provider-specific JSON body（`serde_json::Value`），可修改
///   - `model`: 当前使用的模型元数据
/// 返回: `true` 表示修改了 payload
pub type OnPayloadFn = Arc<
    dyn Fn(&mut serde_json::Value, &Model) -> Pin<Box<dyn Future<Output = bool> + Send>>
        + Send + Sync,
>;

/// 响应钩子：在收到 HTTP 响应后、消费 body 前调用。
/// 参数:
///   - `response`: HTTP 状态码和响应头
///   - `model`: 当前使用的模型元数据
/// 用途: 可观测性/审计/限流响应头解析，不可修改响应
pub type OnResponseFn = Arc<
    dyn Fn(&ProviderResponse, &Model) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send + Sync,
>;
```

### 19.2 Provider 调用点（伪代码）

```
// 每个 provider 实现在 stream() 的 tokio::spawn 内部遵循以下序列:

1. let mut payload = build_params(model, context, options);  // → serde_json::Value
2. if let Some(hook) = &options.on_payload {
       hook(&mut payload, &model).await;
   }
3. let response = client.post(url)
       .json(&payload)
       .headers(headers)
       .send()
       .await?;
4. if let Some(hook) = &options.on_response {
       hook(&ProviderResponse {
           status: response.status().as_u16(),
           headers: response.headers().iter()
               .map(|(k,v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
               .collect(),
       }, &model).await;
   }
5. process_sse_stream(response).await;  // 正式开始流式消费
```

### 19.3 生命周期序列

```
buildParams → onPayload(mut payload, model) → HTTP send → receive response
  → onResponse({status, headers}, model) → stream SSE/JSON → push events → Done/Error
```

### 19.4 典型使用场景（agent-core / Extension 侧）

```rust
let options = StreamOptions {
    on_payload: Some(Arc::new(|payload, model| {
        Box::pin(async move {
            // Extension: 在发送前注入额外的 system message、修改 temperature
            if let Some(extras) = get_extra_payload_fields() {
                if let Some(obj) = payload.as_object_mut() {
                    obj.extend(extras);
                }
            }
            true  // 表示已修改
        })
    })),
    on_response: Some(Arc::new(|response, _model| {
        Box::pin(async move {
            tracing::info!(
                status = response.status,
                headers = ?response.headers,
                "LLM response received"
            );
        })
    })),
    ..StreamOptions::default()
};
```

---

## 20. 新增测试计划

### 20.1 模型注册表测试 (`tests/models_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_get_model_found` | get_model("anthropic", "claude-sonnet-4-20250514") 返回 Some |
| `test_get_model_not_found` | 不存在模型返回 None |
| `test_models_for_provider` | models_for_provider("openai") 返回正确数量的模型 |
| `test_providers_list` | providers() 包含全部已知 provider |
| `test_calculate_cost` | calculate_cost 正确计算（input/output/cache 分量） |
| `test_supports_xhigh` | gpt-5.2 → true, gpt-4.1 → false, opus-4-7 → true |
| `test_models_are_equal` | 同 provider+id → true，不同 → false |

### 20.2 上下文溢出测试 (`tests/overflow_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_anthropic_prompt_too_long` | Anthropic overflow 消息 → true |
| `test_openai_exceeds_context_window` | OpenAI overflow 消息 → true |
| `test_google_input_exceeds_maximum` | Google overflow 消息 → true |
| `test_generic_context_length_exceeded` | 通用 overflow pattern → true |
| `test_non_overflow_throttling_excluded` | Bedrock throttling 消息 → false |
| `test_non_overflow_rate_limit_excluded` | rate limit 消息 → false |
| `test_silent_overflow_detection` | stop=Stop, input > context_window → true |
| `test_no_overflow_on_normal_stop` | stop=Stop, input < context_window → false |
| `test_no_overflow_on_tool_use` | stop=ToolUse, no error message → false |

### 20.3 Tool 校验测试 (`tests/validation_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_valid_arguments_pass` | 符合 schema 的参数 → Ok |
| `test_coerce_string_to_number` | `"42"` 用于 integer 字段 → 自动转换后通过 |
| `test_coerce_string_to_bool` | `"true"` 用于 boolean 字段 → 自动转换后通过 |
| `test_missing_required_field` | 缺少 required 字段 → SchemaViolation |
| `test_wrong_type_uncoercible` | `"abc"` 用于 integer 字段 → SchemaViolation |
| `test_tool_not_found` | 不存在的 tool name → ToolNotFound |
| `test_coerce_number_to_string` | `42` 用于 string 字段 → 自动转换 `"42"` |
| `test_error_message_format` | 校验失败消息包含 path、field name 和 received args |

### 20.4 兼容性测试 (`tests/compat_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_detect_openai_standard` | openai + api.openai.com → 标准 compat |
| `test_detect_deepseek_compat` | deepseek → thinking_format=DeepSeek |
| `test_detect_openrouter_anthropic_cache` | openrouter + anthropic/* → cache_control_format=Anthropic |
| `test_detect_grok_no_reasoning_effort` | xai → supports_reasoning_effort=false |
| `test_merge_explicit_overrides_auto` | 显式字段覆盖 auto-detected，未设置字段保留 auto |
| `test_merge_fully_explicit` | 全部显式设置 → auto 不参与 |

---

## 21. 实现优先级与依赖关系

| 优先级 | 特性 | 新增依赖 | 前置依赖 |
|---|---|---|---|
| **P0** | 核心类型补全：text_signature (§3.1), thought_signature (§3.2), provider 字段 (§3.4), total_tokens (§3.5) | 无 | 无 |
| **P0** | StreamOptions 补全：max_retries, max_retry_delay_ms, headers, metadata, thinking_budgets (§6) | 无 | 无 |
| **P0** | Message Transformation (§25) | 无 | types.rs |
| **P0** | Cache/thinking 参数接入 provider body (§9.1, §9.2) | 无 | StreamOptions, CacheRetention |
| **P0** | Model Registry (§14) | `phf` | 无 |
| **P0** | Context Overflow Detection (§15) | `regex` | types.rs |
| **P0** | JSON Repair (§23) | 无（手写 ~200 行） | 无 |
| **P1** | Tool Call Validation (§16) | `jsonschema` | types.rs |
| **P1** | ThinkingBudgets + adjust_max_tokens_for_thinking (§6) | 无 | provider.rs |
| **P1** | Provider Compant Layer (§17) | 无 | Model Registry |
| **P2** | Prompt Cache Key (§18) | 无 | StreamOptions |
| **P2** | Provider Hooks (§19) | 无 | StreamOptions |
| **P3** | Mistral Provider (§9.4) | 无（复用 reqwest + SSE） | LlmProvider trait |
| **P3** | Bedrock Provider (§9.5) | `aws-sdk-bedrockruntime` | LlmProvider trait |
| **P3** | OpenRouter/Vercel Routing compat (§17.3) | 无 | OpenAiCompat |

---

## 22. 新增 workspace 依赖汇总

```toml
# Cargo.toml (workspace root)
[workspace.dependencies]
# ... existing ...
regex = "1"
phf = { version = "0.11", features = ["macros"] }

# crates/llm-client/Cargo.toml
[dependencies]
# ... existing ...
regex = { workspace = true }
phf = { workspace = true }
jsonschema = "0.28"

# Bedrock provider (P3, optional):
# aws-sdk-bedrockruntime = { version = "1", features = ["rustls"], optional = true }
```

---

## 23. JSON Repair (`src/repair.rs`)

### 23.1 动机

LLM 输出的 tool call arguments 可能包含：
- 未闭合的字符串（截断的 JSON）
- 多余的尾部逗号（如 `{"a": 1,}`）
- 单引号替代双引号
- Unicode 孤立 surrogate（Rust `String` 已过滤字节层，但 LLM 输出在解码后可能含）
- 非标准 whitespace 字符

pi-mono 依赖 `partial-json` npm 包处理此类问题。llm-client 手写等价的修复逻辑，不引入额外 crate。

### 23.2 API

```rust
/// Repair malformed JSON from LLM output.
///
/// Applies heuristics in order:
///   1. Fix unclosed strings by appending closing quote
///   2. Remove trailing commas before closing brackets/braces
///   3. Convert single-quoted strings to double-quoted
///   4. Escape unescaped control characters
///   5. Balance brackets (`[]`, `{}`) by appending missing closers
///   6. Strip non-printable Unicode
///
/// Returns the repaired string. If input is valid JSON, returned unchanged.
pub fn repair_json(s: &str) -> String;

/// Parse JSON with repair, returning `serde_json::Value` or the first parse error
/// (after repair heuristics fail).
pub fn parse_json_with_repair(s: &str) -> Result<serde_json::Value, serde_json::Error>;

/// Streaming JSON parser that accumulates partial fragments and attempts
/// parse on each delta. Used during tool call argument streaming to extract
/// partially-complete JSON objects.
pub struct StreamingJsonParser {
    buffer: String,
}

impl StreamingJsonParser {
    pub fn new() -> Self;

    /// Feed a delta fragment. Returns `Some(Value)` if a valid partial JSON
    /// can be parsed at this point.
    pub fn feed(&mut self, delta: &str) -> Option<serde_json::Value>;

    /// Consume the parser and return the final best-effort parse result.
    pub fn finalize(self) -> Result<serde_json::Value, serde_json::Error>;
}

/// Strip lone surrogates and other invalid Unicode from LLM output text.
pub fn sanitize_unicode(s: &str) -> String;
```

### 23.3 集成点

Provider 实现中，在以下位置调用：

1. **ToolCallDelta 累积**：`StreamingJsonParser::feed(tool_call_delta_fragment)` — 流式增量解析
2. **ToolCallEnd 完成**：`parse_json_with_repair(accumulated_json_string)` — 完成后修复并解析
3. **Error message**：`sanitize_unicode(error_message)` — 过滤非法 Unicode

代码量估算：~200 行，零第三方依赖。

### 23.4 测试计划 (`tests/repair_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_repair_unclosed_string` | `{"key":"val` → `{"key":"val"}` |
| `test_repair_trailing_comma_object` | `{"a":1,}` → `{"a":1}` |
| `test_repair_trailing_comma_array` | `[1,2,]` → `[1,2]` |
| `test_repair_single_quotes` | `{'a':'b'}` → `{"a":"b"}` |
| `test_valid_json_unchanged` | 有效 JSON → 返回原字符串 |
| `test_streaming_parser_accumulate` | feed 3 个 delta → 返回部分 JSON |
| `test_streaming_parser_finalize` | finalize → Ok(完整 JSON) |
| `test_sanitize_unicode_surrogates` | surrogate 字符 → stripped |

---

## 24. 增量补全总结

| 类别 | 改动项 | 涉及的 spec 章节 |
|---|---|---|
| **类型补全** | Content::Text.text_signature, ToolCall.thought_signature, Thinking.thinking_signature | §3.1, §3.2 |
| **类型补全** | AssistantMessage.provider, Usage.total_tokens + compute_total() | §3.4, §3.5 |
| **StreamOptions** | max_retries, max_retry_delay_ms, headers, metadata, thinking_budgets | §6 |
| **推理工具** | ThinkingBudgets, adjust_max_tokens_for_thinking() | §6 |
| **Compat** | zai_tool_stream, OpenRouterRouting, VercelGatewayRouting | §17.1 |
| **Provider** | Mistral (§9.4), Bedrock (§9.5) | §9 |
| **JSON Repair** | repair_json + StreamingJsonParser + sanitize_unicode | §23 |
| **文件结构** | 新增 src/repair.rs, 新增 provider/mistral.rs, provider/bedrock.rs | §1 |
| **依赖** | aws-sdk-bedrockruntime (P3), 其余全部零新增 | §2, §22 |
| **消息转换** | transform_messages, normalize_tool_call_id, 图片降级, thinking block 处理, orphan 补齐 | §25 |
| **OAuth** | OAuthToken, OAuthProvider trait, Anthropic/Copilot v0.1 | §26 |
| **Cache/Rasoning 接入** | Anthropic cache_control body + thinking params + beta headers, OpenAI prompt_cache_key + thinking_format | §9.1, §9.2 |
| **校验增强** | allOf/anyOf/oneOf union coercion, additionalProperties, 7 种类型转换 | §16.4 |
| **文件结构** | 新增 src/transform.rs, src/oauth.rs | §1 |

---

## 25. Message Transformation (`src/transform.rs`)

### 25.1 动机

跨 provider 切换模型时（如 OpenAI → Anthropic），消息格式存在不兼容问题：
- Anthropic 对 tool call ID 有严格长度限制（≤64 chars），OpenAI Responses 的 ID 可达 450+ 字符
- 非视觉模型不支持 `Content::Image` 块
- Thinking block 签名（Anthropic）无法跨模型传递
- Tool call 缺少前置 assistant message 导致 API 拒绝

pi-mono 的 `transformMessages()` 在每次 LLM 调用前执行标准化转换。llm-client 同样需要此层。

### 25.2 API

```rust
/// 消息转换选项
#[derive(Debug, Clone, Default)]
pub struct TransformOptions {
    /// 目标 API 协议 ("anthropic-messages" | "openai-completions" | ...)
    pub target_api: Option<String>,
    /// 目标模型是否支持图片输入
    pub supports_images: bool,
    /// 是否保留 thinking block（仅同模型跨 turn）
    pub preserve_thinking: bool,
}

/// 按目标 provider 要求标准化消息列表。
///
/// # 转换规则
///
/// 1. **Tool call ID 截断** (§25.3)
///    对长度超过 64 chars 的 tool_call_id，截断为 prefix + short_hash。
///    保留 id 唯一性（8 位 hex hash 后缀）。
///    同步更新对应的 ToolResultMessage.tool_call_id 以保持关联。
///
/// 2. **图片降级** (§25.4)
///    当 `supports_images == false` 时，将 `Content::Image` 替换为
///    `Content::Text { text: "(image omitted: model does not support images)" }`。
///    连续多张图片合并为一个占位符文本块。
///
/// 3. **Thinking block 处理** (§25.5)
///    当 `preserve_thinking == false` 时，移除所有 `Content::Thinking` 块。
///    redacted thinking block 也被移除（无法跨模型传递签名）。
///
/// 4. **Orphan tool call 补齐** (§25.6)
///    如果消息序列中存在 ToolResultMessage 之前没有对应的 AssistantMessage，
///    在前方自动插入一个空的 AssistantMessage 作为占位。
pub fn transform_messages(
    messages: &[Message],
    options: &TransformOptions,
) -> Vec<Message>;

/// Generate a short hex hash of a string (non-cryptographic).
/// Used for tool call ID truncation.
fn short_hash(s: &str) -> String;
```

### 25.3 Tool Call ID 标准化

```rust
/// 标准化 tool call ID
/// 规则:
///   - len ≤ 64: 原样返回
///   - len > 64: 对完整 ID 做 xxh64 → 8 位 hex hash → "call_{hash}id_{original[...last8]}"
///     确保 Anthropic 兼容的同时保留可追溯性
fn normalize_tool_call_id(id: &str) -> String {
    if id.len() <= 64 { return id.to_string(); }
    // 截断策略：取前 8 位 + hash 后缀保证唯一性
    let hash = short_hash(id);
    format!("call_{}{}", hash, &id[id.len().saturating_sub(8)..])
}
```

### 25.4 图片降级逻辑

```
for each message in messages:
  if message is UserMessage and content contains Image:
    if model does NOT support images:
      replace all consecutive Image blocks with:
        Content::Text { text: "(image omitted: model does not support images)" }
  if message is ToolResultMessage and content contains Image:
    same as above with text: "(tool image omitted: model does not support images)"
```

### 25.5 Thinking Block 处理

```
for each AssistantMessage in messages:
  if !preserve_thinking:
    content = content.filter(|c| c is not Content::Thinking)
    # 移除所有 thinking block（包括 redacted）
```

### 25.6 Orphan Tool Call 补齐

```
for each (prev, curr) in messages.windows(2):
  if prev is not AssistantMessage and curr is ToolResultMessage:
    insert placeholder AssistantMessage {
      content: [],
      provider: "system",
      model: "",
      api: Api { provider: "transform", model: "" },
      usage: Usage::default(),
      stop_reason: StopReason::ToolUse,
      timestamp: now(),
    } before curr
```

### 25.7 测试计划 (`tests/transform_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_tool_call_id_truncation` | 450-char id → 规范化后 ≤64 chars |
| `test_tool_call_id_preserves_mapping` | 截断 id 与对应 tool_result 的 id 一致 |
| `test_image_downgrade_non_vision` | Image → "(image omitted)" 占位符文本 |
| `test_image_merge_consecutive` | 连续 3 张图片 → 1 个占位符 |
| `test_image_preserved_vision_model` | supports_images=true → Image 原样保留 |
| `test_thinking_block_removed_cross_provider` | preserve_thinking=false → Thinking 块移除 |
| `test_thinking_block_preserved_same_model` | preserve_thinking=true → Thinking 块保留 |
| `test_orphan_tool_result_padded` | 孤 ToolResult → 插入空 AssistantMessage |

---

## 26. OAuth Provider 支持 (`src/oauth.rs`)

### 26.1 动机

pi-mono 支持 5 种 OAuth 认证方式（Anthropic OAuth, GitHub Copilot, Google Code Assist, Antigravity, OpenAI Codex）。llm-client 当前仅支持静态 API key。服务端场景需要支持 OAuth 以接入 Copilot、Anthropic Pro 等应用级认证。

v0.1 阶段仅定义 OAuth trait 抽象接口，Provider 适配器在后续版本实现。

### 26.2 API

```rust
use async_trait::async_trait;
use secrecy::SecretString;

/// OAuth token with expiry.
#[derive(Debug, Clone)]
pub struct OAuthToken {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_at: Option<std::time::SystemTime>,
    pub scopes: Vec<String>,
}

/// OAuth provider abstraction.
/// Implementations handle token acquisition, refresh, and persistence.
#[async_trait]
pub trait OAuthProvider: Send + Sync {
    /// Provider identifier (e.g., "github-copilot", "anthropic-oauth").
    fn provider_name(&self) -> &str;

    /// Acquire initial credentials (e.g., browser OAuth flow or device code).
    async fn login(&self) -> Result<OAuthToken, std::io::Error>;

    /// Refresh expired token.
    async fn refresh(&self, token: &OAuthToken) -> Result<OAuthToken, std::io::Error>;

    /// Read persisted token from disk (if supported).
    fn load_token(&self) -> Option<OAuthToken>;

    /// Persist token to disk (if supported).
    fn save_token(&self, token: &OAuthToken) -> std::io::Result<()>;
}
```

### 26.3 v0.1 内置 Provider

| Provider | 认证方式 | 实现状态 |
|---|---|---|
| Anthropic OAuth | Browser OAuth + `sk-ant-oat*` token | P3 |
| GitHub Copilot | Device code + Bearer token | P3 |

### 26.4 与 LlmProvider 集成

Provider 实现中，`stream()` 方法在解析 API key 时按优先级查找：
```
1. StreamOptions::api_key
2. LlmProvider state (constructor-set key)
3. OAuth token (load/refresh as needed)
4. Env var (provider-specific)
5. Err(AuthError)
```

OAuth token 在 `stream()` 调用前通过 `OAuthProvider::refresh()` 保证有效性。Token 持久化到 `~/.pandaria/auth.json` 或等效路径。

### 26.5 测试计划 (`tests/oauth_tests.rs`)

| 测试 | 验证点 |
|---|---|
| `test_oauth_token_serialization` | OAuthToken 序列化/反序列化 |
| `test_oauth_token_load_save` | 文件读写往返 |
| `test_oauth_refresh_flow` | 过期 token → refresh → 新 token |
