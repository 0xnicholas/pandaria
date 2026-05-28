# 多模态模型接入实施计划

> **Status:** Completed ✅ — 理解型多模态 + 生成型多模态已交付
> **优先级: P1** — 非阻塞级，可在现有功能迭代中并行推进。
> **目标:** 分阶段实现理解型多模态（Video/Audio 输入）和生成型多模态（MediaProvider + MediaGenerationTool）。
> **Spec Reference:** `docs/specs/2026-05-19-multimodal-support.md`

---

## 当前状态

- `Content` 已有 `Image` 输入变体，`openai_compatible_stream` 已支持 `image_url`。
- `AgentLoop` 透传 `Vec<Content>`，`ToolExecutor` 支持 `Content::Image` 返回。
- **缺失:** `Video`/`Audio` 输入变体、`MediaProvider` trait、生成型多模态工具、`transform` 降级逻辑。

---

## 开发顺序

```
Phase 0 (基础设施调整)
  ├── Task 0.1: AgentTool::execute 签名扩展 CancellationToken
  ├── Task 0.2: 创建独立 MediaModelRegistry
  └── Task 0.3: 消息协议结构化（PR-2）

Phase 1 (理解型多模态基础)
  ├── Task 1.1: Content/Modality 扩展
  ├── Task 1.2: openai_compatible_stream Video/Audio 序列化
  ├── Task 1.3: transform 降级 + 兼容处理
  ├── Task 1.4: TUI HistoricalContent 适配
  ├── Task 1.5: API Gateway 默认降级
  ├── Task 1.6: AgentSpace::media_dir 扩展
  └── Task 1.7: SSE ToolResult 文本提取改进

Phase 2 (生成型多模态核心)
  ├── Task 2.1: MediaProvider trait + 类型系统
  ├── Task 2.2: define_media_provider! 宏 + media_shared.rs
  ├── Task 2.3: DoubaoMediaProvider（Seedream 图片生成）
  └── Task 2.4: MediaGenerationTool（AgentTool 实现）

Phase 2.5 (生成型多模态进阶)
  └── Task 2.5: DoubaoMediaProvider（Seedance 视频生成 + 异步轮询）

Phase 3 ( polish )
  ├── Task 3.1: Media 模型注册表填充
  ├── Task 3.2: 集成测试
  ├── Task 3.3: 成本计量集成（on_tool_result + CostTracker）
  ├── Task 3.4: AGENTS.md 更新
  └── Task 3.5: 补充单元测试覆盖
```

---

## Phase 0: 基础设施调整

### Task 0.1: AgentTool::execute 签名扩展 CancellationToken

**Files:**
- Modify: `crates/agent-core/src/types.rs`
- Modify: `crates/agent-core/src/harness/tool.rs`
- Modify: `crates/agent-core/src/harness/agent_loop.rs`
- Modify: 所有现有 `AgentTool` 实现及测试中的 mock tool（机械改动，内部逻辑不变）

**Steps:**

- [ ] **Step 1: 扩展 `AgentTool` trait 签名**

```rust
async fn execute(
    &self,
    tool_call_id: &str,
    params: serde_json::Value,
    on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    signal: CancellationToken,  // 新增
) -> Result<AgentToolResult, AgentError>;
```

- [ ] **Step 2: 同步修改所有现有 tool 实现**

所有 tool 的 `execute` 方法增加 `_signal: CancellationToken` 参数。需要修改的文件和 tool：

**集成测试：**
- `crates/agent-core/tests/e2e_tool_use_tests.rs`：`EchoTool`

**单元测试（`tool.rs`）：**
- `MockTool`
- `ProgressTool`
- `ErrorTool`
- `TerminateTool`
- `PanicTool`

**单元测试（`agent_loop.rs`）：**
- `CounterTool`
- `SequentialCounterTool`
- `TerminatingTool`
- `NonTerminatingTool`

**其他文件：**
- `crates/agent-core/src/file_ops.rs`：检查是否有 `AgentTool` 实现（如 `FileReadTool`、`FileWriteTool` 等）

- [ ] **Step 3: `ToolExecutor` 透传 CancellationToken**

修改 `crates/agent-core/src/harness/tool.rs`：

```rust
pub(crate) async fn execute_tool_call(
    &self,
    tool_call: &ToolCall,
    on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    signal: CancellationToken,  // 新增
) -> Result<ToolResultMsg, AgentError> {
    // ...
    let mut result = catch_panic(
        self.tool.execute(&tool_call.id, tool_input, on_progress, signal)
    ).await??;
    // ...
}
```

- [ ] **Step 4: `AgentLoop` 透传 CancellationToken**

修改 `crates/agent-core/src/harness/agent_loop.rs`：

```rust
async fn execute_single_tool(
    &self,
    tc: &ai_provider::ToolCall,
    signal: CancellationToken,  // 新增
) -> ai_provider::ToolResultMessage {
    // ...
    let result = executor.execute_tool_call(
        tc,
        Some(&move |update: crate::types::AgentToolProgressUpdate| { ... }),
        signal,  // 透传
    ).await;
    // ...
}
```

`execute_tools` 中调用 `execute_single_tool` 时传入 `signal.clone()`：

```rust
// 顺序执行分支（保留 signal.is_cancelled() 循环守卫，同时透传给工具内部）
for tc in tool_calls {
    if signal.is_cancelled() {
        break;
    }
    results.push(self.execute_single_tool(tc, signal.clone()).await);
}

// 并行执行分支
let tasks: Vec<_> = tool_calls.iter().map(|tc| {
    let signal = signal.clone();
    async move {
        tokio::select! {
            result = self.execute_single_tool(tc, signal.clone()) => result,
            _ = signal.cancelled() => {
                ai_provider::ToolResultMessage {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    content: vec![],
                    details: Some(serde_json::json!({"cancelled": true})),
                    is_error: true,
                    timestamp: std::time::SystemTime::now(),
                }
            }
        }
    }
}).collect();
```

> **策略说明**：顺序执行分支保留 `signal.is_cancelled()` 作为循环守卫（快速路径），同时通过 `execute_single_tool` 将 signal 透传给 `ToolExecutor::execute_tool_call`，最终到达工具内部。对于长时间运行的工具（如内部含 HTTP 轮询的自定义工具），工具内部可通过 `signal.cancelled()` 主动中断。对于同步 API（如 Seedream），CancellationToken 作用有限，但透传不会带来副作用。

- [ ] **Step 5: 编译验证**

```bash
cargo check -p agent-core
cargo test -p agent-core --lib
cargo test -p agent-core --test e2e_tool_use_tests
```

Expected: PASS

---

### Task 0.2: 创建独立 MediaModelRegistry

**Files:**
- Create: `crates/ai-provider/src/media/mod.rs`（最小骨架，仅包含 registry 导出）
- Create: `crates/ai-provider/src/media/registry.rs`
- Create: `crates/ai-provider/src/media/task.rs`（Phase 0 提前定义 `MediaTaskType`，避免后续返工）

**Steps:**

- [ ] **Step 1: 创建 `media/task.rs`（MediaTaskType 提前定义）**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaTaskType {
    ImageGeneration,
    VideoGeneration,
    AudioGeneration,
}

/// 将字符串映射为 MediaTaskType（供 MediaGenerationTool 参数解析使用）。
pub fn media_task_type_from_str(s: &str) -> Result<MediaTaskType, MediaError> {
    match s {
        "image" => Ok(MediaTaskType::ImageGeneration),
        "video" => Ok(MediaTaskType::VideoGeneration),
        "audio" => Ok(MediaTaskType::AudioGeneration),
        _ => Err(MediaError::UnsupportedTask(s.to_string())),
    }
}
```

> `MediaError` 先在 `media/error.rs` 中定义最小版本（仅 `UnsupportedTask`），Phase 2.1 再扩展其余变体。

- [ ] **Step 2: 创建 `media/mod.rs` 最小骨架**

```rust
pub mod error;
pub mod registry;
pub mod task;

pub use error::MediaError;
pub use registry::{MediaModel, MediaModelRegistry};
pub use task::{MediaTaskType, media_task_type_from_str};
```

- [ ] **Step 3: 创建 `media/error.rs`（最小版本）**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("unsupported task type: {0}")]
    UnsupportedTask(String),
}
```

- [ ] **Step 4: 创建 `MediaModel` struct（直接使用 `MediaTaskType`）**

在 `registry.rs` 中：

```rust
use std::collections::HashMap;
use crate::media::MediaTaskType;

#[derive(Debug, Clone)]
pub struct MediaModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub base_url: String,
    pub supported_tasks: Vec<MediaTaskType>,
    pub cost_per_call: Option<f64>,
    pub headers: Option<HashMap<String, String>>,
}

pub struct MediaModelRegistry {
    models: HashMap<String, MediaModel>,
}

impl MediaModelRegistry {
    pub fn new() -> Self {
        Self { models: HashMap::new() }
    }
    pub fn insert(&mut self, model: MediaModel) {
        self.models.insert(model.id.clone(), model);
    }
    pub fn get(&self, model_id: &str) -> Option<&MediaModel> {
        self.models.get(model_id)
    }
    pub fn models_for_provider(&self, provider: &str) -> Vec<&MediaModel> {
        self.models.values().filter(|m| m.provider == provider).collect()
    }
}
```

> `MediaTaskType` 在 Phase 0 直接定义，避免 Phase 2.1 时返工修改 registry、`build_default()`、`resolve_model` 等多处代码。

- [ ] **Step 5: `lib.rs` 导出 media 模块**

```rust
pub mod media;
```

- [ ] **Step 6: 编译验证**

```bash
cargo check -p ai-provider
```

Expected: PASS

---

### Task 0.3: 消息协议结构化（PR-2）

> **Spec Reference:** 前置条件第 2 条

**Files:**
- Modify: `crates/api-gateway/src/types.rs`
- Modify: `crates/api-gateway/src/routes/messages.rs`
- Modify: `crates/tenant/src/manager.rs`
- Modify: `crates/agent-core/src/harness/session.rs`
- Modify: `crates/tui/src/client/model.rs`
- Modify: `crates/tui/src/client/rest.rs`
- Modify: `crates/api-gateway/tests/common/mod.rs`

**Steps:**

- [ ] **Step 1: `api-gateway` 定义 `MessageContentPart`**

```rust
// crates/api-gateway/src/types.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContentPart {
    Text { text: String },
    Image { data: String, mime_type: String },
    Video { data: String, mime_type: String },
    Audio { data: String, mime_type: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: Vec<MessageContentPart>,
}
```

- [ ] **Step 2: `tenant` 转换逻辑 + `send_message` 签名更新**

```rust
// tenant/src/manager.rs
#[async_trait]
pub trait TenantManager: Send + Sync {
    async fn send_message(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        content: Vec<ai_provider::Content>,
    ) -> Result<u64, TenantError>;
    // ...
}

fn convert_content(parts: Vec<MessageContentPart>) -> Vec<ai_provider::Content> {
    parts.into_iter().map(|p| match p {
        MessageContentPart::Text { text } => ai_provider::Content::Text { text, text_signature: None },
        MessageContentPart::Image { data, mime_type } => ai_provider::Content::Image { data, mime_type },
        MessageContentPart::Video { data, mime_type } => ai_provider::Content::Video { data, mime_type },
        MessageContentPart::Audio { data, mime_type } => ai_provider::Content::Audio { data, mime_type },
    }).collect()
}
```

在 `TenantManagerImpl::send_message` 中调用转换：

```rust
// tenant/src/manager.rs
impl TenantManagerImpl {
    pub async fn send_message(
        &self,
        tenant_id: &str,
        session_id: &Uuid,
        content: Vec<ai_provider::Content>,
    ) -> Result<u64, TenantError> {
        // ... 原有逻辑，将 String content 改为 Vec<Content> ...
        let session = self.get_session(tenant_id, session_id).await?;
        let messages = session.prompt_with_content(content).await?;
        // ...
    }
}
```

> 若原有实现中 `TenantManagerImpl::send_message` 直接调用 `session.prompt(text)`，需改为 `session.prompt_with_content(content)`。原有 `prompt(String)` 保留作为向后兼容的快捷方法。

- [ ] **Step 3: `SessionActor` 新增 `prompt_with_content`**

```rust
// agent-core/src/harness/session.rs
impl SessionActor {
    pub async fn prompt(&mut self, text: String) -> Result<Vec<AgentMessage>, AgentError> {
        self.prompt_with_content(vec![Content::Text { text, text_signature: None }]).await
    }

    pub async fn prompt_with_content(
        &mut self,
        content: Vec<Content>,
    ) -> Result<Vec<AgentMessage>, AgentError> {
        // 原 prompt 逻辑迁移至此
        // ...
    }
}
```

- [ ] **Step 4: `api-gateway` `send()` handler 更新**

`api-gateway/src/routes/messages.rs` 的 `send()` handler 需将 `SendMessageRequest` 中的 `Vec<MessageContentPart>` 转换为 `Vec<ai_provider::Content>` 后传给 `TenantManager::send_message`：

```rust
// crates/api-gateway/src/routes/messages.rs
pub async fn send(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, GatewayError> {
    let content = convert_content(req.content);
    let message_id = state.tenant_manager.send_message(&tenant_id.0, &id, content).await?;
    Ok(Json(SendMessageResponse { message_id }))
}
```

> `convert_content` 可内联在 `messages.rs` 中，或放在 `api-gateway/src/types.rs` 作为 `MessageContentPart` 的 `impl` 方法。

- [ ] **Step 5: `tui` 和测试 mock 同步更新**

- `tui/src/client/model.rs`: `SendMessageRequest` 类型定义改为 `Vec<MessageContentPart>`
- `tui/src/client/rest.rs`: 构造 `SendMessageRequest` 时传入 `Vec<MessageContentPart>`
- `api-gateway/tests/common/mod.rs`: mock `TenantManager::send_message` 签名适配

- [ ] **Step 6: 编译验证**

```bash
cargo check -p api-gateway -p tenant -p agent-core -p tui
```

Expected: PASS

---

## Phase 1: 理解型多模态基础

### Task 1.1: Content / Modality 扩展

**Files:**
- Modify: `crates/ai-provider/src/types.rs`
- Modify: `crates/ai-provider/src/models.rs`
- Modify: `crates/ai-provider/src/models_data.rs`

**Steps:**

- [ ] **Step 1: Modality 扩展**

在 `models.rs` 的 `Modality` enum 中新增 `Video` / `Audio`（`Modality` 定义在 `models.rs` 而非 `types.rs`）：

```rust
pub enum Modality {
    Text,
    Image,
    Video,   // 新增
    Audio,   // 新增
}
```

- [ ] **Step 2: Content 扩展 Video / Audio 变体**

```rust
pub enum Content {
    // ... existing Text, Image, Thinking, ToolCall ...

    #[serde(rename = "video")]
    Video {
        data: String,
        mime_type: String,
    },

    #[serde(rename = "audio")]
    Audio {
        data: String,
        mime_type: String,
    },
}
```

- [ ] **Step 3: 兼容性兜底（serde 未知变体）**

移除 `Content` 的 `#[derive(Deserialize)]`，手写 `Deserialize` impl；`Serialize` 继续 derive 保留。

```rust
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum Content {
    // ... variants ...
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let ty = value.get("type").and_then(|t| t.as_str());
        match ty {
            Some("text") => {
                let text = value.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let sig = value.get("text_signature").and_then(|s| s.as_str()).map(|s| s.to_string());
                Ok(Content::Text { text, text_signature: sig })
            }
            Some("image") => {
                let data = value.get("data").and_then(|d| d.as_str()).unwrap_or("").to_string();
                let mime_type = value.get("mime_type").and_then(|m| m.as_str()).unwrap_or("image/png").to_string();
                Ok(Content::Image { data, mime_type })
            }
            Some("video") => {
                let data = value.get("data").and_then(|d| d.as_str()).unwrap_or("").to_string();
                let mime_type = value.get("mime_type").and_then(|m| m.as_str()).unwrap_or("video/mp4").to_string();
                Ok(Content::Video { data, mime_type })
            }
            Some("audio") => {
                let data = value.get("data").and_then(|d| d.as_str()).unwrap_or("").to_string();
                let mime_type = value.get("mime_type").and_then(|m| m.as_str()).unwrap_or("audio/wav").to_string();
                Ok(Content::Audio { data, mime_type })
            }
            Some("thinking") => {
                let thinking = value.get("thinking").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let sig = value.get("thinking_signature").and_then(|s| s.as_str()).map(|s| s.to_string());
                let redacted = value.get("redacted").and_then(|r| r.as_bool()).unwrap_or(false);
                Ok(Content::Thinking { thinking, thinking_signature: sig, redacted })
            }
            Some("toolCall") => {
                // ToolCall 结构体没有 `type` 字段，但 serde 默认忽略未知字段，
                // 因此将整个 Value（含 `"type": "toolCall"`）反序列化为 ToolCall 是安全的。
                let tc = ToolCall::deserialize(value).map_err(serde::de::Error::custom)?;
                Ok(Content::ToolCall(tc))
            }
            Some(other) => Ok(Content::Text {
                text: format!("[unsupported content type: {}]", other),
                text_signature: None,
            }),
            None => Err(serde::de::Error::custom("missing 'type' field in Content")),
        }
    }
}
```

> 这样不引入 `Unknown` 变体，避免修改所有 match `Content` 的代码。每个分支从 `Value` 中直接提取字段，不依赖内层结构体的 `Deserialize`（因为 `type` 字段会冲突）。

- [ ] **Step 4: 更新理解型多模态模型的 `input_modalities`**

在 `models_data.rs` 中，审核并更新以下模型的 `input_modalities`：

| 模型 | 应更新为 |
|---|---|
| GPT-4o 系列 | `vec![Modality::Text, Modality::Image]`（vision 支持） |
| Gemini 系列 | `vec![Modality::Text, Modality::Image, Modality::Video, Modality::Audio]`（原生多模态） |
| Claude Sonnet 4 | `vec![Modality::Text, Modality::Image]`（vision 支持） |
| 豆包 seed-1.6-vision | `vec![Modality::Text, Modality::Image, Modality::Video]`（如支持 video） |

> `models_data.rs` 使用 `insert!` 宏生成模型数据，直接修改宏调用中的 modalities 数组即可。
>
> **注意**：`transform.rs` 中 `TransformOptions` 的 `supports_images` / `supports_video_input` / `supports_audio_input` 通过 `model_meta.input_modalities` 推导。若元数据标注错误，会导致不该降级的内容被降级，或向不支持的 provider 发送非法内容。

- [ ] **Step 5: 补全所有 Content match 分支**

需要检查并补全的文件：

**含 `_ =>` 通配符的分支（新增变体自动被捕获，不会编译失败，但建议显式处理以提高可读性）：**
- `crates/ai-provider/src/providers/openai.rs`（UserMessage / AssistantMessage / ToolResult）
- `crates/ai-provider/src/providers/mistral.rs`（同上）
- `crates/ai-provider/src/providers/google.rs`（UserMessage / AssistantMessage / ToolResult：现有 `_ =>` 通配符已降级为空文本/None）

**无需修改（不直接 exhaustive match `Content` 枚举）：**
- `crates/agent-core/src/harness/agent_loop.rs`（仅 `if let Content::ToolCall` 和 `_ => None` 过滤，不涉及 Content 变体扩展）
- `crates/agent-core/src/hook/default_dispatcher.rs`（未 match Content 枚举）
- `crates/agent-core/src/harness/session.rs`（`complete` 方法使用 `_ => None` 通配符，自动过滤新增变体）
- `crates/ai-provider/src/providers/anthropic_common.rs`（StreamParser 从 SSE 事件手动构建 Content，不 match Content 枚举本身）

**⚠️ exhaustive match（无 `_` 分支，新增变体后必然编译失败，必须修改）：**
- `crates/agent-core/src/harness/compaction/mod.rs`（`estimate_tokens`：`AgentMessage::Assistant` 分支无 `_` 通配符，新增 `Video`/`Audio` 后**必然编译失败**，必须添加分支 → 4800 token。`AgentMessage::User` 和 `AgentMessage::ToolResult` 分支已有 `_ => 0` 通配符，**不会编译失败**，但会**静默 undercount**，建议显式添加 `Video`/`Audio` 分支）

**不直接 match `Content` 枚举，无需因新增变体而修改：**
- `crates/ai-provider/src/providers/anthropic_common.rs`（StreamParser 从 SSE 事件手动构建 Content，不 match Content 枚举本身）

**其他需处理：**
- `crates/ai-provider/src/transform.rs`：新增 `downgrade_video_audio()` 函数（参考 `downgrade_images()`）

**每个 match 分支的默认处理：**
- `openai.rs` / `mistral.rs`：序列化为 `video_url` / `input_audio`
- `google.rs` UserMessage / AssistantMessage / ToolResult：`Video`/`Audio` 降级为空文本/None（现有 `_ =>` 通配符自动处理，但建议显式添加分支以提高可读性）
- `transform.rs`：不支持 video/audio 的 provider 降级为 `[video: {mime_type}]` / `[audio: {mime_type}]`
- `agent_loop.rs`：透传（不 emit MessageUpdate）
- `compaction.rs`：`Video`/`Audio` 按 4800 token 估算（与 Image 同权）
- `default_dispatcher.rs`：忽略

- [ ] **Step 6: 编译验证**

```bash
cargo check -p ai-provider
cargo check -p agent-core
```

Expected: PASS（所有 exhaustive match 已补全，`_ =>` 通配符分支也已显式处理）

---

### Task 1.2: openai_compatible_stream Video/Audio 序列化

**Files:**
- Modify: `crates/ai-provider/src/providers/openai.rs`

**Steps:**

- [ ] **Step 1: 扩展 UserMessage content 序列化**

在 `openai_compatible_stream` 的 messages 构建循环中，为 `Content::Video` / `Content::Audio` 增加分支：

```rust
crate::Content::Video { data, mime_type } => serde_json::json!({
    "type": "video_url",
    "video_url": { "url": format!("data:{};base64,{}", mime_type, data) }
}),
crate::Content::Audio { data, mime_type } => serde_json::json!({
    "type": "input_audio",
    "input_audio": { "data": data, "format": mime_type.strip_prefix("audio/").unwrap_or("wav") }
}),
```

- [ ] **Step 2: 扩展 AssistantMessage content 序列化**

AssistantMessage 理论上不应包含 Video/Audio（理解型多模态的输出是文本）。当前代码中 AssistantMessage 的 match 已有 `_ => serde_json::json!({"type": "text", "text": ""})` 通配符，新增 `Video`/`Audio` 变体后**不会导致编译错误**，但建议显式处理以提高可读性：

```rust
crate::Content::Video { .. } | crate::Content::Audio { .. } => {
    // Assistant 不应输出 video/audio；降级为空文本
    serde_json::json!({"type": "text", "text": ""})
}
```

- [ ] **Step 3: ToolResult content 序列化（无需修改）**

当前 `openai_compatible_stream` 中 ToolResult 的序列化逻辑已使用 `filter_map` 只提取 `Content::Text`：

```rust
"content": m.content.iter().filter_map(|c| match c {
    crate::Content::Text { text, .. } => Some(text.as_str()),
    _ => None,
}).collect::<Vec<_>>().join("\n")
```

新增 `Content::Video` / `Content::Audio` 后，`_ => None` 分支会自动过滤它们，无需额外修改。

> 注意：`MediaGenerationTool` 返回给 LLM 的是文本描述（如"视频已保存至 /workspaces/..."），不是 `Content::Video` / `Content::Audio`，因此不会触发此问题。

- [ ] **Step 4: 编译验证**

```bash
cargo check -p ai-provider
cargo test -p ai-provider --lib
```

Expected: PASS

---

### Task 1.3: transform 降级 + 兼容处理

**Files:**
- Modify: `crates/ai-provider/src/compat.rs`
- Modify: `crates/ai-provider/src/transform.rs`

**Steps:**

- [ ] **Step 1: TransformOptions 扩展**

在 `TransformOptions` struct 中新增：

```rust
pub struct TransformOptions {
    pub target_api: Option<String>,
    pub supports_images: bool,
    pub supports_video_input: bool,   // 新增
    pub supports_audio_input: bool,   // 新增
    pub preserve_thinking: bool,
}
```

> **注意**：`OpenAiCompat` **不需要**新增 `supports_video_input`/`supports_audio_input` 字段。`TransformOptions` 的这两个标志由 `AgentLoop` 根据 `model_meta.input_modalities` 直接推导（见 Step 4），比 provider 级别检测更准确（模型粒度 vs provider 粒度）。

- [ ] **Step 2: `transform.rs` 降级逻辑扩展**

在 `transform.rs` 中新增 `downgrade_video_audio()` 和 `downgrade_tool_result_media()`：

```rust
pub fn transform_messages(messages: &[Message], options: &TransformOptions) -> Vec<Message> {
    let mut result: Vec<Message> = messages.to_vec();

    // 1. Image downgrade（已有）
    if !options.supports_images {
        downgrade_images(&mut result);
    }

    // 2. Video/Audio downgrade（新增）
    downgrade_video_audio(&mut result, options);

    // 3. ToolResult 媒体降级（新增）—— 确保 OpenAI-compatible 序列化前 ToolResult 只含 Text
    downgrade_tool_result_media(&mut result);

    // 4. Thinking block handling（已有）
    if !options.preserve_thinking {
        remove_thinking_blocks(&mut result);
    }

    // 5. Tool call ID normalization（已有）
    normalize_tool_call_ids(&mut result);

    // 6. Orphan tool result padding（已有）
    pad_orphan_tool_results(&mut result);

    result
}

pub fn downgrade_video_audio(messages: &mut [Message], options: &TransformOptions) {
    if options.supports_video_input && options.supports_audio_input {
        return;
    }

    for msg in messages.iter_mut() {
        let content = match msg {
            Message::User(u) => &mut u.content,
            Message::ToolResult(t) => &mut t.content,
            _ => continue,
        };

        for c in content.iter_mut() {
            match c {
                Content::Video { mime_type, .. } if !options.supports_video_input => {
                    *c = Content::Text {
                        text: format!("[video: {}]", mime_type),
                        text_signature: None,
                    };
                }
                Content::Audio { mime_type, .. } if !options.supports_audio_input => {
                    *c = Content::Text {
                        text: format!("[audio: {}]", mime_type),
                        text_signature: None,
                    };
                }
                _ => {}
            }
        }
    }
}

/// 将 ToolResult 中的 Image/Video/Audio 降级为文本描述，确保 OpenAI-compatible 序列化不丢失信息。
/// OpenAI Chat Completions API 的 `role: "tool"` 不支持多模态数组，无法直接传递图片/视频/音频。
pub fn downgrade_tool_result_media(messages: &mut [Message]) {
    for msg in messages.iter_mut() {
        if let Message::ToolResult(tr) = msg {
            let mut text_parts: Vec<String> = Vec::new();
            for c in &tr.content {
                match c {
                    Content::Text { text, .. } => text_parts.push(text.clone()),
                    Content::Image { mime_type, .. } => {
                        text_parts.push(format!("[image: {}]", mime_type));
                    }
                    Content::Video { mime_type, .. } => {
                        text_parts.push(format!("[video: {}]", mime_type));
                    }
                    Content::Audio { mime_type, .. } => {
                        text_parts.push(format!("[audio: {}]", mime_type));
                    }
                    _ => {}
                }
            }
            tr.content = vec![Content::Text {
                text: text_parts.join("\n"),
                text_signature: None,
            }];
        }
    }
}
```

- [ ] **Step 3: 更新 `AgentLoop` 中 `TransformOptions` 的构造**

`crates/agent-core/src/harness/agent_loop.rs` 中创建 `TransformOptions` 的代码需要补充 video/audio 支持检测：

```rust
let supports_images = model_meta
    .as_ref()
    .map(|m| m.input_modalities.iter().any(|modality| matches!(modality, ai_provider::Modality::Image)))
    .unwrap_or(false);
let supports_video_input = model_meta
    .as_ref()
    .map(|m| m.input_modalities.iter().any(|modality| matches!(modality, ai_provider::Modality::Video)))
    .unwrap_or(false);
let supports_audio_input = model_meta
    .as_ref()
    .map(|m| m.input_modalities.iter().any(|modality| matches!(modality, ai_provider::Modality::Audio)))
    .unwrap_or(false);

let transform_opts = ai_provider::TransformOptions {
    target_api,
    supports_images,
    supports_video_input,   // 新增
    supports_audio_input,   // 新增
    preserve_thinking: false,
};
```

- [ ] **Step 4: 编译验证**

```bash
cargo check -p ai-provider
cargo check -p agent-core
cargo test -p ai-provider transform::tests
```

Expected: PASS

---

### Task 1.4: TUI 客户端 `HistoricalContent` 适配

**背景：** API Gateway 的 `GET /sessions/{id}/messages` 直接返回 `Vec<agent_core::AgentMessage>`（使用 `ai_provider::Message` 序列化）。TUI 通过 REST API 获取历史消息并反序列化为 `Vec<HistoricalMessage>`，其 `content` 字段类型为 `Vec<HistoricalContent>`。当前 `HistoricalContent` 只有 `Text`/`Thinking`/`ToolCall` 变体，缺失 `Image`/`Video`/`Audio`。当消息历史中出现这些类型时，TUI 反序列化会因 serde 找不到对应变体而失败。

**Files:**
- Modify: `crates/tui/src/client/model.rs`
- Modify: `crates/tui/src/app.rs`

**Steps:**

- [ ] **Step 1: 扩展 `HistoricalContent` 变体**

在 `tui/src/client/model.rs` 中为 `HistoricalContent` 添加多模态变体：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HistoricalContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },      // 新增
    #[serde(rename = "video")]
    Video { data: String, mime_type: String },      // 新增
    #[serde(rename = "audio")]
    Audio { data: String, mime_type: String },      // 新增
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "toolCall")]
    ToolCall { id: String, name: String, arguments: serde_json::Value },
}
```

> 使用 derive `Deserialize` 即可，`serde(tag = "type")` 会自动映射 `"type": "image"` / `"video"` / `"audio"`。
>
> **兼容性策略**：`HistoricalContent` 当前使用 derive `Deserialize`，与 `Content` 的手写 Deserializer 策略不一致。
>
> **默认行为（API Gateway 降级）**：API Gateway 的 `GET /sessions/{id}/messages` 端点已默认将 `Content::Image`/`Video`/`Audio` 降级为 `Content::Text { text: "[图片/视频/音频内容]" }`（见 Task 1.5）。因此旧版 TUI 不会反序列化失败，TUI 扩展 `HistoricalContent` 仅为正向能力增强（未来支持多媒体渲染）。

- [ ] **Step 2: 更新 `tui/src/app.rs` 的 exhaustive match**

`convert_history` 函数中对 `HistoricalContent` 的 match（line 1229）没有 `_` 分支，新增变体后必然编译失败。添加显式的 `Image`/`Video`/`Audio` 分支以忽略暂时不支持渲染的多模态内容（保留编译器对新变体的检查）：

```rust
for c in a.content {
    match c {
        HistoricalContent::Text { text } => {
            text_lines.push(ratatui::text::Line::from(text));
        }
        HistoricalContent::Thinking { thinking } => {
            // ... existing logic ...
        }
        HistoricalContent::ToolCall { id, name, arguments } => {
            // ... existing logic ...
        }
        HistoricalContent::Image { .. }
        | HistoricalContent::Video { .. }
        | HistoricalContent::Audio { .. } => {
            // TUI 暂不支持渲染多模态内容，忽略
        }
    }
}
```

> `app.rs` 中另外两处对 `HistoricalContent` 的 match（UserMessage line 1218、ToolResult line 1265）已有 `_ => None` 通配符，新增变体后自动被过滤，无需修改。

- [ ] **Step 3: 编译验证**

```bash
cargo check -p tui
```

Expected: PASS

---

### Task 1.5: API Gateway 默认降级

**Files:**
- Modify: `crates/api-gateway/src/routes/sessions.rs`（`messages` GET handler）

**Steps:**

- [ ] **Step 1: 在 `messages` 端点中实现默认降级**

在返回 `Json<Vec<AgentMessage>>` 前，遍历消息内容，将 `Content::Image`/`Video`/`Audio` 降级为 `Content::Text` 占位符：

```rust
// crates/api-gateway/src/routes/sessions.rs
pub async fn messages(
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<agent_core::AgentMessage>>, GatewayError> {
    let mut msgs = state.tenant_manager.get_session_messages(&tenant_id.0, &id).await?;
    // 默认降级：旧版 TUI 兼容性
    for msg in &mut msgs {
        downgrade_media_in_message(msg);
    }
    Ok(Json(msgs))
}

/// 将消息中的 Image/Video/Audio 降级为 Text 占位符（仅影响响应，不修改原始数据）。
fn downgrade_media_in_message(msg: &mut agent_core::AgentMessage) {
    use agent_core::AgentMessage;
    let content = match msg {
        AgentMessage::User(u) => Some(&mut u.content),
        AgentMessage::Assistant(a) => Some(&mut a.content),
        AgentMessage::ToolResult(t) => Some(&mut t.content),
    };
    if let Some(content) = content {
        for c in content.iter_mut() {
            let replacement = match c {
                ai_provider::Content::Image { .. } => Some("[图片内容]"),
                ai_provider::Content::Video { .. } => Some("[视频内容]"),
                ai_provider::Content::Audio { .. } => Some("[音频内容]"),
                _ => None,
            };
            if let Some(text) = replacement {
                *c = ai_provider::Content::Text {
                    text: text.to_string(),
                    text_signature: None,
                };
            }
        }
    }
}
```

> 降级只影响响应，**不修改** `SessionStore` 中的原始数据。未来若 TUI 支持多媒体显示，可通过 Query Param（如 `?format=raw`）或 HTTP Header（如 `X-Client-Supports-Multimedia`）请求未降级数据。

- [ ] **Step 2: 编译验证**

```bash
cargo check -p api-gateway
```

Expected: PASS

---

### Task 1.6: `AgentSpace::media_dir` 扩展

**Files:**
- Modify: `crates/agent-core/src/space.rs`

**Steps:**

- [ ] **Step 1: 新增 `media_dir` 方法**

```rust
impl AgentSpace {
    /// `{root}/workspaces/{tenant_id}/media/`
    pub fn media_dir(&self, tenant_id: &str) -> PathBuf {
        self.workspace_for(tenant_id).join("media")
    }
}
```

> `ensure_dirs()` 不需要预先创建 `media` 子目录（每个租户独立，按需创建）。`MediaGenerationTool` 和 Phase 3 的外移逻辑均使用 `media_dir()` 而非直接拼接路径。

- [ ] **Step 2: 编译验证**

```bash
cargo check -p agent-core
```

Expected: PASS

---

### Task 1.7: SSE 事件流 ToolResult 文本提取改进

**Files:**
- Modify: `crates/api-gateway/src/routes/events.rs`

**Steps:**

- [ ] **Step 1: 将 `extract_text_content` 改进为 `extract_tool_result_text`**

当前 `extract_text_content` 只提取 `Content::Text`，丢弃 `Image`/`Video`/`Audio`。改进后将非文本内容转换为文本占位符，确保客户端通过 SSE 知道有媒体内容：

```rust
fn extract_tool_result_text(contents: &[Content]) -> Option<String> {
    let parts: Vec<String> = contents.iter().map(|c| match c {
        Content::Text { text, .. } => text.clone(),
        Content::Image { mime_type, .. } => format!("[image: {}]", mime_type),
        Content::Video { mime_type, .. } => format!("[video: {}]", mime_type),
        Content::Audio { mime_type, .. } => format!("[audio: {}]", mime_type),
        _ => String::new(),
    }).collect();
    let joined = parts.join("\n");
    if joined.is_empty() { None } else { Some(joined) }
}
```

> SSE 不适合传输大体积 base64 数据。客户端如需显示图片/视频/音频，应调用 `GET /sessions/{id}/messages` 获取完整消息历史（该端点已由 API Gateway 默认降级保护旧版客户端，新版客户端可通过 `?format=raw` 获取未降级数据）。

- [ ] **Step 2: 编译验证**

```bash
cargo check -p api-gateway
```

Expected: PASS

---

## Phase 2: 生成型多模态核心

### Task 2.1: MediaProvider trait + 类型系统

**Files:**
- Modify: `crates/ai-provider/src/media/mod.rs`（在 Phase 0 骨架上扩展）
- Create: `crates/ai-provider/src/media/error.rs`
- Create: `crates/ai-provider/src/media/task.rs`
- Modify: `crates/ai-provider/src/lib.rs`

**Steps:**

- [ ] **Step 1: 扩展 `media/mod.rs`**

在 Phase 0 的骨架基础上，**追加** `MediaProvider` trait 和新类型定义（保留已有的 `pub mod error; pub mod task; pub mod registry;` 和 re-exports）：

```rust
// crates/ai-provider/src/media/mod.rs
pub mod error;
pub mod registry;
pub mod task;

pub use error::MediaError;
pub use registry::{MediaModel, MediaModelRegistry};
pub use task::{MediaTaskType, media_task_type_from_str};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub enum MediaRequest {
    ImageGeneration {
        prompt: String,
        size: Option<String>,
        style: Option<String>,
        quality: Option<String>,
        n: Option<u32>,
    },
    VideoGeneration {
        prompt: String,
        duration: Option<u32>,
        resolution: Option<String>,
        aspect_ratio: Option<String>,
        image_refs: Vec<MediaReference>,
    },
    AudioGeneration {
        prompt: String,
        voice: Option<String>,
        format: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct MediaReference {
    pub url: String,
    pub mime_type: String,
}

#[derive(Debug, Clone)]
pub enum MediaResponse {
    Inline { data: String, mime_type: String },
    Reference { url: String, mime_type: String },
}

#[async_trait]
pub trait MediaProvider: Send + Sync {
    fn provider_name(&self) -> &str;
    fn supported_tasks(&self) -> Vec<MediaTaskType>;

    async fn generate(
        &self,
        model: &str,
        request: MediaRequest,
        signal: CancellationToken,
    ) -> Result<MediaResponse, MediaError>;

    /// 下载远程媒体文件。默认实现使用 provider 内部 HTTP client。
    async fn download(
        &self,
        url: &str,
        max_size: u64,
        signal: CancellationToken,
    ) -> Result<Vec<u8>, MediaError> {
        let client = self.client();
        let mut response = client.get(url).send().await
            .map_err(|e| MediaError::DownloadFailed(e.to_string()))?
            .error_for_status()
            .map_err(|e| MediaError::DownloadFailed(format!("HTTP error: {}", e)))?;
        let mut downloaded: u64 = 0;
        let mut buffer = Vec::new();
        while let Some(chunk) = response.chunk().await
            .map_err(|e| MediaError::DownloadFailed(e.to_string()))?
        {
            downloaded += chunk.len() as u64;
            if downloaded > max_size {
                return Err(MediaError::FileTooLarge { size: downloaded, max: max_size });
            }
            buffer.extend_from_slice(&chunk);
        }
        Ok(buffer)
    }

    /// 返回内部 HTTP client（供 download 默认实现使用）。
    fn client(&self) -> &reqwest::Client;
}
```

- [ ] **Step 2: 创建 `media/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("unsupported task type: {0}")]
    UnsupportedTask(String),
    #[error("task failed: {0}")]
    TaskFailed(String),
    #[error("task timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("task cancelled")]
    Cancelled,
    #[error("download failed: {0}")]
    DownloadFailed(String),
    #[error("file too large: {size} bytes, max: {max} bytes")]
    FileTooLarge { size: u64, max: u64 },
}
```

- [ ] **Step 3: 创建 `media/task.rs`**

```rust
/// 异步任务句柄（用于需要轮询的 provider，如 Seedance）
pub struct MediaTaskHandle {
    pub task_id: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaTaskStatus {
    Queued,
    Running,
    Completed,
    Failed { reason: String },
}

/// 指数退避轮询辅助函数
pub fn next_poll_interval(current: std::time::Duration, max: std::time::Duration) -> std::time::Duration {
    std::cmp::min(current * 2, max)
}
```

- [ ] **Step 4: `lib.rs` 补充导出**

Phase 0 已添加 `pub mod media;`，此处补充 `pub use media::*;`：

```rust
pub use media::*;
```

- [ ] **Step 5: 编译验证**

```bash
cargo check -p ai-provider
```

Expected: PASS

---

### Task 2.2: define_media_provider! 宏

**Files:**
- Create: `crates/ai-provider/src/providers/media_shared.rs`

**Steps:**

- [ ] **Step 1: 参考 `shared.rs` 的 `define_provider!` 宏，创建简化版 `define_media_provider!`**

```rust
macro_rules! define_media_provider {
    (
        $struct_name:ident,
        $provider_str:literal,
        $env_key:literal,
        $default_url:literal
    ) => {
        pub struct $struct_name {
            config: crate::providers::shared::ProviderConfig,
        }

        impl $struct_name {
            pub fn new(api_key: Option<secrecy::SecretString>) -> Self {
                Self::with_base_url(api_key, $default_url)
            }

            pub fn with_base_url(
                api_key: Option<secrecy::SecretString>,
                base_url: &str,
            ) -> Self {
                Self {
                    config: crate::providers::shared::ProviderConfig::new(
                        api_key,
                        base_url,
                        $provider_str,
                        $env_key,
                    ),
                }
            }

            pub fn with_client(
                client: reqwest::Client,
                api_key: Option<secrecy::SecretString>,
                base_url: &str,
            ) -> Self {
                Self {
                    config: crate::providers::shared::ProviderConfig::with_client(
                        client,
                        api_key,
                        base_url,
                        $provider_str,
                        $env_key,
                    ),
                }
            }

            pub fn config(&self) -> &crate::providers::shared::ProviderConfig {
                &self.config
            }
        }
    };
}

pub(crate) use define_media_provider;
```

> 与 `shared.rs` 的 `define_provider!` 风格保持一致：不使用 `#[macro_export]`，通过 `pub(crate) use` 导出，调用路径为 `crate::providers::media_shared::define_media_provider!`。

- [ ] **Step 2: 注册 `media_shared` 模块**

在 `crates/ai-provider/src/providers/mod.rs` 中声明新模块，使 `doubao_media.rs` 能引用 `crate::providers::media_shared::define_media_provider!`：

```rust
#[macro_use]
pub mod shared;
pub mod media_shared;   // 新增

pub mod anthropic;
// ... 其余模块保持不变 ...
```

> 使用 `#[macro_use]` 可确保宏在当前 crate 内可见。`pub(crate)` 级别已足够，因为 `define_media_provider!` 仅通过 `pub(crate) use` 导出。

- [ ] **Step 3: 编译验证**

```bash
cargo check -p ai-provider
```

Expected: PASS

---

### Task 2.3: DoubaoMediaProvider（Seedream 图片生成）

**Files:**
- Create: `crates/ai-provider/src/providers/doubao_media.rs`
- Modify: `crates/ai-provider/src/providers/mod.rs`
- Modify: `crates/ai-provider/src/lib.rs`

**Steps:**

- [ ] **Step 1: 创建 `doubao_media.rs`**

```rust
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

use crate::media::{MediaError, MediaProvider, MediaRequest, MediaResponse, MediaTaskType};

crate::providers::media_shared::define_media_provider!(
    DoubaoMediaProvider,
    "doubao",
    "DOUBAO_API_KEY",
    "https://ark.cn-beijing.volces.com/api/v3"
);

impl DoubaoMediaProvider {
    async fn generate_image(
        &self,
        model: &str,
        prompt: String,
        size: Option<String>,
    ) -> Result<MediaResponse, MediaError> {
        // Seedream API 实现
        // POST /contents/generations
        // Body: { model, prompt, width, height }
        // Response: { data: [{ url, ... }] }
        todo!("implement Seedream image generation")
    }

    async fn generate_video(
        &self,
        model: &str,
        prompt: String,
        duration: Option<u32>,
        resolution: Option<String>,
        aspect_ratio: Option<String>,
        image_refs: Vec<crate::media::MediaReference>,
        signal: CancellationToken,
    ) -> Result<MediaResponse, MediaError> {
        // Seedance API 实现（异步任务）
        // POST /contents/generations/tasks
        // GET  /contents/generations/tasks/{task_id}
        todo!("implement Seedance video generation")
    }
}

#[async_trait::async_trait]
impl MediaProvider for DoubaoMediaProvider {
    fn provider_name(&self) -> &str {
        "doubao"
    }

    fn supported_tasks(&self) -> Vec<MediaTaskType> {
        vec![MediaTaskType::ImageGeneration, MediaTaskType::VideoGeneration]
    }

    async fn generate(
        &self,
        model: &str,
        request: MediaRequest,
        signal: CancellationToken,
    ) -> Result<MediaResponse, MediaError> {
        match request {
            MediaRequest::ImageGeneration { prompt, size, .. } => {
                self.generate_image(model, prompt, size).await
            }
            MediaRequest::VideoGeneration { prompt, duration, resolution, aspect_ratio, image_refs } => {
                self.generate_video(model, prompt, duration, resolution, aspect_ratio, image_refs, signal).await
            }
            _ => Err(MediaError::UnsupportedTask("unsupported media request".to_string())),
        }
    }

    fn client(&self) -> &reqwest::Client {
        &self.config.client
    }
}
```

> **实现说明**：Phase 2 先完成骨架 + Seedream 图片生成（同步 API）。Seedance 视频生成的异步轮询在 Phase 2.5 实现。

- [ ] **Step 2: 注册模块**

`providers/mod.rs` 添加 `pub mod doubao_media;`

`lib.rs` 添加 `pub use providers::doubao_media::DoubaoMediaProvider;`

- [ ] **Step 3: 编译验证**

```bash
cargo check -p ai-provider
```

Expected: PASS（todo!() 允许编译通过）

---

### Task 2.4: MediaGenerationTool

> **文件位置说明**：当前 `agent-core/src/skills/` 只包含 skill 元数据类型，不包含可执行工具。`MediaGenerationTool` 作为首个生产级 `AgentTool` 实现，放在 `src/tools/` 目录（Spec 推荐）。若项目偏好不新增 `tools/` 目录，也可放入 `src/harness/media_generation.rs`。

**Files:**
- Create: `crates/agent-core/src/tools/media_generation.rs`
- Create: `crates/agent-core/src/tools/mod.rs`
- Modify: `crates/agent-core/src/lib.rs`
- Modify: `crates/agent-core/Cargo.toml`

**Steps:**

- [ ] **Step 0: 添加依赖**

在 workspace `Cargo.toml` 的 `[workspace.dependencies]` 中新增：

```toml
base64 = "0.22"
```

在 `crates/agent-core/Cargo.toml` 中新增：

```toml
base64 = { workspace = true }
```

> `base64` 用于解码 Inline 数据。远程文件下载由 `MediaProvider::download` 默认实现处理，`agent-core` 不直接依赖 `reqwest`。

- [ ] **Step 1: 创建 `tools/mod.rs`**

```rust
pub mod media_generation;
pub use media_generation::MediaGenerationTool;
```

- [ ] **Step 2: 创建 `tools/media_generation.rs`**

```rust
use std::sync::Arc;
use async_trait::async_trait;
use ai_provider::media::{MediaProvider, MediaRequest, MediaResponse, MediaTaskType, MediaModelRegistry, media_task_type_from_str};
use crate::types::{AgentTool, AgentToolResult, AgentToolProgressUpdate, AgentError};
use crate::space::AgentSpace;

pub struct MediaGenerationTool {
    provider: Arc<dyn MediaProvider>,
    registry: Arc<MediaModelRegistry>,
    space: AgentSpace,
    default_model: String,
    tenant_id: String,
    /// 单文件大小上限（字节），默认 100MB。
    max_file_size: u64,
}

impl MediaGenerationTool {
    pub fn new(
        provider: Arc<dyn MediaProvider>,
        registry: Arc<MediaModelRegistry>,
        default_model: impl Into<String>,
        tenant_id: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            registry,
            space: AgentSpace::from_env_or_default(),
            default_model: default_model.into(),
            tenant_id: tenant_id.into(),
            max_file_size: 100 * 1024 * 1024,
        }
    }

    pub fn with_space(mut self, space: AgentSpace) -> Self {
        self.space = space;
        self
    }

    pub fn with_max_file_size(mut self, max: u64) -> Self {
        self.max_file_size = max;
        self
    }

    /// 根据 media_type 和可选的 model 参数，解析出实际使用的模型 ID。
    fn resolve_model(&self, media_type: &str, explicit_model: Option<&str>) -> Result<String, AgentError> {
        let task = media_task_type_from_str(media_type)
            .map_err(|e| AgentError::ToolExecutionFailed(e.to_string()))?;
        let candidate = explicit_model.unwrap_or(&self.default_model);
        if let Some(meta) = self.registry.get(candidate) {
            if meta.supported_tasks.contains(&task) {
                return Ok(candidate.to_string());
            }
        }
        // 自动回退：查找 registry 中第一个支持该 task 的模型
        self.registry.models_for_provider(self.provider.provider_name())
            .into_iter()
            .find(|m| m.supported_tasks.contains(&task))
            .map(|m| m.id.clone())
            .ok_or_else(|| AgentError::ToolExecutionFailed(
                format!("no model supports media_type: {}", media_type)
            ))
    }
}

#[async_trait]
impl AgentTool for MediaGenerationTool {
    fn name(&self) -> &str { "generate_media" }

    fn description(&self) -> &str {
        "Generate images, videos, or audio based on a text prompt. \
         Use this when the user asks for visual or audio content. \
         For images under 1MB, returns a base64 inline image; \
         for larger images and all videos/audio, saves to workspace and returns the file path."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "media_type": {
                    "type": "string",
                    "enum": ["image", "video", "audio"],
                    "description": "Type of media to generate"
                },
                "prompt": {
                    "type": "string",
                    "description": "Detailed description of the desired media"
                },
                "model": {
                    "type": "string",
                    "description": "Optional model ID to use. If omitted, uses the default model or auto-selects one that supports the requested media_type."
                },
                "size": {
                    "type": "string",
                    "description": "Size hint. For images: e.g. '1024x1024'. For videos: mapped to 'resolution' field, e.g. '1080p'."
                },
                "duration": {
                    "type": "integer",
                    "description": "Duration in seconds, for video only"
                }
            },
            "required": ["media_type", "prompt"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
        signal: CancellationToken,
    ) -> Result<AgentToolResult, AgentError> {
        let media_type = params["media_type"].as_str()
            .ok_or_else(|| AgentError::ToolExecutionFailed("media_type is required".to_string()))?;
        let prompt = params["prompt"].as_str().unwrap_or("").to_string();
        let explicit_model = params.get("model").and_then(|m| m.as_str());

        if let Some(cb) = on_progress {
            cb(AgentToolProgressUpdate {
                content: format!("正在生成 {}...", media_type),
            });
        }

        let model = self.resolve_model(media_type, explicit_model)?;

        let request = match media_type {
            "image" => MediaRequest::ImageGeneration {
                prompt,
                size: params["size"].as_str().map(|s| s.to_string()),
                style: None,
                quality: None,
                n: Some(1),
            },
            "video" => MediaRequest::VideoGeneration {
                prompt,
                duration: params["duration"].as_u64().map(|d| d as u32),
                resolution: params["size"].as_str().map(|s| s.to_string()),
                aspect_ratio: None,
                image_refs: vec![], // Phase 2 暂不支持参考图
            },
            "audio" => MediaRequest::AudioGeneration {
                prompt,
                voice: None,
                format: Some("mp3".to_string()),
            },
            _ => return Err(AgentError::ToolExecutionFailed(format!("unsupported media_type: {}", media_type))),
        };

        let response = self.provider.generate(&model, request, signal.clone())
            .await
            .map_err(|e| AgentError::ToolExecutionFailed(e.to_string()))?;

        let (content, mut details) = match response {
            MediaResponse::Inline { data, mime_type } => {
                if mime_type.starts_with("image/") && data.len() < (1024 * 1024) {
                    (vec![ai_provider::Content::Image { data, mime_type }], serde_json::Map::new())
                } else {
                    let path = self.save_media_to_workspace(&data, &mime_type).await?;
                    (vec![ai_provider::Content::Text {
                        text: format!("媒体已保存至 {}", path.display()),
                        text_signature: None,
                    }], serde_json::Map::new())
                }
            }
            MediaResponse::Reference { url, mime_type } => {
                let bytes = self.provider.download(&url, self.max_file_size, signal).await
                    .map_err(|e| AgentError::ToolExecutionFailed(e.to_string()))?;
                let path = self.save_media_to_workspace_bytes(&bytes, &mime_type).await?;
                let mut d = serde_json::Map::new();
                d.insert("url".to_string(), serde_json::Value::String(url));
                d.insert("mime_type".to_string(), serde_json::Value::String(mime_type));
                (vec![ai_provider::Content::Text {
                    text: format!("媒体已保存至 {}", path.display()),
                    text_signature: None,
                }], d)
            }
        };

        // 注入成本信息
        if let Some(cost) = self.registry.get(&model).and_then(|m| m.cost_per_call) {
            details.insert("cost_per_call".to_string(), serde_json::json!(cost));
            details.insert("currency".to_string(), serde_json::json!("CNY"));
        }
        details.insert("model".to_string(), serde_json::json!(model));
        details.insert("media_type".to_string(), serde_json::json!(media_type));

        Ok(AgentToolResult {
            content,
            details: Some(serde_json::Value::Object(details)),
            is_error: false,
            terminate: false,
        })
    }
}

impl MediaGenerationTool {
    /// 将 base64 字符串解码后保存到租户 workspace。
    async fn save_media_to_workspace(
        &self,
        data: &str,
        mime_type: &str,
    ) -> Result<std::path::PathBuf, AgentError> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data)
            .map_err(|e| AgentError::ToolExecutionFailed(format!("base64 decode failed: {}", e)))?;
        self.save_media_to_workspace_bytes(&bytes, mime_type).await
    }

    /// 将已解码的字节数据保存到租户 workspace。
    async fn save_media_to_workspace_bytes(
        &self,
        bytes: &[u8],
        mime_type: &str,
    ) -> Result<std::path::PathBuf, AgentError> {
        // 写入 workspace/{tenant_id}/media/ 子目录，避免污染 workspace 根目录
        let workspace = self.space.media_dir(&self.tenant_id);
        tokio::fs::create_dir_all(&workspace).await
            .map_err(|e| AgentError::ToolExecutionFailed(format!("create workspace failed: {}", e)))?;

        if bytes.len() as u64 > self.max_file_size {
            return Err(AgentError::ToolExecutionFailed(format!(
                "file too large: {} bytes, max: {} bytes",
                bytes.len(),
                self.max_file_size
            )));
        }

        let ext = mime_type.split('/').nth(1).unwrap_or("bin");
        // 处理复合 MIME type，如 "image/svg+xml" → "svg"
        let ext = ext.split('+').next().unwrap_or(ext);
        let filename = format!("{}.{}", uuid::Uuid::new_v4(), ext);
        let path = workspace.join(&filename);
        tokio::fs::write(&path, bytes).await
            .map_err(|e| AgentError::ToolExecutionFailed(format!("write file failed: {}", e)))?;

        Ok(path)
    }
}
```

> **参考图（image_refs）说明**：`MediaRequest::VideoGeneration` 支持传入 `image_refs`，但 `generate_media` 工具在 Phase 2 暂不暴露该参数。后续迭代中可扩展 JSON Schema 和参数解析逻辑以支持首帧/尾帧/参考图。
>
> **安全说明**：`generate_media` 的输出路径由工具内部根据 `tenant_id` 计算（`AgentSpace::media_dir(tenant_id)`），不暴露 `path` 参数给 LLM，因此天然受 workspace 边界限制。PathGuard 的 `extract_paths` 从 tool 参数中提取路径，由于 JSON schema 中没有 `path` 字段，无需额外拦截。Inline/Reference 媒体数据均经流式下载或 base64 解码后直接写入磁盘，不经过 LLM 上下文。

- [ ] **Step 3: 注册模块**

`lib.rs` 添加 `pub mod tools;` 和 `pub use tools::MediaGenerationTool;`

- [ ] **Step 4: 编译验证**

```bash
cargo check -p agent-core
```

Expected: PASS

- [ ] **Step 5: 工具注册到 SessionActor**

`MediaGenerationTool` 需要在创建 `SessionActor` 时注入到 `SessionConfig.tools` 中。在 `TenantSupervisor` 或 `SessionActor::new` 的调用方（如 `api-gateway` 的 session 创建逻辑）中组装：

```rust
use ai_provider::media::MediaModelRegistry;
use agent_core::tools::MediaGenerationTool;

let media_provider = Arc::new(ai_provider::DoubaoMediaProvider::new(api_key));
let media_registry = Arc::new(MediaModelRegistry::build_default());
let media_tool = Arc::new(MediaGenerationTool::new(
    media_provider,
    media_registry,
    "doubao-seedream-5-0", // default model
    tenant_id,
));

let session_config = SessionConfig {
    tools: vec![
        // ... 其他工具 ...
        media_tool as AgentToolRef,
    ],
    // ...
};
```

> **实现说明**：具体注册位置取决于项目的 session 创建流程。若 `api-gateway` 或 `tenant` crate 负责组装 `SessionConfig`，则在该处注入 `MediaGenerationTool`。若采用 skills 系统动态加载，可将 `MediaGenerationTool` 作为默认内置 skill。

---

## Phase 2.5: Seedance 视频生成（异步轮询）

### Task 2.5: DoubaoMediaProvider 视频生成实现

**Files:**
- Modify: `crates/ai-provider/src/providers/doubao_media.rs`

**Steps:**

- [ ] **Step 1: 实现 `generate_video` 方法**

内部封装异步任务轮询：

```rust
async fn generate_video(...) -> Result<MediaResponse, MediaError> {
    // 1. 创建任务
    let task = self.create_video_task(model, prompt, duration, resolution, aspect_ratio, image_refs).await?;
    
    // 2. 轮询
    let mut interval = Duration::from_secs(1);
    let max_interval = Duration::from_secs(30);
    let deadline = Instant::now() + Duration::from_secs(300); // 5分钟超时
    
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {},
            _ = signal.cancelled() => return Err(MediaError::Cancelled),
        }
        
        if Instant::now() > deadline {
            return Err(MediaError::Timeout(Duration::from_secs(300)));
        }
        
        let status = self.query_task(&task.task_id).await?;
        match status {
            MediaTaskStatus::Completed => break,
            MediaTaskStatus::Failed { reason } => return Err(MediaError::TaskFailed(reason)),
            _ => interval = std::cmp::min(interval * 2, max_interval),
        }
    }
    
    // 3. 获取结果
    self.fetch_video_result(&task.task_id).await
}
```

- [ ] **Step 2: 编写 mock HTTP 测试**

使用 `wiremock` 模拟：
- 创建任务 → 返回 task_id
- 第一次查询 → status: "queued"
- 第二次查询 → status: "running"
- 第三次查询 → status: "completed", result.url: "..."

- [ ] **Step 3: 编译验证**

```bash
cargo check -p ai-provider
```

Expected: PASS（完整集成测试在 Task 3.2 中运行，需先注册 `[[test]]` 条目）

---

## Phase 3: Polish

### Task 3.1: Media 模型注册表填充

**Files:**
- Modify: `crates/ai-provider/src/media/registry.rs`

**Steps:**

- [ ] **Step 1: 填充 `MediaModelRegistry`**

在 `registry.rs` 中提供静态构建函数。`MediaModel` 的 `supported_tasks` 在 Phase 0 已直接使用 `Vec<MediaTaskType>`，无需替换。

```rust
impl MediaModelRegistry {
    pub fn build_default() -> Self {
        let mut registry = Self::new();
        
        registry.insert(MediaModel {
            id: "doubao-seedream-5-0".to_string(),
            name: "Doubao Seedream 5.0".to_string(),
            provider: "doubao".to_string(),
            base_url: "https://ark.cn-beijing.volces.com/api/v3".to_string(),
            supported_tasks: vec![MediaTaskType::ImageGeneration],
            cost_per_call: Some(0.2),
            headers: None,
        });
        
        registry.insert(MediaModel {
            id: "doubao-seedance-2-0".to_string(),
            name: "Doubao Seedance 2.0".to_string(),
            provider: "doubao".to_string(),
            base_url: "https://ark.cn-beijing.volces.com/api/v3".to_string(),
            supported_tasks: vec![MediaTaskType::VideoGeneration],
            cost_per_call: Some(1.5),
            headers: None,
        });
        
        registry
    }
}
```

- [ ] **Step 2: `lib.rs` 导出默认注册表**

```rust
pub use media::registry::{MediaModel, MediaModelRegistry};
```

- [ ] **Step 3: 编译验证**

```bash
cargo check -p ai-provider
```

Expected: PASS

---

### Task 3.2: 集成测试

**Files:**
- Create: `crates/ai-provider/tests/integration/doubao_media_tests.rs`
- Modify: `crates/ai-provider/Cargo.toml`

**Steps:**

- [ ] **Step 1: 注册测试文件**

在 `crates/ai-provider/Cargo.toml` 中新增：

```toml
[[test]]
name = "doubao_media_tests"
path = "tests/integration/doubao_media_tests.rs"
```

- [ ] **Step 2: Seedream 图片生成 mock 测试**

```rust
#[tokio::test]
async fn test_mock_seedream_image_generation() {
    let server = MockServer::start().await;
    // mock POST /contents/generations → { data: [{ url: "..." }] }
    let provider = DoubaoMediaProvider::with_base_url(None, server.uri());
    let response = provider.generate("doubao-seedream-5-0", MediaRequest::ImageGeneration { ... }, CancellationToken::new()).await;
    assert!(matches!(response, Ok(MediaResponse::Reference { .. })));
}
```

- [ ] **Step 3: Seedance 视频生成异步轮询 mock 测试**

```rust
#[tokio::test]
async fn test_mock_seedance_video_generation_async() {
    let server = MockServer::start().await;
    // mock POST → task_id
    // mock GET x3 → queued → running → completed
    let provider = DoubaoMediaProvider::with_base_url(None, server.uri());
    let response = provider.generate("doubao-seedance-2-0", MediaRequest::VideoGeneration { ... }, CancellationToken::new()).await;
    assert!(matches!(response, Ok(MediaResponse::Reference { .. })));
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p ai-provider --test doubao_media_tests
```

Expected: PASS

---

### Task 3.3: 成本计量集成

**Files:**
- Modify: `crates/agent-core/src/tools/media_generation.rs`
- Modify: `crates/agent-core/src/hook/default_dispatcher.rs`
- Modify: `crates/tenant/src/meter.rs`（或新建）

**Steps:**

- [ ] **Step 1: MediaGenerationTool 注入成本信息**

成本注入已在 **Task 2.4 Step 2** 的 `MediaGenerationTool::execute` 中实现。核心逻辑：

```rust
let (content, mut details) = match response {
    MediaResponse::Inline { data, mime_type } => { /* ... */ (content, serde_json::Map::new()) }
    MediaResponse::Reference { url, mime_type } => {
        let mut d = serde_json::Map::new();
        d.insert("url".to_string(), serde_json::Value::String(url));
        d.insert("mime_type".to_string(), serde_json::Value::String(mime_type));
        (content, d)
    }
};

// 注入成本信息
if let Some(cost) = self.registry.get(&model).and_then(|m| m.cost_per_call) {
    details.insert("cost_per_call".to_string(), serde_json::json!(cost));
    details.insert("currency".to_string(), serde_json::json!("CNY"));
}
details.insert("model".to_string(), serde_json::json!(model));
details.insert("media_type".to_string(), serde_json::json!(media_type));

Ok(AgentToolResult {
    content,
    details: Some(serde_json::Value::Object(details)),
    is_error: false,
    terminate: false,
})
```

> **说明**：`MediaProvider` 返回的 `MediaResponse` 当前不含 `cost` 字段。成本通过 `MediaModelRegistry` 的元数据计算，由 `MediaGenerationTool` 在边界处注入到 `details` 中。`Inline` 分支无 `url`/`mime_type`，`details` 仅包含 `model`/`media_type` 和可选的 `cost_per_call`。

- [ ] **Step 2: `DefaultHookDispatcher` 扩展 `cost_callback`**

由于 `on_tool_execution_end` 当前在代码库中**无任何调用点**（trait 定义存在但从未被调用），成本记录必须通过 `on_tool_result` 完成：

```rust
// agent-core/src/hook/default_dispatcher.rs
pub struct DefaultHookDispatcher {
    pub space: AgentSpace,
    pub denied_tools: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub path_guard_fields: HashMap<String, Vec<String>>,
    pub path_guard_scan_unknown: bool,
    pub max_turns_per_session: usize,
    session_turn_counts: DashMap<String, AtomicUsize>,
    /// 可选：媒体成本回调 (tenant_id, cost_cny)
    pub cost_callback: Option<Arc<dyn Fn(&str, f64) + Send + Sync>>,
}

impl std::fmt::Debug for DefaultHookDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultHookDispatcher")
            .field("space", &self.space)
            .field("denied_tools", &self.denied_tools)
            .field("allowed_tools", &self.allowed_tools)
            .field("path_guard_fields", &self.path_guard_fields)
            .field("path_guard_scan_unknown", &self.path_guard_scan_unknown)
            .field("max_turns_per_session", &self.max_turns_per_session)
            .field("cost_callback", &self.cost_callback.is_some())
            .finish()
    }
}

#[async_trait]
impl HookDispatcher for DefaultHookDispatcher {
    // ...

    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        // 现有 PathGuard / ContentFilter 逻辑 ...

        // 媒体成本记录
        if let Some(ref cb) = self.cost_callback {
            if let Some(ref details) = ctx.details {
                if let Some(cost) = details.get("cost_per_call").and_then(|v| v.as_f64()) {
                    cb(&ctx.tenant_id, cost);
                }
            }
        }

        ToolResultMutation::default()
    }
}
```

> **注意**：`DefaultHookDispatcher` 原有 `#[derive(Debug)]`，新增 `dyn Fn` 字段后必须改为手动实现 `Debug`，否则编译失败。手动实现中 `cost_callback` 仅打印 `is_some()` 布尔值，避免暴露回调内部状态。

- [ ] **Step 3: tenant 层 `CostTracker` 新建**

在 `tenant/src/meter.rs` 中新增（或新建文件）：

```rust
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-tenant cumulative cost tracker.
pub struct CostTracker {
    media_cost_cny: AtomicU64,  // 以 0.001 CNY 为最小单位，避免浮点精度问题
    llm_cost_cny: AtomicU64,
}

impl CostTracker {
    pub fn new() -> Self {
        Self {
            media_cost_cny: AtomicU64::new(0),
            llm_cost_cny: AtomicU64::new(0),
        }
    }

    pub fn record_media_call(&self, cost_cny: f64) {
        let millis = (cost_cny * 1000.0) as u64;
        self.media_cost_cny.fetch_add(millis, Ordering::Relaxed);
    }

    pub fn media_cost_cny(&self) -> f64 {
        self.media_cost_cny.load(Ordering::Relaxed) as f64 / 1000.0
    }
}
```

在 `TenantManagerImpl::create_session` 中注入回调：

```rust
let cost_tracker = Arc::new(CostTracker::new());
let cost_tracker_clone = cost_tracker.clone();
let mut dispatcher = DefaultHookDispatcher::new();
dispatcher.cost_callback = Some(Arc::new(move |tenant_id, cost| {
    cost_tracker_clone.record_media_call(cost);
    tracing::info!(tenant_id, cost, "media cost recorded");
}));
let hook_dispatcher = Arc::new(dispatcher) as Arc<dyn HookDispatcher>;
```

> **注意**：生成型多模态按调用计费（如 0.2元/张图片），与 LLM 的按 token 计费模型不同。tenant 计量系统需要支持两种计费维度。

- [ ] **Step 4: 编译验证**

```bash
cargo check -p agent-core -p tenant
```

Expected: PASS

---

### Task 3.4: AGENTS.md 更新

**Files:**
- Modify: `AGENTS.md`

**Steps:**

- [ ] **Step 1: 在 "模块边界" 章节添加 media 模块说明**

```
crates/
  ai-provider/
    media/             # 生成型多模态抽象（MediaProvider trait、MediaRequest/Response）
```

- [ ] **Step 2: 在 "当前状态" 表格中更新多模态状态**

| 项目 | 状态 |
|---|---|
| 理解型多模态（Image/Video/Audio 输入） | ✅ 已支持 |
| 生成型多模态（MediaProvider） | ✅ 已支持 |

- [ ] **Step 3: 在 "关键约束" 中补充多模态安全约束**

```
- 生成型多模态工具（generate_media）的文件输出必须经过 PathGuard 校验，禁止写入 workspace 以外的路径。
- 媒体生成任务的 tracing span 必须携带 tenant_id 和 session_id。
```

---

### Task 3.5: 补充单元测试覆盖

**Files:**
- Modify: `crates/ai-provider/src/types.rs`
- Modify: `crates/ai-provider/src/models.rs`
- Modify: `crates/ai-provider/src/transform.rs`
- Modify: `crates/ai-provider/src/media/mod.rs`

**Steps:**

- [ ] **Step 1: Content serde 测试**

在 `types.rs` 的 `#[cfg(test)]` 中新增：

```rust
#[test]
fn test_content_video_audio_roundtrip() {
    let content = Content::Video { data: "base64vid".to_string(), mime_type: "video/mp4".to_string() };
    let json = serde_json::to_string(&content).unwrap();
    assert!(json.contains("\"type\":\"video\""));
    let back: Content = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, Content::Video { .. }));
}

#[test]
fn test_content_unknown_type_fallback() {
    let json = r#"{"type":"document","data":"xyz"}"#;
    let back: Content = serde_json::from_str(json).unwrap();
    assert!(matches!(back, Content::Text { text, .. } if text == "[unsupported content type: document]"));
}
```

- [ ] **Step 2: Modality serde 测试**

在 `models.rs` 的 `#[cfg(test)]` 中新增：

```rust
#[test]
fn test_modality_video_audio_serde() {
    let m = Modality::Video;
    let json = serde_json::to_string(&m).unwrap();
    assert_eq!(json, "\"Video\"");
    let back: Modality = serde_json::from_str(&json).unwrap();
    assert_eq!(back, Modality::Video);
}
```

- [ ] **Step 3: transform 降级测试**

在 `transform.rs` 的 `#[cfg(test)]` 中新增：

```rust
#[test]
fn test_video_audio_downgrade() {
    let messages = vec![Message::User(crate::UserMessage {
        content: vec![
            Content::Text { text: "look".into(), text_signature: None },
            Content::Video { data: "vid".into(), mime_type: "video/mp4".into() },
        ],
        timestamp: std::time::SystemTime::now(),
    })];
    let result = transform_messages(
        &messages,
        &TransformOptions {
            supports_video_input: false,
            supports_audio_input: false,
            preserve_thinking: true,
            ..Default::default()
        },
    );
    let user = match &result[0] { Message::User(m) => m, _ => panic!() };
    assert!(matches!(user.content[1], Content::Text { ref text, .. } if text == "[video: video/mp4]"));
}

#[test]
fn test_tool_result_media_downgrade() {
    let messages = vec![Message::ToolResult(crate::ToolResultMessage {
        tool_call_id: "tc1".into(),
        tool_name: "generate_media".into(),
        content: vec![
            Content::Text { text: "图片已保存".into(), text_signature: None },
            Content::Image { data: "base64img".into(), mime_type: "image/png".into() },
        ],
        details: None,
        is_error: false,
        timestamp: std::time::SystemTime::now(),
    })];
    let mut result = messages.clone();
    downgrade_tool_result_media(&mut result);
    let tr = match &result[0] { Message::ToolResult(m) => m, _ => panic!() };
    assert_eq!(tr.content.len(), 1);
    assert!(matches!(tr.content[0], Content::Text { ref text, .. } if text == "图片已保存\n[image: image/png]"));
}
```

- [ ] **Step 4: MediaModelRegistry 测试**

在 `media/registry.rs` 的 `#[cfg(test)]` 中新增（`build_default()` 定义在该文件中）：

```rust
#[test]
fn test_registry_model_lookup() {
    let registry = MediaModelRegistry::build_default();
    assert!(registry.get("doubao-seedream-5-0").is_some());
    assert!(registry.get("doubao-seedance-2-0").is_some());
    assert!(registry.get("nonexistent").is_none());
}
```

---

## 阻塞与风险

| 风险 | 缓解措施 |
|---|---|
| `Content` 新增变体导致 serde 反序列化失败 | 手写 `Deserialize` impl，未知 `type` 降级为 `Text` 占位符（Task 1.1 Step 3） |
| `AgentLoop` / `compaction` 中遗漏 Video/Audio match 分支 | Task 1.1 Step 5 全量扫描，编译器 exhaustive match 保证 |
| `AgentTool::execute` 签名变更影响所有 tool 实现 | Phase 0 Task 0.1 集中处理，机械增加参数，内部逻辑不变 |
| Media 模型与 LLM 模型注册表混淆 | 独立 `MediaModelRegistry`，`RouterProvider` 无需感知 media 模型 |
| Seedance API 文档细节与假设不符 | Phase 2.5 单独跟踪，先完成 Seedream（同步 API）验证架构 |
| 理解型多模态 base64 数据撑爆 SessionStore | API Gateway 限制单条消息大小（建议 ≤ 10MB）；compaction 对 Video/Audio 按 4800 token 估算仅为语义估算，不反映实际存储开销；未来中长期应将大媒体数据移出 SessionStore，改为文件引用 |
| 大文件（视频）内存占用 | `MediaProvider::download` 默认实现流式下载并检查 `max_file_size`；`MediaGenerationTool` 的 `save_media_to_workspace_bytes` 直接写入磁盘。注意：base64 Inline 解码仍需一次性读入内存。 |
| `api-gateway` 中遗漏的 `AgentTool` 实现未同步更新签名 | Task 0.1 前全局搜索 `impl AgentTool`，确认所有实现均已更新。当前代码库中 `AgentTool` 实现集中在 `agent-core` crate 内，无外部实现。 |
| SessionStore 新旧版本混跑导致反序列化失败 | 部署策略：滚动更新时确保所有节点同时升级（或先升级 session 存储格式版本号，旧节点拒绝读取新版本数据并返回明确错误）。 |
| TUI 客户端版本滞后，反序列化 `HistoricalContent` 失败 | API Gateway `GET /sessions/{id}/messages` 已默认降级（Task 1.5），旧版 TUI 不会收到未知变体。TUI 扩展 `HistoricalContent` 仅为正向能力增强。 |
