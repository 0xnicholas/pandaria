# llm-client v0.1 开发计划

> **状态:** 已完成 ✅ — v0.1 所有任务已落地（2026-05-03）
> **后续:** 见 `2026-05-03-llm-client-v0.2.md`
>
> **注:** 计划中列出的 `tests/` 目录下 6 个集成测试文件（retry_tests.rs, repair_tests.rs, models_tests.rs, overflow_tests.rs, validation_tests.rs, compat_tests.rs）以及 Phase 9 的 Mistral/Bedrock Provider 未在 v0.1 实现，已移至 v0.2 计划跟踪。

**Date:** 2026-05-02
**Status:** Completed
**Reference:** `docs/specs/2026-05-02-llm-client.md`, `AGENTS.md`
**Current Code:** `crates/llm-client/src/` (17 files, 5260 lines, 87 tests passing)

---

## 概述

将 llm-client crate 从当前 MVP（5 文件，3 字段 StreamOptions，5 变体事件流）升级到完整 spec 定义（25+ 文件，13 字段 StreamOptions，12 变体事件流，8 个新模块，5 个 provider）。

### 当前基线

```
crates/llm-client/src/
  lib.rs         # re-exports: LlmError, provider::*, streaming::*, types::*
  types.rs       # Content, ToolCall, UserMessage, AssistantMessage, ToolResultMessage,
                 # Message, Api, Usage, StopReason, LlmContext, ToolDef
  error.rs       # LlmError: 7 variants (Timeout no Duration field)
  provider.rs    # LlmProvider trait, StreamOptions (3 fields), MockProvider test
  streaming.rs   # AssistantMessageEvent (5 variants), AssistantMessageEventStream (type alias)
```

### 开发原则

- **Spec 驱动**：以 `docs/specs/2026-05-02-llm-client.md` 为目标
- **测试先行**：每个模块先写测试，再写实现
- **增量可编译**：每步完成后 `cargo build -p llm-client` 通过
- **阶段性 breaking change**：Phase 1 会破坏 agent-core（事件流 + Message 类型变更）。llm-client 内部先完成所有变更，Phase 2.2 一次性修复 agent-core。Phase 1-2.1 期间仅检查 `cargo build -p llm-client`，Phase 2.2 起恢复 `cargo build -p agent-core` 检查。

---

## Phase 0: 依赖与基础设施 (~30 min)

### T0.1 添加 workspace 依赖

**文件**: `Cargo.toml` (workspace root)

在 `[workspace.dependencies]` 追加：
```toml
regex = "1"
phf = { version = "0.11", features = ["macros"] }
```

### T0.2 更新 llm-client Cargo.toml

**文件**: `crates/llm-client/Cargo.toml`

追加依赖：
```toml
tokio = { workspace = true }
tracing = { workspace = true }
reqwest = { version = "0.12", features = ["stream", "json"] }
eventsource-stream = "0.6"
secrecy = { version = "0.10", features = ["serde"] }
regex = { workspace = true }
phf = { workspace = true }
jsonschema = "0.28"
```

追加 dev-dependencies：
```toml
wiremock = "0.6"
tokio-test = "0.4"
```

**验证**: `cargo build -p llm-client` 成功

---

## Phase 1: 核心类型补全 (~2h)

**每个 task 完成后更新 `src/lib.rs`：** 新增模块声明 (`pub mod xxx;`) 和 re-export。

### T1.1 升级 LlmError

**文件**: `crates/llm-client/src/error.rs`
**Spec**: §7.1

改动：
- `Timeout` 增加 `std::time::Duration` 字段
- 新增 `AuthError(String)` variant
- 新增 `ContextOverflow(String)` variant
- 新增 `StreamError(String)` variant
- 新增 `impl LlmError { pub fn is_retryable(&self) -> bool }`

**测试**: `cargo test -p llm-client -- error` (现有内联测试需更新)

### T1.2 Content 字段补全

**文件**: `crates/llm-client/src/types.rs`
**Spec**: §3.1, §3.2

改动：
- `Content::Text` 增加 `text_signature: Option<String>` (serde skip if None)
- `Content::Thinking` 增加 `thinking_signature: Option<String>` 和 `redacted: bool` (default)
- `ToolCall` 增加 `thought_signature: Option<String>`

**验证**: 现有 `types.rs` 内联测试需更新 roundtrip 用例

### T1.3 Message 类型补全

**文件**: `crates/llm-client/src/types.rs`
**Spec**: §3.4, §3.5

改动：
- `AssistantMessage` 增加 `provider: String` 字段
- `AssistantMessage` 增加 `model: String` 字段 (当前代码缺失)
- `Usage` 增加 `total_tokens: u64` + `#[serde(default)]`
- `Usage` 增加 `pub fn compute_total(&self) -> u64`

**agent-core 影响**: `crates/agent-core/src/loop.rs:135-143` 构造 `AssistantMessage` 的地方需增加 `provider` 和 `model` 字段。`loop.rs:93-94` 已有 `provider` 变量可用。

### T1.4 升级 AssistantMessageEvent

**文件**: `crates/llm-client/src/streaming.rs`
**Spec**: §4.1

从当前 5 变体升级到 12 变体：
- 所有变体增加 `content_index: usize` 和 `partial: AssistantMessage`
- 新增 `TextStart`, `TextEnd`
- 新增 `ThinkingStart`, `ThinkingDelta`, `ThinkingEnd`
- 新增 `ToolCallStart`, `ToolCallEnd`
- `ToolCallDelta` 从 `{ tool_call: ToolCall }` 改为 `{ delta: String }`（原始 JSON 片段）
- `Done` 改为 `{ reason: StopReason, message: AssistantMessage }`
- `Error` 改为 `{ error: AssistantMessage }`
- 保留 `Start` (增加 `partial`) 和 `TextDelta` (增加 `content_index`, `partial`)

**agent-core 影响**: `crates/agent-core/src/loop.rs:109-132` 的事件消费代码需重写以适配新事件格式。这是最大的 breaking change。

### T1.5 升级 AssistantMessageEventStream

**文件**: `crates/llm-client/src/streaming.rs`
**Spec**: §4.2

从 `Pin<Box<dyn Stream>>` 类型别名改为 concrete struct：

```rust
pub struct AssistantMessageEventStream {
    rx: tokio::sync::mpsc::Receiver<AssistantMessageEvent>,
    final_message: Option<Result<AssistantMessage, LlmError>>,
    terminated: bool,
}
```

方法：
- `pub fn new(buffer: usize) -> (Self, tokio::sync::mpsc::Sender<AssistantMessageEvent>)`
- `pub async fn next(&mut self) -> Option<AssistantMessageEvent>`
- `pub async fn to_message(mut self) -> Result<AssistantMessage, LlmError>`

**agent-core 影响**: `loop.rs:80-105` 的 stream 消费代码需改用 `stream.next()` 替代 `StreamExt::next()`。`loop.rs:235-253` 的测试 MockProvider 需改为创建 mpsc channel 并 spawn 一个 task 来发送事件。

---

## Phase 2: StreamOptions 与 Provider 升级 (~2h)

### T2.1 扩展 StreamOptions + 新建 cache.rs, hooks.rs

**文件**: `crates/llm-client/src/provider.rs` (修改), `src/cache.rs` (新建), `src/hooks.rs` (新建)
**Spec**: §6, §18, §19

**新建文件 `src/cache.rs`**:
```rust
pub enum CacheRetention { None, Short, Long }
impl CacheRetention { pub fn resolve(explicit: Option<Self>) -> Self; }
impl Default for CacheRetention { fn default() -> Self { Self::Short } }
```

**新建文件 `src/hooks.rs`**:
```rust
pub struct ProviderResponse { pub status: u16, pub headers: HashMap<String, String> }
pub type OnPayloadFn = Arc<dyn Fn(&mut serde_json::Value, &Model) -> Pin<Box<dyn Future<Output = bool>>>>;
pub type OnResponseFn = Arc<dyn Fn(&ProviderResponse, &Model) -> Pin<Box<dyn Future<Output = ()>>>>;
```

**文件 `src/provider.rs`** — StreamOptions 从 3 字段扩展到 13 字段：
- 新增 `api_key: Option<secrecy::SecretString>`
- 新增 `timeout: std::time::Duration` (default 60s)
- 新增 `reasoning: Option<ReasoningLevel>`
- 新增 `thinking_budgets: Option<ThinkingBudgets>`
- 新增 `max_retries: u32` (default 3)
- 新增 `max_retry_delay_ms: u64` (default 60_000)
- 新增 `headers: Option<HashMap<String, String>>`
- 新增 `metadata: Option<HashMap<String, String>>`
- 新增 `cache_retention: CacheRetention` (default Short, imported from cache.rs)
- 新增 `session_id: Option<String>`
- 新增 `on_payload: Option<OnPayloadFn>` (imported from hooks.rs)
- 新增 `on_response: Option<OnResponseFn>` (imported from hooks.rs)

类型定义（`provider.rs` 内）：
- `ReasoningLevel` enum (§6)
- `ThinkingBudgets` struct (§6)
- Manual `Debug` impl (redacts api_key, skips callback types)

新增函数：
- `pub fn adjust_max_tokens_for_thinking(...) -> (u32, u32)` (§6)

新增 `LlmProvider` trait 的 `api_for()` default method (§5)。

**注意**: `CacheRetention` 定义在 `src/cache.rs`，`OnPayloadFn`/`OnResponseFn`/`ProviderResponse` 定义在 `src/hooks.rs`。`provider.rs` 通过 `use crate::cache::CacheRetention` 和 `use crate::hooks::*` 导入。Phase 7 时 `cache.rs` 和 `hooks.rs` 已存在，无需移动。

### T2.2 更新 agent-core 适配

**文件**: `crates/agent-core/src/loop.rs`, `crates/agent-core/src/session.rs`

- 更新 `StreamOptions::default()` 调用（字段变化但 `..Default::default()` 仍然有效）
- 更新事件消费 loop 以适配新 `AssistantMessageEvent` 变体和 `next()` API
- 更新测试中的 MockProvider 实现
- 构造 `AssistantMessage` 时传入 `provider` 和 `model` 字段

**验证**: `cargo build -p agent-core` 通过，`cargo test -p agent-core` 通过

---

## Phase 3: 工具模块 (~4h)

### T3.1 util.rs — 工具函数

**文件**: `crates/llm-client/src/util.rs` (新建)
**Spec**: §10

```rust
pub fn extract_tool_calls(content: &[Content]) -> Vec<ToolCall>;
pub fn build_tool_defs(tools: &[ToolDef]) -> Vec<ToolDef>;
```

`build_tool_defs` 等价于 `tools.to_vec()`，提供语义化命名。

### T3.2 retry.rs — 指数退避重试

**文件**: `crates/llm-client/src/retry.rs` (新建)
**Spec**: §8

```rust
pub async fn with_retry<F, Fut>(
    operation: F,
    max_retries: u32,
    max_retry_delay_ms: Option<u64>,
) -> Result<Fut::Output, LlmError>
```

- 重试触发: `RateLimited | Overloaded | Timeout`
- 退避: 100ms → 200ms → 400ms
- `max_retry_delay_ms` 超限时立即返回错误
- tracing instrument 记录 `retry_count`

**测试**: `tests/retry_tests.rs` — 5 个用例 (§11.4)

### T3.3 repair.rs — JSON 修复

**文件**: `crates/llm-client/src/repair.rs` (新建)
**Spec**: §23

```rust
pub fn repair_json(s: &str) -> String;
pub fn parse_json_with_repair(s: &str) -> Result<serde_json::Value, serde_json::Error>;
pub struct StreamingJsonParser;
impl StreamingJsonParser {
    pub fn new() -> Self;
    pub fn feed(&mut self, delta: &str) -> Option<serde_json::Value>;
    pub fn finalize(self) -> Result<serde_json::Value, serde_json::Error>;
}
pub fn sanitize_unicode(s: &str) -> String;
```

实现 6 步启发式修复（§23.2）。手写 ~200 行，零三方依赖。

**测试**: `tests/repair_tests.rs` — 8 个用例 (§23.4)

---

## Phase 4: 模型注册表 (~3h)

### T4.1 Model 类型定义 (不含 compat 字段)

**文件**: `crates/llm-client/src/models.rs` (新建)
**Spec**: §14.1, §14.3

```rust
pub enum Modality { Text, Image }
pub struct TokenCost { pub input, output, cache_read, cache_write: f64 }
pub struct Model { pub id, name, api, provider, base_url: String, pub reasoning: bool,
    pub input_modalities: Vec<Modality>, pub cost: TokenCost, pub context_window: u32,
    pub max_tokens: u32, pub headers: Option<HashMap<String, String>> }
// compat 字段延后到 T6.2 完成后再添加 (需要 OpenAiCompat / AnthropicCompat 类型)

pub fn calculate_cost(model: &Model, usage: &Usage) -> TokenCost;
pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool;
pub fn supports_xhigh(model_id: &str) -> bool;
```

**注意**: `Model::compat` 字段依赖 T6.2 定义的 `OpenAiCompat` 和 `AnthropicCompat` 类型。T4.1 先定义不含 compat 的 Model，T6.2 完成后再补充 `compat` 字段和 `models_data.rs` 中的 compat 数据。

### T4.2 模型数据与注册表

**文件**: `crates/llm-client/src/models_data.rs` (新建)
**Spec**: §14.2, §14.4

**实现方式**：不使用嵌套 `phf::Map<&str, phf::Map<&str, Model>>`（嵌套 compile-time map 在 phf crate 中支持有限）。

替代方案 — 两级索引用两个 `phf::Map` 实现：

```rust
// models_data.rs
// 第一级: model_id → Model (flat, all models keyed by "provider/model_id")
static MODELS: phf::Map<&'static str, Model> = phf_map! {
    "anthropic/claude-sonnet-4-20250514" => Model { ... },
    "openai/gpt-5.2" => Model { ... },
    // ... 18 entries ...
};

// 第二级: provider → Vec<&str> (model_ids for that provider)
static PROVIDER_MODELS: phf::Map<&'static str, &'static [&'static str]> = phf_map! {
    "anthropic" => &["claude-sonnet-4-20250514", "claude-opus-4-7", ...],
    "openai" => &["gpt-5.2", "gpt-5.3", ...],
    "google" => &["gemini-2.5-pro", ...],
};
```

`ModelRegistry` 基于这两个 map 实现 `get()` 和 `models_for_provider()` 查找。

覆盖 18 个核心模型：
- Anthropic (6): claude-sonnet-4-*, claude-opus-4-*, claude-haiku-4-*
- OpenAI (8): gpt-5.*, gpt-5.1-codex
- Google (4): gemini-2.5-*, gemini-3.0-*

**文件**: `crates/llm-client/src/models.rs` (续)

`ModelRegistry` 基于 `models_data.rs` 中的两个 `phf::Map` 实现：

```rust
pub struct ModelRegistry;

impl ModelRegistry {
    pub fn builtin() -> &'static Self;
    pub fn get(&self, provider: &str, model_id: &str) -> Option<&'static Model> {
        models_data::MODELS.get(&format!("{}/{}", provider, model_id))
    }
    pub fn models_for_provider(&self, provider: &str) -> Vec<&'static Model>;
    pub fn providers(&self) -> Vec<&'static str>;
}

pub fn get_model(provider: &str, model_id: &str) -> Option<&'static Model>;
pub fn models_for_provider(provider: &str) -> Vec<&'static Model>;
pub fn providers() -> Vec<&'static str>;
```

**测试**: `tests/models_tests.rs` — 7 个用例 (§20.1)

---

## Phase 5: 上下文溢出检测 (~2h)

### T5.1 overflow.rs

**文件**: `crates/llm-client/src/overflow.rs` (新建)
**Spec**: §15

```rust
pub fn is_context_overflow(
    error_message: Option<&str>,
    stop_reason: &StopReason,
    context_window: Option<u32>,
    input_tokens: u64,
    cache_read_tokens: u64,
) -> bool;
```

实现：
- 19 个溢出 regex 模式 (编译为 `LazyLock<Vec<Regex>>`)
- 3 个排除模式 (rate limit, throttling, 429)
- 静默溢出检测 (stop=Stop 但 tokens > context_window)

**测试**: `tests/overflow_tests.rs` — 9 个用例 (§20.2)

---

## Phase 6: Tool 校验与 Compat 层 (~3h)

### T6.1 validation.rs — Tool Call 参数校验

**文件**: `crates/llm-client/src/validation.rs` (新建)
**Spec**: §16

```rust
pub enum ValidationError { ToolNotFound(String), SchemaViolation { ... } }
pub struct ValidationMessage { pub path, message: String }

pub fn validate_tool_arguments(tool: &ToolDef, tool_call: &ToolCall) -> Result<Value, ValidationError>;
pub fn validate_tool_call(tools: &[ToolDef], tool_call: &ToolCall) -> Result<Value, ValidationError>;

// 内部:
fn coerce_arguments(args: &mut serde_json::Value, schema: &serde_json::Value);
```

- 使用 `jsonschema` crate 校验
- Schema 缓存: `LazyLock<Mutex<HashMap<String, JSONSchema>>>`
- Best-effort 强制转换: String→Number, String→Bool, Number→String
- 校验失败消息格式与 pi-mono 对齐

**测试**: `tests/validation_tests.rs` — 8 个用例 (§20.3)

### T6.2 compat.rs — Provider 兼容层

**文件**: `crates/llm-client/src/compat.rs` (新建)
**Spec**: §17

类型定义:
- `OpenAiCompat` (18 字段)
- `AnthropicCompat` (2 字段)
- `MaxTokensField`, `ThinkingFormat`, `CacheControlFormat` 枚举
- `OpenRouterRouting`, `VercelGatewayRouting` structs

函数:
- `detect_openai_compat(provider, base_url, model_id) -> OpenAiCompat`
- `detect_anthropic_compat(provider, base_url) -> AnthropicCompat`
- `merge_openai_compat(baseline, explicit) -> OpenAiCompat`
- `merge_anthropic_compat(baseline, explicit) -> AnthropicCompat`

**测试**: `tests/compat_tests.rs` — 6 个用例 (§20.4)

**T6.2 完成后回填**: 在 `models.rs` 中补全 `pub compat: ModelCompat` 字段，并更新 `models_data.rs` 中 18 个模型的 compat 数据。

---

## Phase 7: Cache 与 Hooks 完善 (已部分完成)

`cache.rs` 和 `hooks.rs` 的文件和类型已在 Phase 2 创建。本 Phase 仅添加验证测试。如果 Phase 2 时未编写 `CacheRetention::resolve()` 的实现逻辑，此处补全。

### T7.1 cache.rs — 补全实现

**文件**: `crates/llm-client/src/cache.rs`
**Spec**: §18

如果 Phase 2 时仅定义了类型签名，此处补全：
- `CacheRetention::resolve()` 的 env var 检查逻辑 (`PI_CACHE_RETENTION == "long"`)

### T7.2 hooks.rs — 验证

**文件**: `crates/llm-client/src/hooks.rs`

类型已在 Phase 2 定义。如果 Phase 2 时未提供 `Default` impl 或辅助函数，此处补全。

---

## Phase 8: Provider 实现 (~10h)

### T8.1 公共基础设施

**文件**: `crates/llm-client/src/providers/mod.rs` (新建)

- 定义 `API_KEY_RESOLUTION_ORDER`: `StreamOptions::api_key` → env var (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GOOGLE_API_KEY`) → `Err(AuthError)`
- 统一 `reqwest::Client` 构造辅助（timeout, headers 合并）
- SSE 流处理辅助：`eventsource-stream` 的 `EventStream` 适配到 `tokio` task
- JSON 流处理辅助：NDJSON/JSON-lines 解析

### T8.2 AnthropicProvider

**文件**: `crates/llm-client/src/providers/anthropic.rs` (新建)
**Spec**: §9.1

实现 `LlmProvider` trait。

**reqwest Client 构造**:
- Base URL: `https://api.anthropic.com/v1/messages`
- Headers: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`
- 合并 `StreamOptions::headers`

**消息转换** (llm-client → Anthropic format):
- `system_prompt` → `system: [{ text, cache_control }]`
- `UserMessage` → `{ role: "user", content: [{ type: "text", text }, ...] }`
- `AssistantMessage` → `{ role: "assistant", content: [{ type: "text" }, { type: "thinking" }, { type: "tool_use" }] }`
- `ToolResultMessage` → `{ role: "user", content: [{ type: "tool_result", tool_use_id, content }] }`
- `ToolDef` → `{ name, description, input_schema }`

**SSE 事件映射** (§9.1 详细表):
- `message_start` → `Start { partial }`
- `content_block_start`:
  - `"text"` → `TextStart`
  - `"tool_use"` → `ToolCallStart { name, id }`
  - `"thinking"` → `ThinkingStart`
  - `"redacted_thinking"` → `ThinkingStart { redacted: true }`
- `content_block_delta`:
  - `text_delta` → `TextDelta`
  - `input_json_delta` → `ToolCallDelta`
  - `thinking_delta` → `ThinkingDelta`
  - `signature_delta` → 更新 thinking_signature
- `content_block_stop`:
  - `text` → `TextEnd`
  - `tool_use` → 解析 partialJson → `ToolCallEnd`
  - `thinking` → `ThinkingEnd`
- `message_delta` → 更新 partial 的 usage, stop_reason
- `message_stop` → `Done`

**错误处理**:
- HTTP 4xx/5xx → 编码为 `AssistantMessageEvent::Error`
- SSE 解析失败 → `Error { error: AssistantMessage { stop_reason: Error, error_message: "..." } }`
- `CancellationToken` 检查在每次 stream chunk 后

**Cache control**:
- 根据 `StreamOptions::cache_retention` 设置 `cache_control: { type: "ephemeral", ttl? }`
- 标记位置: 所有 system text blocks + 最后 tool + 最后 user content

**Thinking handling**:
- 根据 `ReasoningLevel` 设置 Anthropic thinking 参数:
  - Adaptive (Opus 4.6+, Sonnet 4.6): `thinking: { type: "adaptive", display: "summarized" }` + `output_config: { effort }`
  - Budget (pre-4.6): `thinking: { type: "enabled", budget_tokens }`
- `thinking_signature` 累积与跨 turn 传递

**Hooks**:
- Step 2: `on_payload(&mut payload_json, &model)`
- Step 4: `on_response(&ProviderResponse { status, headers }, &model)`

### T8.3 OpenAiProvider

**文件**: `crates/llm-client/src/providers/openai.rs` (新建)
**Spec**: §9.2

实现 `LlmProvider` trait。

**reqwest Client 构造**:
- Base URL: `https://api.openai.com/v1/chat/completions`
- Headers: `Authorization: Bearer <key>`, `content-type: application/json`
- 合并 `StreamOptions::headers`

**消息转换**:
- `system_prompt` → `messages[0]: { role: "system", content: text }`
- `UserMessage` → `{ role: "user", content: [{ type: "text", text }, ...] }`
- `AssistantMessage` → `{ role: "assistant", content: [{ type: "text" }, { tool_calls: [...] }] }`
- `ToolResultMessage` → `{ role: "tool", tool_call_id, content }`

**SSE 事件映射** (§9.2 详细表):
- `response.created` → `Start`
- `choice.delta.content` → `TextDelta` (第一个 delta 前自动发 `TextStart`)
- `choice.delta.tool_calls[i]`:
  - `id + function.name 出现` → `ToolCallStart`
  - `function.arguments` → `ToolCallDelta`
- `choice.delta.reasoning_content` (或 `reasoning/reasoning_text`) → `ThinkingDelta`
- `choice.finish_reason` → `Done`

**Tool call 累积**: 按 `tool_calls[i].index` 索引，每个 tool call 独立累积 `function.arguments` 片段。收到 `finish_reason` 时解析所有累积的 JSON。

**Compat 集成**:
- 通过 `detect_openai_compat()` + Model 查找 resolved compat
- `thinking_format` 决定 reasoning 参数格式 (§17)
- `max_tokens_field` 决定用 `max_tokens` 还是 `max_completion_tokens`
- `reasoning_effort_map` 做 ThinkingLevel → provider string 映射

**Cache**:
- `api.openai.com`: `prompt_cache_key = session_id` (when cache_retention != None)
- Long retention: `prompt_cache_retention: "24h"` + `session_id`/`x-client-request-id` headers

### T8.4 GoogleProvider

**文件**: `crates/llm-client/src/providers/google.rs` (新建)
**Spec**: §9.3

实现 `LlmProvider` trait。

**reqwest Client 构造**:
- Base URL: `https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent`
- Auth: `x-goog-api-key` header 或 `Authorization: Bearer`
- 合并 `StreamOptions::headers`

**消息转换**:
- `system_prompt` → `systemInstruction: { parts: [{ text }] }`
- `UserMessage` → `{ role: "user", parts: [{ text }] }`
- `AssistantMessage` → `{ role: "model", parts: [{ text } | { functionCall } | { thought: true, text }] }`
- `ToolResultMessage` → `{ role: "user", parts: [{ functionResponse: { name, response } }] }`

**JSON 流解析** (每个 chunk 是 `GenerateContentResponse`):
- `candidates[0].content.parts[]`:
  - `{ text, thought: true }` → `ThinkingDelta`
  - `{ text }` → `TextDelta`
  - `{ functionCall }` → `ToolCallEnd` (一次性完整)
- 停止原因: `candidates[0].finishReason` → StopReason 映射

**Thinking 处理**:
- `thought: true` flag 区分 thinking vs text
- `thought_signature` 在 `content.parts[i].thoughtSignature` 中保留

**Tool call ID 生成**: counter-based, prefix `call_`

**错误处理**: HTTP 错误 + JSON 解析失败 → `AssistantMessageEvent::Error`

---

## Phase 9: 扩展 Provider (P3, 可选)

### T9.1 MistralProvider

**文件**: `crates/llm-client/src/providers/mistral.rs` (新建)
**Spec**: §9.4

- SSE 解析 (与 OpenAI Completions 同构)
- Tool call ID 截断 (≤36 chars, shortHash)
- `promptMode: "reasoning"` + `reasoningEffort`

### T9.2 AwsBedrockProvider

**文件**: `crates/llm-client/src/providers/bedrock.rs` (新建)
**Spec**: §9.5

- 依赖 `aws-sdk-bedrockruntime` (optional feature gate)
- SigV4 签名认证
- ConverseStream API 事件映射
- CachePoint/CacheTTL 缓存标记
- 自适应推理 vs 预算推理 dispatching

---

## 任务依赖图

```
T0.1 ──→ T0.2 ──→ T1.1 ──→ T1.2 ──→ T1.3 ──→ T1.4 ──→ T1.5
                                         │
                            ┌────────────┼────────────┐
                            ▼            ▼            ▼
                         T2.1      T3.1 T3.2 T3.3   T4.1
                            │            │            │
                            ▼            └────────────┘
                         T2.2                        │
                      (agent-core                    ▼
                       适配)                     T4.2
                            │                       │
                            └───────────┬───────────┘
                                        ▼
                                     T5.1
                                        │
                              ┌─────────┴─────────┐
                              ▼                   ▼
                           T6.1               T6.2
                              │                   │
                              └─────────┬─────────┘
                                        │
                              ┌─────────┴─────────┐
                              ▼                   ▼
                           T7.1               T7.2
                              │                   │
                              └─────────┬─────────┘
                                        ▼
                                     T8.1 ──→ T8.2 ──→ T8.3 ──→ T8.4
                                                    │
                                                    ▼
                                              T9.1, T9.2 (P3)
```

注意：
- **Phase 1 完成后，Phase 2 (T2.1)、Phase 3 (T3.x)、Phase 4 (T4.1) 可并行启动**
- T2.1 依赖 T1.5 完成（需要新的 `AssistantMessageEventStream` 类型）
- T3.x 仅依赖 Phase 1 完成的类型（Content, ToolDef, LlmError）— 不依赖 StreamOptions
- T4.1 依赖 `OpenAiCompat`, `AnthropicCompat` 类型 — 这些由 T6.2 提供。T4.1 先定义 `Model` + `TokenCost` 等不需要 compat 的类型，在 T6.2 完成后补全 `ModelCompat` 字段和 `models_data.rs` 中的 compat 数据

---

## 时间估算

| Phase | 内容 | 预估时间 |
|---|---|---|
| Phase 0 | 依赖与基础设施 | 30 min |
| Phase 1 | 核心类型补全 (5 tasks) | 2h |
| Phase 2 | StreamOptions + agent-core 适配 (2 tasks) | 2h |
| Phase 3 | 工具模块: util, retry, repair (3 tasks) | 3h |
| Phase 4 | 模型注册表 (2 tasks) | 3h |
| Phase 5 | 上下文溢出检测 | 2h |
| Phase 6 | Tool 校验 + Compat 层 (2 tasks) | 3h |
| Phase 7 | Cache + Hooks 完善 | 30 min |
| Phase 8 | Provider 实现 (4 tasks) | 12h |
| Phase 9 | 扩展 Provider (可选) | 6h |
| **总计** | | **~36h** (含 P3) / **~30h** (不含 P3) |

---

## 验证检查点

每 task 完成后执行：

```bash
cargo build -p llm-client           # 编译通过
cargo test -p llm-client            # 测试通过 (如适用)
```

**Phase 1-2.1 期间**: agent-core 编译会中断（breaking changes）。仅检查 llm-client。

**Phase 2.2 起**:
```bash
cargo build -p agent-core           # agent-core 编译通过
cargo test -p agent-core            # agent-core 测试通过
```

**全部完成**:
```bash
cargo clippy -p llm-client          # 无 lint 警告
```

### Breaking Change 管理

Phase 1.4-1.5 (AssistantMessageEvent + Stream 升级) 和 Phase 1.3 (AssistantMessage 结构变更) 会破坏 agent-core。

处理策略：
1. 先在 llm-client 完成所有类型变更
2. 再一次性更新 agent-core 的适配代码
3. agent-core 适配放在 Phase 2.2，作为 breaking change 的最终消费点

---

## 新增文件汇总

| 文件 | Phase | Spec 章节 |
|---|---|---|
| `src/util.rs` | P3 | §10 |
| `src/retry.rs` | P3 | §8 |
| `src/repair.rs` | P3 | §23 |
| `src/models.rs` | P4 | §14.1, §14.3 |
| `src/models_data.rs` | P4 | §14.4 |
| `src/overflow.rs` | P5 | §15 |
| `src/validation.rs` | P6 | §16 |
| `src/compat.rs` | P6 | §17 |
| `src/cache.rs` | P7 | §18 |
| `src/hooks.rs` | P7 | §19 |
| `src/providers/mod.rs` | P8 | §9 |
| `src/providers/anthropic.rs` | P8 | §9.1 |
| `src/providers/openai.rs` | P8 | §9.2 |
| `src/providers/google.rs` | P8 | §9.3 |
| `src/providers/mistral.rs` | P9 | §9.4 |
| `src/providers/bedrock.rs` | P9 | §9.5 |
| `tests/retry_tests.rs` | P3 | §11.4 |
| `tests/repair_tests.rs` | P3 | §23.4 |
| `tests/models_tests.rs` | P4 | §20.1 |
| `tests/overflow_tests.rs` | P5 | §20.2 |
| `tests/validation_tests.rs` | P6 | §20.3 |
| `tests/compat_tests.rs` | P6 | §20.4 |
