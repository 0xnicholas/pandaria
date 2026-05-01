# llm-client

LLM provider 抽象层。定义消息类型、Tool 定义、LlmProvider trait 及流式 SSE 事件类型。

## 职责

- 定义与 LLM 交互的通用消息协议（`UserMessage`、`AssistantMessage`、`ToolResultMessage`）
- 定义 `Content` enum 统一表示文本、图片、思考、ToolCall 等内容块
- 定义 `ToolDef` 和 `ToolCall` 类型
- 定义 `LlmProvider` trait：provider 接口 + 流式响应抽象
- 定义 `LlmError` 错误枚举（rate limit、超时、取消等）
- 定义 `AssistantMessageEventStream`：SSE 流式事件类型（`Start`、`TextDelta`、`ToolCallDelta`、`Done`、`Error`）

## 公开接口

| 模块 | 核心导出 |
|---|---|
| `types` | `Message`、`Content`、`ToolCall`、`ToolDef`、`UserMessage`、`AssistantMessage`、`ToolResultMessage`、`StopReason`、`LlmContext`、`Api`、`Usage` |
| `error` | `LlmError` |
| `provider` | `LlmProvider` trait、`StreamOptions` |
| `streaming` | `AssistantMessageEvent`、`AssistantMessageEventStream` |

## 边界

- **不实现**具体的 HTTP 调用（由上层或 provider 实现方处理）
- **不实现**指数退避重试（由上层 `LlmClient` 封装层处理）
- 所有类型支持 `serde` 序列化/反序列化

## 依赖

- `serde` / `serde_json` — JSON 序列化
- `async-trait` — async trait 支持
- `thiserror` — 错误类型
- `futures` — Stream trait
- `tokio-util` — `CancellationToken`
