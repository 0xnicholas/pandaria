# llm-client

LLM provider 抽象层。定义消息类型、Tool 定义、LlmProvider trait 及流式 SSE 事件类型。

Note: This library only includes models that support tool calling (function calling), as this is essential for agentic workflows.

## 职责

- 定义与 LLM 交互的通用消息协议（`UserMessage`、`AssistantMessage`、`ToolResultMessage`）
- 定义 `Content` enum 统一表示文本、图片、思考、ToolCall 等内容块
- 定义 `ToolDef` 和 `ToolCall` 类型
- 定义 `LlmProvider` trait：provider 接口 + 流式响应抽象
- 定义 `LlmError` 错误枚举（rate limit、超时、取消等）
- 定义 `AssistantMessageEventStream`：SSE 流式事件类型（`Start`、`TextDelta`、`ToolCallDelta`、`Done`、`Error`）
- 模型注册表（`ModelRegistry`）和兼容性检测（`OpenAiCompat` / `AnthropicCompat`）
- 工具参数验证与类型强制转换（`validate_tool_call`）
- 上下文溢出检测（`is_context_overflow`）
- JSON 修复与流式解析（`repair_json`、`StreamingJsonParser`）
- 消息跨 Provider 转换（`transform_messages`）
- 指数退避重试（`with_retry`）

## 公开接口

| 模块 | 核心导出 |
|---|---|
| `types` | `Message`、`Content`、`ToolCall`、`ToolDef`、`UserMessage`、`AssistantMessage`、`ToolResultMessage`、`StopReason`、`LlmContext`、`Api`、`Usage` |
| `error` | `LlmError` |
| `provider` | `LlmProvider` trait、`StreamOptions`、`ReasoningLevel`、`ThinkingBudgets` |
| `streaming` | `AssistantMessageEvent`、`AssistantMessageEventStream` |
| `models` | `Model`、`ModelRegistry`、`get_model`、`models_for_provider`、`providers`、`calculate_cost`、`supports_xhigh` |
| `compat` | `OpenAiCompat`、`AnthropicCompat`、`ThinkingFormat`、`detect_openai_compat`、`merge_openai_compat` |
| `validation` | `validate_tool_call`、`validate_tool_arguments`、`ValidationError` |
| `overflow` | `is_context_overflow` |
| `repair` | `repair_json`、`parse_json_with_repair`、`StreamingJsonParser`、`sanitize_unicode` |
| `transform` | `transform_messages`、`TransformOptions` |
| `retry` | `with_retry` |
| `cache` | `CacheRetention` |
| `hooks` | `OnPayloadFn`、`OnResponseFn`、`ProviderResponse` |

## Provider 实现状态

| Provider | 文件 | 状态 | 特性 |
|---|---|---|---|
| Anthropic | `providers/anthropic.rs` | 完整 | SSE + cache_control + thinking + beta headers |
| OpenAI | `providers/openai.rs` | 完整 | SSE + reasoning + prompt_cache_key + thinking_format 映射 |
| Google | `providers/google.rs` | 完整 | JSON stream + x-goog-api-key |
| Mistral | `providers/mistral.rs` | 待实现 | — |
| AWS Bedrock | `providers/bedrock.rs` | 待实现 | 可选 feature `bedrock` |

## 边界

- **不实现**具体的 OAuth 流程（Browser OAuth / Device code 等留到后续版本）
- 所有类型支持 `serde` 序列化/反序列化
- 所有公开 API 均有文档注释

## 依赖

- `serde` / `serde_json` — JSON 序列化
- `async-trait` — async trait 支持
- `thiserror` — 错误类型
- `futures` — Stream trait
- `tokio-util` — `CancellationToken`
- `reqwest` — HTTP 客户端
- `eventsource-stream` — SSE 解析
- `secrecy` — 密钥安全处理
- `regex` — 正则匹配
- `jsonschema` — JSON Schema 验证

## 测试

```bash
cargo test -p llm-client
cargo clippy -p llm-client --all-features -- -D warnings
```

当前测试覆盖：~170 个测试全部通过（87 单元测试 + 83 集成测试）。
