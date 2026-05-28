# ai-provider v0.2 开发实施计划

> **Status:** Completed ✅ — all tasks delivered

**Goal:** 将 ai-provider crate 从 v0.1（核心功能完成）升级到完整 spec 覆盖，补全 wiremock 集成测试、P3 Provider（Mistral/Bedrock）、OAuth 抽象层，并完成最终质量验证。

**优先级: P1** — 不阻塞其他模块，可与 agent-core / extensions 全程并行开发。

**Architecture:** 基于 v0.1 已完成的模块化设计（types → streaming → provider → providers/* → tools），增量补全缺失的集成测试和可选 Provider 实现。保持现有接口稳定，agent-core 零破坏。

**Tech Stack:** Rust 2024, tokio, reqwest, wiremock, serde, secrecy, regex, jsonschema, eventsource-stream, aws-sdk-bedrockruntime (optional)

**联合开发时序:**
```
全程并行: ai-provider v0.2 P1 (Phase 1) 
          └─ 与 agent-core Phase 0-9 + extensions Phase 1-4 完全独立，可同时执行
可选并行: ai-provider v0.2 P3 (Phase 2-3)
          └─ 建议在所有核心模块（agent-core + extensions）P0 完成后启动
```

---

## 当前基线（2026-05-03）

v0.1 已完成（87 单元测试 + 2 集成测试全部通过，agent-core 零破坏）：

```
crates/ai-provider/src/
  types.rs          286 lines  ✅ Content/Message/ToolCall/Usage/StopReason
  streaming.rs      396 lines  ✅ 12-variant AssistantMessageEvent + mpsc Stream
  provider.rs       331 lines  ✅ LlmProvider trait + StreamOptions (15 fields)
  providers/
    anthropic.rs    686 lines  ✅ SSE + cache_control + thinking + beta headers
    openai.rs       467 lines  ✅ SSE + reasoning + prompt_cache_key
    google.rs       378 lines  ✅ JSON stream + x-goog-api-key
  retry.rs          170 lines  ✅ with_retry()
  repair.rs         236 lines  ✅ StreamingJsonParser + repair_json
  models.rs         214 lines  ✅ ModelRegistry (LazyLock<HashMap>, 18 models)
  models_data.rs    407 lines  ✅ 编译期模型数据
  overflow.rs       183 lines  ✅ 19 regex + 3 exclusion patterns
  validation.rs     354 lines  ✅ jsonschema + 7 coercion types
  compat.rs         319 lines  ✅ OpenAiCompat/AnthropicCompat + detect/merge
  cache.rs           45 lines  ✅ CacheRetention
  hooks.rs           27 lines  ✅ OnPayloadFn/OnResponseFn
  transform.rs      399 lines  ✅ normalize IDs + downgrade images + pad orphans
  util.rs            71 lines  ✅ extract_tool_calls + build_tool_defs
  error.rs           45 lines  ✅ LlmError (10 variants)
  lib.rs             37 lines  ✅ 统一 re-export

tests/
  anthropic_smoke.rs           ✅ 2 个集成测试（真实 HTTP）
  
缺失: wiremock mock HTTP 测试, Mistral/Bedrock Provider, OAuth
```

---

## 文件结构规划

```
crates/ai-provider/
├── src/
│   ├── lib.rs                    # 新增 re-export: mistral, bedrock, oauth
│   ├── oauth.rs                  # NEW OAuthToken + OAuthProvider trait
│   └── providers/
│       ├── mod.rs                # 新增 mistral/bedrock feature gate
│       ├── mistral.rs            # NEW MistralProvider
│       └── bedrock.rs            # NEW AwsBedrockProvider (optional)
└── tests/
    ├── types_serde.rs            # NEW type serialization roundtrip (10 cases)
    ├── anthropic_tests.rs        # NEW wiremock HTTP mock (4 cases)
    ├── openai_tests.rs           # NEW wiremock HTTP mock (3 cases)
    ├── google_tests.rs           # NEW wiremock HTTP mock (2 cases)
    ├── streaming_tests.rs        # NEW EventStream behavior (5 cases)
    ├── retry_tests.rs            # NEW with_retry policy (5 cases)
    ├── overflow_tests.rs         # NEW overflow detection (9 cases)
    ├── validation_tests.rs       # NEW tool validation (8 cases)
    ├── models_tests.rs           # NEW ModelRegistry (7 cases)
    ├── compat_tests.rs           # NEW compat auto-detect (6 cases)
    ├── repair_tests.rs           # NEW JSON repair (8 cases)
    ├── transform_tests.rs        # NEW message transform (8 cases)
    ├── security_tests.rs         # NEW API key leak prevention (3 cases)
    └── oauth_tests.rs            # NEW OAuth (3 cases)
```

---

## Phase 1: 测试补全与质量提升（P1 — 全程可并行，~7h）

### Task 1.1: wiremock HTTP Mock 集成测试 (P1)

**Files:**
- Create: `crates/ai-provider/tests/anthropic_tests.rs`
- Create: `crates/ai-provider/tests/openai_tests.rs`
- Create: `crates/ai-provider/tests/google_tests.rs`

**背景:** 当前仅有 `anthropic_smoke.rs`（2 测试），使用真实 HTTP 请求。需要 wiremock 本地 mock 测试覆盖所有 Provider 的 SSE/JSON 流解析逻辑。

**测试矩阵:**

| Provider | 测试用例 | 验证点 |
|---------|---------|--------|
| Anthropic | `test_mock_basic_text_stream` | message_start → TextStart/TextDelta/TextEnd/Done |
| Anthropic | `test_mock_tool_call_streaming` | input_json_delta 累积 → ToolCallEnd |
| Anthropic | `test_mock_thinking_streaming` | thinking_delta + signature → ThinkingEnd |
| Anthropic | `test_mock_error_response` | HTTP 4xx → Error 事件编码 |
| OpenAI | `test_mock_basic_text_stream` | response.created → TextDelta/Done |
| OpenAI | `test_mock_tool_call_streaming` | tool_calls delta → ToolCallEnd |
| OpenAI | `test_mock_reasoning_stream` | reasoning_content → ThinkingDelta |
| Google | `test_mock_basic_text_stream` | JSON stream → TextDelta/TextEnd/Done |
| Google | `test_mock_tool_call` | functionCall → ToolCallEnd |

**关键实现注意:**
- wiremock 测试需要修改 Provider 的 `base_url`（通过 `with_base_url()` 或类似方法）
- SSE body 需要严格遵循各 Provider 的原生格式
- 测试使用 `stream.next().await` 逐个验证事件顺序

**依赖:** `wiremock = "0.6"`（已在 dev-dependencies）

- [ ] **Step 1.1.1: 创建 anthropic wiremock 测试**

使用 `wiremock::MockServer` 模拟 Anthropic SSE 流，验证事件解析。

```rust
use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::{method, path, header};

#[tokio::test]
async fn test_mock_basic_text_stream() {
    let server = MockServer::start().await;
    
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_raw(
                "event: message_start\ndata: {...}\n\nevent: content_block_start\ndata: {...}\n\n...",
                "text/event-stream"
            ))
        .mount(&server)
        .await;
    
    // 使用 server.uri() 构造 provider
    let provider = AnthropicProvider::with_base_url(server.uri());
    let stream = provider.stream("claude-sonnet-4", context, options, token).await.unwrap();
    
    // 验证事件序列
    let events = collect_events(stream).await;
    assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
    assert!(matches!(events[1], AssistantMessageEvent::TextStart { .. }));
    // ...
}
```

- [ ] **Step 1.1.2: 创建 openai wiremock 测试**

```rust
#[tokio::test]
async fn test_mock_tool_call_streaming() {
    let server = MockServer::start().await;
    
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_raw(
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_123\",\"function\":{\"name\":\"test\"}}]}}]}\n\n...",
                "text/event-stream"
            ))
        .mount(&server)
        .await;
    
    // 验证 ToolCallStart + ToolCallDelta + ToolCallEnd 序列
}
```

- [ ] **Step 1.1.3: 创建 google wiremock 测试**

Google 使用 JSON stream（非 SSE），每个 chunk 是一个完整的 JSON 对象。

- [ ] **Step 1.1.4: 运行测试**

```bash
cargo test -p ai-provider --test anthropic_tests --test openai_tests --test google_tests
```

Expected: 9 passed

- [ ] **Step 1.1.5: Commit**

```bash
git add crates/ai-provider/tests/anthropic_tests.rs crates/ai-provider/tests/openai_tests.rs crates/ai-provider/tests/google_tests.rs
git commit -m "test(ai-provider): add wiremock HTTP mock tests for providers"
```

---

### Task 1.2: 模块级集成测试迁移 (P1)

**Files:**
- Create: `crates/ai-provider/tests/types_serde.rs`
- Create: `crates/ai-provider/tests/streaming_tests.rs`
- Create: `crates/ai-provider/tests/retry_tests.rs`
- Create: `crates/ai-provider/tests/overflow_tests.rs`
- Create: `crates/ai-provider/tests/validation_tests.rs`
- Create: `crates/ai-provider/tests/models_tests.rs`
- Create: `crates/ai-provider/tests/compat_tests.rs`
- Create: `crates/ai-provider/tests/repair_tests.rs`
- Create: `crates/ai-provider/tests/transform_tests.rs`
- Create: `crates/ai-provider/tests/security_tests.rs`

**背景:** 当前这些测试都是内联 `#[cfg(test)]` 模块。根据 AGENTS.md 规范，集成测试应放在 `tests/` 目录。

**策略:** 
1. 将现有内联测试复制到 `tests/` 目录作为集成测试，保留原内联测试作为单元测试（双保险）
2. **新增 spec 要求的额外测试用例**（当前内联测试中缺失的）

**测试清单:**

| 文件 | 用例数 | 来源 | 备注 |
|------|--------|------|------|
| types_serde.rs | 10 | Spec §11.1 | **新增文件**，覆盖 Content/Message/StopReason/ToolDef 序列化 |
| streaming_tests.rs | 5 | Spec §11.2, §4.2 | 含 `test_event_stream_content_index_tracking`（多 block 交替） |
| retry_tests.rs | 5 | Spec §11.4, §8 | 含 `test_exponential_backoff_timing`（验证延迟） |
| overflow_tests.rs | 9 | Spec §20.2, §15 | 含 `test_silent_overflow_detection`（静默溢出） |
| validation_tests.rs | 8 | Spec §20.3, §16 | 含 `test_error_message_format`（错误消息格式） |
| models_tests.rs | 7 | Spec §20.1, §14 | 含 `test_supports_xhigh`（模型支持检查） |
| compat_tests.rs | 6 | Spec §20.4, §17 | 含 OpenRouter/Vercel routing 测试 |
| repair_tests.rs | 8 | Spec §23.4, §23 | 含 `test_streaming_parser_accumulate`（流式解析） |
| transform_tests.rs | 8 | Spec §25.7, §25 | 含 `test_image_merge_consecutive`（连续图片合并） |
| security_tests.rs | 3 | Spec §11.5, §7.2 | 含 `test_provider_error_no_raw_body`（错误消息过滤） |

- [ ] **Step 1.2.1: 创建 streaming_tests.rs**

```rust
use llm_client::{AssistantMessageEvent, AssistantMessageEventStream, AssistantMessage, StopReason};

#[tokio::test]
async fn test_event_stream_push_next() {
    let (mut stream, tx) = AssistantMessageEventStream::new(32);
    let partial = AssistantMessage::default();
    
    tx.send(AssistantMessageEvent::Start { partial: partial.clone() }).await.unwrap();
    tx.send(AssistantMessageEvent::Done { reason: StopReason::Stop, message: partial }).await.unwrap();
    
    assert!(matches!(stream.next().await, Some(AssistantMessageEvent::Start { .. })));
    assert!(matches!(stream.next().await, Some(AssistantMessageEvent::Done { .. })));
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_event_stream_to_message_done() {
    let (stream, tx) = AssistantMessageEventStream::new(32);
    let msg = AssistantMessage::default();
    
    tx.send(AssistantMessageEvent::Done { reason: StopReason::Stop, message: msg.clone() }).await.unwrap();
    
    let result = stream.to_message().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_event_stream_to_message_error() {
    let (stream, tx) = AssistantMessageEventStream::new(32);
    let msg = AssistantMessage { stop_reason: StopReason::Error, error_message: Some("test".into()), ..Default::default() };
    
    tx.send(AssistantMessageEvent::Error { error: msg }).await.unwrap();
    
    let result = stream.to_message().await;
    assert!(result.is_err());
}
```

- [ ] **Step 1.2.2: 创建 retry_tests.rs**

```rust
use llm_client::{with_retry, LlmError};

#[tokio::test]
async fn test_retry_success_after_rate_limit() {
    let mut attempts = 0;
    let result = with_retry(
        || {
            attempts += 1;
            async move {
                if attempts < 3 {
                    Err(LlmError::RateLimited("test".into()))
                } else {
                    Ok(())
                }
            }
        },
        3,
        None,
    ).await;
    
    assert!(result.is_ok());
    assert_eq!(attempts, 3);
}

#[tokio::test]
async fn test_retry_exhausted() {
    let result = with_retry(
        || async { Err(LlmError::RateLimited("test".into())) },
        2,
        None,
    ).await;
    
    assert!(matches!(result, Err(LlmError::RateLimited(_))));
}
```

- [ ] **Step 1.2.3: 创建 overflow_tests.rs**

```rust
use llm_client::{is_context_overflow, StopReason};

#[test]
fn test_anthropic_prompt_too_long() {
    assert!(is_context_overflow(
        Some("prompt is too long: 213462 tokens > 200000"),
        &StopReason::Error,
        Some(200000),
        213462,
        0,
    ));
}

#[test]
fn test_non_overflow_rate_limit_excluded() {
    assert!(!is_context_overflow(
        Some("rate limit exceeded"),
        &StopReason::Error,
        None,
        0,
        0,
    ));
}
```

- [ ] **Step 1.2.4: 创建 models_tests.rs + compat_tests.rs**

```rust
// tests/models_tests.rs
use llm_client::{get_model, models_for_provider, providers, calculate_cost, supports_xhigh, models_are_equal};

#[test]
fn test_get_model_found() {
    let model = get_model("anthropic", "claude-sonnet-4-20250514");
    assert!(model.is_some());
}

#[test]
fn test_supports_xhigh_gpt5() {
    assert!(supports_xhigh("gpt-5.2"));
    assert!(!supports_xhigh("gpt-4.1"));
}

// tests/compat_tests.rs
use llm_client::{detect_openai_compat, ThinkingFormat, merge_openai_compat};

#[test]
fn test_detect_deepseek_compat() {
    let compat = detect_openai_compat("deepseek", "https://api.deepseek.com", "deepseek-chat");
    assert_eq!(compat.thinking_format, Some(ThinkingFormat::DeepSeek));
}
```

- [ ] **Step 1.2.5: 创建 validation_tests.rs + security_tests.rs**

```rust
// tests/validation_tests.rs
use llm_client::{validate_tool_call, ToolDef, ToolCall, ValidationError};

#[test]
fn test_coerce_string_to_number() {
    // "42" -> 42 coercion test
}

#[test]
fn test_error_message_format() {
    // Verify error message contains path, field name, and received args
}

// tests/security_tests.rs
use llm_client::LlmError;
use secrecy::SecretString;

#[test]
fn test_secret_string_debug_redacted() {
    let secret = SecretString::new("api-key-123".into());
    let debug = format!("{:?}", secret);
    assert!(!debug.contains("api-key-123"));
}
```

- [ ] **Step 1.2.6: 创建 repair_tests.rs + transform_tests.rs**

```rust
// tests/repair_tests.rs
use llm_client::{repair_json, StreamingJsonParser, parse_json_with_repair};

#[test]
fn test_repair_unclosed_string() {
    let result = repair_json("{\"key\":\"val");
    assert_eq!(result, "{\"key\":\"val\"}");
}

// tests/transform_tests.rs
use llm_client::{transform_messages, TransformOptions, Message, Content};

#[test]
fn test_image_merge_consecutive() {
    // 3 consecutive images -> 1 placeholder
}
```

- [ ] **Step 1.2.7: 创建 types_serde.rs**

```rust
use llm_client::{UserMessage, AssistantMessage, ToolResultMessage, Message, StopReason, ToolDef, Content};

#[test]
fn test_user_message_roundtrip() {
    let msg = UserMessage { /* ... */ };
    let json = serde_json::to_string(&msg).unwrap();
    let back: UserMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn test_content_thinking_variant() {
    let content = Content::Thinking { /* ... */ };
    let json = serde_json::to_string(&content).unwrap();
    assert!(json.contains("thinking"));
}
```

- [ ] **Step 1.2.8: 运行所有测试**

```bash
cargo test -p ai-provider
```

Expected: 87 (内联) + 9 (wiremock) + ~60 (集成) = ~156 passed

- [ ] **Step 1.2.11: Commit**

```bash
git add crates/ai-provider/tests/
git commit -m "test(ai-provider): migrate module tests to tests/ directory"
```

---

### Task 1.3: OpenAI thinking_format 多 provider 映射补全 (P1)

**Files:**
- Modify: `crates/ai-provider/src/providers/openai.rs:250-350`（reasoning 参数构建区域）

**背景:** 当前 OpenAI Provider 仅处理标准 OpenAI 的 `reasoning_effort`。Spec §9.2 要求支持 6 种 thinking_format。

**实现步骤:**

- [ ] **Step 1.3.1: 读取 model compat 的 thinking_format**

```rust
let compat = match get_model(provider_name, model) {
    Some(m) => match &m.compat {
        ModelCompat::OpenAI(c) => c.clone(),
        _ => OpenAiCompat::default(),
    },
    None => OpenAiCompat::default(),
};
```

- [ ] **Step 1.3.2: 根据 thinking_format 分支构造请求参数**

```rust
match compat.thinking_format {
    Some(ThinkingFormat::OpenAI) | None => {
        // 默认: reasoning_effort
        if let Some(level) = options.reasoning {
            let effort = match level {
                ReasoningLevel::Minimal | ReasoningLevel::Low => "low",
                ReasoningLevel::Medium => "medium",
                ReasoningLevel::High | ReasoningLevel::XHigh => "high",
            };
            body["reasoning_effort"] = json!(effort);
        }
    }
    Some(ThinkingFormat::OpenRouter) => {
        if let Some(level) = options.reasoning {
            let effort = match level { ... };
            body["reasoning"] = json!({ "effort": effort });
        }
    }
    Some(ThinkingFormat::DeepSeek) => {
        body["thinking"] = json!({ "type": "enabled" });
        if let Some(level) = options.reasoning {
            // DeepSeek 映射: minimal/low/medium/high → "high", xhigh → "max"
            let effort = if level == ReasoningLevel::XHigh { "max" } else { "high" };
            body["reasoning_effort"] = json!(effort);
        }
    }
    Some(ThinkingFormat::Zai) => {
        body["enable_thinking"] = json!(options.reasoning.is_some());
    }
    Some(ThinkingFormat::Qwen) => {
        body["enable_thinking"] = json!(options.reasoning.is_some());
    }
    Some(ThinkingFormat::QwenChatTemplate) => {
        body["chat_template_kwargs"] = json!({
            "enable_thinking": options.reasoning.is_some(),
            "preserve_thinking": true,
        });
    }
}
```

- [ ] **Step 1.3.3: XHigh clamp 检查**

```rust
// 计算 effective reasoning level，XHigh 在不支持的模型上降级为 High
let effective_reasoning = if options.reasoning == Some(ReasoningLevel::XHigh)
    && !supports_xhigh(model)
{
    Some(ReasoningLevel::High)
} else {
    options.reasoning
};

// 使用 effective_reasoning 而非 options.reasoning 构造 body
if let Some(level) = effective_reasoning {
    // ... 原来的 match 逻辑
}
```

- [ ] **Step 1.3.4: 运行测试**

```bash
cargo test -p ai-provider
cargo build -p agent-core
```

Expected: 编译通过，测试通过

- [ ] **Step 1.3.5: Commit**

```bash
git add crates/ai-provider/src/providers/openai.rs
git commit -m "feat(ai-provider): add thinking_format multi-provider mapping for OpenAI"
```

---

### Task 1.4: 文档与清理 (P1)

**Files:**
- Modify: `crates/ai-provider/README.md`
- Modify: `crates/ai-provider/src/transform.rs:1`（修复 unused import warning）

- [ ] **Step 1.4.1: 修复 compiler warning**

```rust
// crates/ai-provider/src/transform.rs:1
// 移除未使用的 ToolResultMessage import
use crate::types::{Api, AssistantMessage, Content, Message, StopReason, Usage};
```

- [ ] **Step 1.4.2: 运行 clippy**

```bash
cargo clippy -p ai-provider --all-features -- -D warnings
```

Expected: 零警告

- [ ] **Step 1.4.3: 更新 README**

在 `crates/ai-provider/README.md` 中：
- 补充新增模块说明（transform, models, overflow, validation, compat, cache, hooks）
- 更新公开接口表格
- 添加 Provider 实现状态表

- [ ] **Step 1.4.4: 运行完整测试套件**

```bash
cargo test -p ai-provider
cargo build -p agent-core
cargo test -p agent-core
```

Expected: 全部通过

- [ ] **Step 1.4.5: Commit**

```bash
git add crates/ai-provider/README.md crates/ai-provider/src/transform.rs
git commit -m "docs(ai-provider): update README and fix warnings"
```

---

## Phase 2: 扩展 Provider（P3 — 可选，建议核心模块完成后启动，~10h）

### Task 2.1: MistralProvider 实现 (P3)

**Files:**
- Create: `crates/ai-provider/src/providers/mistral.rs`
- Modify: `crates/ai-provider/src/providers/mod.rs`
- Modify: `crates/ai-provider/src/lib.rs`

**Spec:** §9.4

**核心差异:**
- SSE 格式与 OpenAI Completions 同构，可复用部分解析逻辑
- Tool call ID 截断：`short_hash()` 限制 ≤36 chars
- Reasoning: `promptMode: "reasoning"` + `reasoningEffort`

- [ ] **Step 2.1.1: 定义 MistralProvider 结构**

```rust
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;
use crate::{LlmProvider, LlmError, AssistantMessageEventStream, LlmContext, StreamOptions};

pub struct MistralProvider {
    client: Client,
    api_key: String,
}

impl MistralProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
        }
    }
}

#[async_trait]
impl LlmProvider for MistralProvider {
    fn provider_name(&self) -> &str { "mistral" }
    
    fn models(&self) -> Vec<String> {
        vec!["mistral-large-latest".into(), "mistral-medium-latest".into()]
    }
    
    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        let (stream, tx) = AssistantMessageEventStream::new(32);
        
        tokio::spawn(async move {
            let result = try_stream(self.client.clone(), model, context, options, &tx, signal).await;
            if let Err(e) = result {
                let _ = tx.send(AssistantMessageEvent::Error { 
                    error: error_to_assistant_message(e) 
                }).await;
            }
        });
        
        Ok(stream)
    }
}
```

- [ ] **Step 2.1.2: 实现消息转换**

Mistral 使用 OpenAI 兼容的消息格式，可直接复用 OpenAI 的转换逻辑。

- [ ] **Step 2.1.3: 实现 SSE 解析**

复用 OpenAI 的 SSE 解析逻辑（`choice.delta.content`、`choice.delta.tool_calls` 等）。

- [ ] **Step 2.1.4: Tool call ID 截断**

```rust
fn truncate_tool_call_id(id: &str) -> String {
    if id.len() <= 36 {
        id.to_string()
    } else {
        let hash = crate::transform::short_hash(id);
        format!("call_{}{}", hash, &id[id.len().saturating_sub(8)..])
    }
}
```

- [ ] **Step 2.1.5: Reasoning 参数**

```rust
if let Some(level) = options.reasoning {
    body["promptMode"] = json!("reasoning");
    let effort = match level {
        ReasoningLevel::Minimal | ReasoningLevel::Low => "low",
        ReasoningLevel::Medium => "medium",
        ReasoningLevel::High | ReasoningLevel::XHigh => "high",
    };
    body["reasoningEffort"] = json!(effort);
}
```

- [ ] **Step 2.1.6: 注册 Provider**

```rust
// crates/ai-provider/src/providers/mod.rs
pub mod anthropic;
pub mod google;
pub mod mistral;
pub mod openai;

// crates/ai-provider/src/lib.rs
pub use providers::mistral::MistralProvider;
```

- [ ] **Step 2.1.7: 测试**

```bash
cargo test -p ai-provider
cargo build -p agent-core
```

- [ ] **Step 2.1.8: Commit**

```bash
git add crates/ai-provider/src/providers/mistral.rs crates/ai-provider/src/providers/mod.rs crates/ai-provider/src/lib.rs
git commit -m "feat(ai-provider): add MistralProvider"
```

---

### Task 2.2: AwsBedrockProvider 实现 (P3 — 时间风险高，建议拆分为子任务)

**Files:**
- Create: `crates/ai-provider/src/providers/bedrock.rs`
- Modify: `crates/ai-provider/src/providers/mod.rs`
- Modify: `crates/ai-provider/src/lib.rs`
- Modify: `crates/ai-provider/Cargo.toml`

**Spec:** §9.5

**依赖:** `aws-sdk-bedrockruntime = { version = "1", optional = true }`

**Feature gate:**
```toml
[features]
bedrock = ["aws-sdk-bedrockruntime"]
```

- [ ] **Step 2.2.1: 添加可选依赖**

```toml
# crates/ai-provider/Cargo.toml
[dependencies]
# ... existing ...
aws-sdk-bedrockruntime = { version = "1", optional = true }

[features]
bedrock = ["aws-sdk-bedrockruntime"]
test-utils = []
```

- [ ] **Step 2.2.2: 定义 AwsBedrockProvider 结构**

```rust
#[cfg(feature = "bedrock")]
pub mod bedrock;

#[cfg(feature = "bedrock")]
use aws_sdk_bedrockruntime::Client as BedrockClient;

pub struct AwsBedrockProvider {
    client: BedrockClient,
    region: String,
}
```

- [ ] **Step 2.2.3: 实现 LlmProvider trait**

使用 `aws_sdk_bedrockruntime::Client::converse_stream()` 方法。

- [ ] **Step 2.2.4: 消息转换**

Bedrock 使用 Anthropic 格式的消息子集：
- `system` → `system: [{ text }]`
- `user` → `messages: [{ role: "user", content: [{ text }, { image }] }]`
- `assistant` → `messages: [{ role: "assistant", content: [{ text }, { toolUse }] }]`
- `tool_result` → `messages: [{ role: "user", content: [{ toolResult }] }]`

- [ ] **Step 2.2.5: ConverseStream 事件映射**

```rust
// Bedrock Stream 事件 → AssistantMessageEvent
match event {
    ContentBlockStart { .. } => TextStart/ToolCallStart,
    ContentBlockDelta { text, .. } => TextDelta,
    ContentBlockStop { .. } => TextEnd/ToolCallEnd,
    MessageStop { stop_reason } => Done,
    // ...
}
```

- [ ] **Step 2.2.6: Cache 支持**

```rust
// CachePoint + CacheTTL 标记
if options.cache_retention != CacheRetention::None {
    // 在 system prompt 和最后一条 assistant message 附加 cache 标记
}
```

- [ ] **Step 2.2.7: Reasoning 支持**

```rust
// 自适应推理
if options.reasoning.is_some() {
    body["performanceConfig"] = json!({
        "adaptiveThinking": { "display": "summarized" }
    });
}

// 预算推理
body["reasoningConfig"] = json!({
    "reasoningType": "enabled",
    "budgetTokens": thinking_budget,
    "display": "summarized",
});
```

- [ ] **Step 2.2.8: 注册 Provider**

```rust
// crates/ai-provider/src/providers/mod.rs
#[cfg(feature = "bedrock")]
pub mod bedrock;

// crates/ai-provider/src/lib.rs
#[cfg(feature = "bedrock")]
pub use providers::bedrock::AwsBedrockProvider;
```

- [ ] **Step 2.2.9: 测试（带 feature gate）**

```bash
cargo test -p ai-provider --features bedrock
cargo build -p ai-provider --all-features
```

- [ ] **Step 2.2.10: Commit**

```bash
git add crates/ai-provider/src/providers/bedrock.rs crates/ai-provider/src/providers/mod.rs crates/ai-provider/src/lib.rs crates/ai-provider/Cargo.toml
git commit -m "feat(ai-provider): add AwsBedrockProvider (optional feature)"
```

---

## Phase 3: OAuth Provider 支持（P3 — 可选，~2h）

### Task 3.1: OAuth 抽象层 (P3)

**Files:**
- Create: `crates/ai-provider/src/oauth.rs`
- Modify: `crates/ai-provider/src/lib.rs`

**Spec:** §26

**v0.1 范围:** 
1. 定义 OAuthToken + OAuthProvider trait 接口
2. 修改现有 Provider（Anthropic/OpenAI/Google）支持 OAuth token 作为 API key 来源
3. 不做具体的 OAuth 流程实现（Browser OAuth / Device code 等留到后续版本）

**关键设计决策:**
- `resolve_api_key()` 保持同步不变，OAuth token 解析在 `stream()` 中独立处理
- OAuth refresh 失败（如网络问题）不阻塞 stream，降级到下一个 key 来源

- [ ] **Step 3.1.1: 定义 OAuthToken + is_expired 辅助函数**

```rust
use secrecy::SecretString;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct OAuthToken {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_at: Option<SystemTime>,
    pub scopes: Vec<String>,
}

fn is_expired(token: &OAuthToken) -> bool {
    match token.expires_at {
        Some(expiry) => std::time::SystemTime::now() >= expiry,
        None => false,
    }
}
```

**要求:** `Debug` impl 必须 redact `access_token` 和 `refresh_token`（使用 `secrecy::SecretString` 的默认行为即可）。

- [ ] **Step 3.1.2: 定义 OAuthProvider trait**

```rust
use async_trait::async_trait;

#[async_trait]
pub trait OAuthProvider: Send + Sync {
    fn provider_name(&self) -> &str;
    async fn login(&self) -> Result<OAuthToken, std::io::Error>;
    async fn refresh(&self, token: &OAuthToken) -> Result<OAuthToken, std::io::Error>;
    fn load_token(&self) -> Option<OAuthToken>;
    fn save_token(&self, token: &OAuthToken) -> std::io::Result<()>;
}
```

- [ ] **Step 3.1.3: 添加到 lib.rs**

```rust
pub mod oauth;
pub use oauth::{OAuthToken, OAuthProvider};
```

- [ ] **Step 3.1.4: 修改 AnthropicProvider 支持 OAuth**

修改 `AnthropicProvider` 结构，增加 `oauth_provider` 字段：

```rust
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: Option<SecretString>,
    base_url: String,
    oauth_provider: Option<Arc<dyn OAuthProvider>>,
}

impl AnthropicProvider {
    pub fn with_oauth(mut self, oauth: Arc<dyn OAuthProvider>) -> Self {
        self.oauth_provider = Some(oauth);
        self
    }
}
```

**保持 `resolve_api_key()` 同步不变**，OAuth token 在 `stream()` 中独立处理：

```rust
async fn stream(
    &self,
    model: &str,
    context: LlmContext,
    options: StreamOptions,
    signal: CancellationToken,
) -> Result<AssistantMessageEventStream, LlmError> {
    // 优先尝试 OAuth token（如果配置了 OAuthProvider）
    let api_key = if let Some(oauth) = &self.oauth_provider {
        if let Some(token) = oauth.load_token() {
            let token = if is_expired(&token) {
                oauth.refresh(&token).await.ok()
            } else {
                Some(token)
            };
            token.map(|t| t.access_token)
        } else {
            None
        }
    } else {
        None
    };
    
    // OAuth 失败时降级到原来的 resolve_api_key
    let api_key = match api_key {
        Some(key) => key,
        None => self.resolve_api_key(&options)?,
    };
    
    // ... rest of stream implementation
}
```

**注意:** OAuth refresh 失败时静默降级到下一个 key 来源（env var / error），不阻塞 stream。

- [ ] **Step 3.1.5: 修改 OpenAiProvider 和 GoogleProvider**

对 OpenAiProvider 和 GoogleProvider 执行同样的修改：
1. 添加 `oauth_provider: Option<Arc<dyn OAuthProvider>>` 字段
2. 提供 `with_oauth()` builder 方法
3. 修改 `resolve_api_key()` 支持 OAuth token（注意：OpenAI 的环境变量是 `OPENAI_API_KEY`，Google 是 `GOOGLE_API_KEY`）

- [ ] **Step 3.1.6: 创建 tests/oauth_tests.rs**

```rust
use llm_client::{OAuthToken, OAuthProvider};
use secrecy::SecretString;

#[test]
fn test_oauth_token_debug_redacted() {
    let token = OAuthToken {
        access_token: SecretString::new("secret_access_token".into()),
        refresh_token: Some(SecretString::new("secret_refresh_token".into())),
        expires_at: None,
        scopes: vec![],
    };
    
    let debug = format!("{:?}", token);
    assert!(!debug.contains("secret_access_token"));
    assert!(!debug.contains("secret_refresh_token"));
}

#[test]
fn test_oauth_token_load_save_roundtrip() {
    // 测试 OAuthToken 的持久化/反序列化
}
```

- [ ] **Step 3.1.7: Commit**

```bash
git add crates/ai-provider/src/oauth.rs crates/ai-provider/src/lib.rs crates/ai-provider/src/providers/anthropic.rs crates/ai-provider/src/providers/openai.rs crates/ai-provider/src/providers/google.rs crates/ai-provider/tests/oauth_tests.rs
git commit -m "feat(ai-provider): add OAuth abstraction layer and integrate with providers"
```

---

## 验证检查点

### 每 Task 完成后执行：

```bash
# 编译检查
cargo build -p ai-provider --all-features

# 测试检查
cargo test -p ai-provider

# Lint 检查
cargo clippy -p ai-provider --all-features -- -D warnings
```

### Phase 1 完成后的最终验证：

```bash
# agent-core 零破坏检查
cargo build -p agent-core
cargo test -p agent-core

# 全量测试
cargo test -p ai-provider --all-features
```

### 预期最终状态：

- [ ] ai-provider: ~150+ 测试全部通过
- [ ] agent-core: 编译通过，测试通过
- [ ] clippy: 零警告
- [ ] 文档: README 更新完成

---

## 时间估算与优先级

| Phase | Task | 内容 | 预估时间 | 优先级 | 可并行性 |
|-------|------|------|---------|--------|---------|
| P1 | 1.1 | wiremock HTTP mock 测试 | 3h | **P1** | 全程可并行 |
| P1 | 1.2 | 模块级集成测试迁移（含 types_serde.rs） | 2.5h | **P1** | 全程可并行 |
| P1 | 1.3 | thinking_format 补全 | 1h | **P1** | 全程可并行 |
| P1 | 1.4 | 文档 + 清理 | 1h | **P1** | 全程可并行 |
| **P1 小计** | | | **7.5h** | | |
| P3 | 2.1 | MistralProvider | 4h | **P3** | 建议核心模块完成后 |
| P3 | 2.2 | AwsBedrockProvider | 6h → **10h** | **P3** | 建议核心模块完成后 |
| P3 | 3.1 | OAuth 抽象层 | 2h | **P3** | 建议核心模块完成后 |
| **P3 小计** | | | **16h** | | |
| **总计** | | | **23.5h** | | |

**P1 建议**: 在 agent-core Phase 0 开始的同时启动，利用 agent-core 开发的时间窗口完成测试补全。
**P3 建议**: 在所有核心 crate（agent-core + extensions）P0 完成后启动，避免并行维护多个大型变更集。
**Bedrock 风险**: 原预估 6h 可能不足（AWS SDK 集成 + SigV4 + ConverseStream），建议预留 10h 或拆分为两个里程碑。

---

## 与 v0.1 计划的关系

- **v0.1 计划** (`2026-05-02-ai-provider-implementation.md`): 已完成 ✅
  - Phase 0-8 全部落地（核心类型、StreamOptions、Provider 实现、工具模块、模型注册表、测试）
  - 见旧计划顶部的"已完成"标记

- **v0.2 计划** (本文档): 增量补全
  - 测试覆盖度提升（wiremock + 模块测试迁移 + types_serde）
  - P3 Provider 扩展（Mistral、Bedrock）
  - OAuth 抽象层 + Provider 集成（v0.1 预留的扩展点）

## 本计划更新记录

| 更新 | 内容 | 原因 |
|------|------|------|
| 2026-05-03 | 初始版本 | 基于 spec review，聚焦 v0.1 剩余工作 |
| 2026-05-03 | 添加 types_serde.rs | Spec §11.1 遗漏，覆盖 Content/Message/ToolDef 序列化测试 |
| 2026-05-03 | Task 1.2 明确"迁移+新增"策略 | 避免仅复制内联测试而遗漏 spec 额外用例 |
| 2026-05-03 | Task 3.1 扩展为"trait + Provider 集成" | Spec §26.4 要求修改现有 Provider 的 `resolve_api_key()` |
