# 多模态模型接入规格书

**Date:** 2026-05-19
**Status:** Draft
**Reference:** AGENTS.md (ADR-001, ADR-003, ADR-004)

---

## 模块定位

本规格书定义 Pandaria  runtime 对**理解型多模态**（vision/audio input → text output）和**生成型多模态**（prompt → image/video/audio output）的接入标准。

- **理解型多模态**由 `ai-provider` 的 `LlmProvider` 承载，属于现有文本聊天架构的自然扩展。
- **生成型多模态**引入新的 `MediaProvider` trait，通过 `agent-core` 的 `AgentTool` 体系接入 Agent Loop。

## 依赖方向

```
api-gateway → tenant → agent-core → ai-provider
                           ↓
                      storage
```

`MediaProvider` 与 `LlmProvider` 处于同一层级（`ai-provider` crate），但二者互不依赖。

---

## 前置条件

在按本规格书实施多模态功能前，以下基础设施调整必须已完成：

1. **`AgentTool::execute` 签名已扩展 `CancellationToken`**  
   `MediaGenerationTool` 需要透传取消信号给异步媒体生成任务（尤其是 Seedance 视频生成的轮询逻辑）。该变更涉及 `agent-core/src/types.rs` 中 `AgentTool` trait 的签名调整，以及所有现有 tool 实现（含测试 mock）的机械适配。

2. **API Gateway / tenant / SessionActor / TUI 的消息发送协议已支持结构化 content**  
   当前整个调用链只接受纯文本：
   - `api-gateway/src/types.rs`: `SendMessageRequest { content: String }`
   - `api-gateway/src/routes/messages.rs`: `send()` handler 调用 `TenantManager::send_message(..., req.content)`（注意：send handler 在 `messages.rs`，`sessions.rs` 只有 `messages` GET 和 `compact` 端点）
   - `tenant/src/manager.rs`: `TenantManager::send_message(..., content: String)`
   - `agent-core/src/harness/session.rs`: `SessionActor::prompt(text: String)`
   - `tui/src/client/rest.rs`: `SendMessageRequest { content: content.to_string() }`
   - `api-gateway/tests/common/mod.rs`: mock `TenantManager::send_message(..., content: String)`

   理解型多模态要求客户端发送结构化消息（内联图片/视频/音频）。升级路径：
   - **方案 A（推荐）**：`api-gateway` 独立定义 `MessageContentPart` 枚举（保持 api-gateway 不直接依赖 ai-provider 类型的设计哲学），`tenant` 负责转换为 `Vec<ai_provider::Content>`，`SessionActor` 新增 `prompt_with_content(Vec<Content>)` 方法（原 `prompt(String)` 保留向后兼容，内部构造 `Content::Text` 后调用 `prompt_with_content`）。

     **方案 A 完整类型定义：**

     ```rust
     // api-gateway/src/types.rs
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

     // 转换逻辑（TenantManagerImpl::send_message 内部）
     fn convert_content(parts: Vec<MessageContentPart>) -> Vec<ai_provider::Content> {
         parts.into_iter().map(|p| match p {
             MessageContentPart::Text { text } => ai_provider::Content::Text { text, text_signature: None },
             MessageContentPart::Image { data, mime_type } => ai_provider::Content::Image { data, mime_type },
             MessageContentPart::Video { data, mime_type } => ai_provider::Content::Video { data, mime_type },
             MessageContentPart::Audio { data, mime_type } => ai_provider::Content::Audio { data, mime_type },
         }).collect()
     }
     ```

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

   - **方案 B（最简）**：`api-gateway` 直接复用 `ai_provider::Content`（Cargo.toml 中已有直接依赖，但 `types.rs` 设计哲学倾向于独立定义）。

   > 若 Phase 1 暂不升级 API Gateway，则理解型多模态的客户端输入必须推迟到 Phase 2；Phase 1 仅限 server-side 注入（如 tool result 返回 `Content::Image`）。

### 前置条件补充说明：ToolExecutor 与 CancellationToken

`AgentTool::execute` 添加 `CancellationToken` 参数后，`ToolExecutor::execute_tool_call` 和 `AgentLoop` 中调用工具执行的代码也需同步更新：

```rust
// agent-core/src/harness/tool.rs
pub(crate) async fn execute_tool_call(
    &self,
    tool_call: &ToolCall,
    on_progress: Option<&(dyn Fn(AgentToolProgressUpdate) + Send + Sync)>,
    signal: CancellationToken,  // 新增
) -> Result<ToolResultMsg, AgentError> {
    // ... 透传给 self.tool.execute(tool_call.id, tool_input, on_progress, signal)
}
```

```rust
// agent-core/src/harness/agent_loop.rs
let result = executor.execute_tool_call(tc, Some(&move |update| {
    // ... progress callback
}), signal.clone()).await;  // 透传 CancellationToken
```

### 前置条件实施计划

以下两条前置条件**必须作为独立 PR 在 multimodal 实施前合并**，避免单一 PR 体积失控：

| 前置条件 | 影响文件 | 估计改动量 | 独立 PR |
|---|---|---|---|
| `AgentTool::execute` 加 `CancellationToken` | `agent-core/src/types.rs`（trait 签名）<br>所有 `AgentTool` 实现（含测试 mock） | ~15 处机械适配 | **PR-1** |
| 消息协议结构化 | `api-gateway/src/types.rs`（`SendMessageRequest`）<br>`api-gateway/src/routes/messages.rs`（`send()` handler）<br>`tenant/src/manager.rs`（`send_message` 签名 + 转换）<br>`agent-core/src/harness/session.rs`（新增 `prompt_with_content`）<br>`tui/src/client/rest.rs`（`SendMessageRequest`）<br>`api-gateway/tests/common/mod.rs`（mock 适配） | ~6 个文件 | **PR-2** |

---

## Phase 划分

| Phase | 范围 | 交付物 | 前置条件 |
|---|---|---|---|
| **Phase 1** | 生成型多模态（`MediaProvider` + `MediaGenerationTool`）<br>理解型多模态 server-side 注入（tool result 返回 `Content::Image`） | `ai-provider/media/` 模块<br>`agent-core/skills/media_tool.rs`<br>`Content` 扩展 `Image`/`Video`/`Audio` + 自定义 Deserializer | PR-1（`CancellationToken`） |
| **Phase 2** | 理解型多模态客户端输入（API Gateway 结构化消息）<br>TUI 多媒体显示 | `api-gateway` `MessageContentPart` 协议<br>`TenantManager` 转换逻辑<br>TUI `HistoricalContent` 扩展 | PR-2（消息协议结构化） |
| **Phase 3** | 大媒体外移 + 结构化进度事件 + 参考图支持 | `AgentSpace` 媒体子目录<br>SessionStore 外移逻辑<br>`AgentEvent::MediaGenerationProgress` | Phase 1 & 2 完成 |

---

## 1. 术语

| 术语 | 定义 |
|---|---|
| **理解型多模态** | 模型接收文本+图片/视频/音频输入，输出文本（如 GPT-4o Vision、Gemini、豆包 seed-1.6-vision） |
| **生成型多模态** | 模型接收 prompt，输出图片/视频/音频文件（如 DALL-E、Seedance、Seedream） |
| **MediaProvider** | `ai-provider` 中新增的 trait，抽象生成型多模态的后端 |
| **MediaGenerationTool** | `agent-core` 中实现 `AgentTool` 的工具，封装 `MediaProvider` 调用 |
| **Modality** | 内容模态：Text / Image / Video / Audio |

---

## 2. 文件结构

### `crates/ai-provider/`

```
src/
  types.rs                  # Content 扩展
  models.rs                 # Modality 扩展
  transform.rs              # Video/Audio 跨 provider 降级（已有）
  media/
    mod.rs                  # MediaProvider trait, MediaTaskType, MediaRequest, MediaResponse
    error.rs                # MediaError (thiserror)
    task.rs                 # MediaTaskId, MediaTaskStatus, 轮询辅助
    registry.rs             # MediaModelRegistry, MediaModel
  providers/
    media_shared.rs         # define_media_provider! 宏（参考 shared.rs）
    doubao_media.rs         # DoubaoMediaProvider（Seedream + Seedance）
    openai_media.rs         # OpenAiMediaProvider（DALL-E，可选）
  lib.rs                    # 导出 media 模块（`pub mod media;` + `pub use media::*;`）
  Cargo.toml                # 确认已有 `tokio-util`、`reqwest`；`MediaError` 需 `thiserror`
```

### `crates/agent-core/`

> **注意**：当前 `agent-core/src/skills/` 只包含 skill 元数据类型，不包含可执行工具。`MediaGenerationTool` 作为首个生产级 `AgentTool` 实现，建议新增 `tools/` 目录：

```
src/
  tools/
    mod.rs                  # 导出所有工具
    media_generation.rs     # MediaGenerationTool（AgentTool 实现）
  skills/                   # 保持不变（skill 元数据）
```

若项目偏好不新增 `tools/` 目录，也可将 `MediaGenerationTool` 放入 `src/harness/media_generation.rs`（与 `ToolExecutor` 同目录），但 `skills/media_tool.rs` 不合适。

**依赖检查**：`MediaGenerationTool` 使用 `base64::engine::general_purpose::STANDARD.decode()` 解码 base64。`agent-core/Cargo.toml` 中**需新增 `base64` 依赖**：

```toml
[dependencies]
# ... 现有依赖 ...
base64 = { workspace = true }
```

---

## 3. 理解型多模态：类型系统扩展

### 3.1 Content 枚举扩展

```rust
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum Content {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },

    #[serde(rename = "image")]
    Image {
        data: String,      // base64-encoded
        mime_type: String,
    },

    // ── 新增 ──
    #[serde(rename = "video")]
    Video {
        data: String,      // base64-encoded video clip
        mime_type: String, // e.g. "video/mp4"
    },

    #[serde(rename = "audio")]
    Audio {
        data: String,      // base64-encoded audio
        mime_type: String, // e.g. "audio/wav", "audio/mp3"
    },
    // ── 新增结束 ──

    // 注意：为向前兼容，此处通过自定义 Deserialize 实现兜底，
    // 不在 enum 中声明 Unknown 变体，避免污染所有 match 分支。

    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
        #[serde(default)]
        redacted: bool,
    },

    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
}
```

**兼容性策略：**
- 旧版 `SessionStore` 数据反序列化时，若遇到未知的 `type` 字段，需要兜底处理。
- `#[serde(other)]` 在 externally tagged enum（`#[serde(tag = "type")]`）上无效，因此采用**自定义 deserializer**：未知 `type` 映射为 `Content::Text { text: "[unsupported content type: {type}]" }`，而非添加 `Unknown` 变体。
- 为避免修改所有 match `Content` 的代码，同时保证旧数据向前兼容，`Serialize` 继续使用 derive，`Deserialize` 手写实现。
- **字段缺失策略**：各分支使用 `unwrap_or("")` / `unwrap_or(false)` 等默认值处理缺失字段，确保部分损坏的数据仍可反序列化（静默降级），而非直接报错。这是有意为之的容错设计，与 `Unknown` 类型兜底保持一致。

**自定义 Deserializer 实现要点：**
```rust
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
                // ToolCall 结构体本身不含 `type` 字段，但 serde 默认忽略未知字段，
                // 因此将含 `"type": "toolCall"` 的 value 直接反序列化为 ToolCall 是安全的。
                // 注意：若未来 ToolCall 添加 #[serde(deny_unknown_fields)]，需先移除 `"type"` 字段。
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

### 3.2 Modality 枚举扩展

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Modality {
    Text,
    Image,
    Video,   // 新增
    Audio,   // 新增
}
```

### 3.2.1 模型元数据审核清单

添加 `Video`/`Audio` 变体后，需审核 `ai-provider/src/models_data.rs` 中所有现有模型的 `input_modalities` 字段，确保 capability 标注准确：

| 模型 | 当前 `input_modalities` | 应更新为 |
|---|---|---|
| GPT-4o 系列 | 仅 `Text` | `Text, Image`（vision 支持） |
| Gemini 系列 | 仅 `Text` | `Text, Image, Video, Audio`（原生多模态） |
| Claude Sonnet 4 | 仅 `Text` | `Text, Image`（vision 支持） |
| 豆包 seed-1.6-vision | 仅 `Text` | `Text, Image, Video`（如支持 video） |

> **实施要求**：`transform.rs` 中 `TransformOptions` 的 `supports_images` / `supports_video_input` / `supports_audio_input` 通过 `model_meta.input_modalities` 推导。若元数据标注错误，会导致不该降级的内容被降级，或向不支持的 provider 发送非法内容。

### 3.3 消息序列化（OpenAI-compatible）

`openai_compatible_stream` 中 `messages` 构建逻辑扩展，覆盖 **User 消息** 和 **ToolResult 消息** 两个来源：

**User 消息序列化：**

```rust
crate::Content::Video { data, mime_type } => serde_json::json!({
    "type": "video_url",
    "video_url": {
        "url": format!("data:{};base64,{}", mime_type, data),
    }
}),

crate::Content::Audio { data, mime_type } => serde_json::json!({
    "type": "input_audio",
    "input_audio": {
        "data": data,
        "format": mime_type.strip_prefix("audio/").unwrap_or("wav"),
    }
}),
```

**ToolResult 消息序列化：**

当前 `openai_compatible_stream` 的 ToolResult 构建逻辑只提取 `Content::Text`：

```rust
crate::Message::ToolResult(m) => serde_json::json!({
    "role": "tool",
    "tool_call_id": m.tool_call_id,
    "content": m.content.iter().filter_map(|c| match c {
        crate::Content::Text { text, .. } => Some(text.as_str()),
        _ => None,
    }).collect::<Vec<_>>().join("\n"),
}),
```

**问题**：如果 tool result 中包含 `Content::Image`/`Video`/`Audio`（如 `MediaGenerationTool` 内联返回小图片），上述逻辑会**完全丢弃**媒体内容，LLM 看不到生成的图片。

**解决方案（两步处理）**：

1. **`openai_compatible_stream` 不做协议层突破**：保持 `role: "tool"` 和字符串 `content`。OpenAI Chat Completions API 严格要求 `role: "tool"` 消息紧跟在对应的 assistant tool_call 之后，且 `content` 必须是字符串。将 `role: "tool"` 改为 `"user"` 会破坏协议约束，导致 API 拒绝请求。

2. **在 `transform.rs` 中预先降级**：`transform_messages()` 在调用 `openai_compatible_stream` 之前，已将 ToolResult 中的 `Content::Image`/`Video`/`Audio` 降级为文本描述：

```rust
// transform.rs —— 在 downgrade_video_audio 中统一处理
fn downgrade_tool_result_media(messages: &mut [Message]) {
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

> **跨 provider 差异说明**：Anthropic Messages API 原生支持在 tool result 中发送图片（通过 `role: "user"` 的 `tool_result` content block）。未来若需支持 Anthropic 的 tool result 图片传递，应在 `anthropic_messages_stream`（而非 `openai_compatible_stream`）中实现专门的序列化逻辑，而非在通用层做协议不兼容的转换。

- 火山引擎 Chat API 支持 `video_url`（公网 URL 或 base64 data URI）。火山引擎在 `video_url` 中额外支持 `fps` 字段控制抽帧频率；`DoubaoMediaProvider` 在其 `try_stream_inner` 覆盖逻辑中注入该字段，通用序列化不硬编码 provider 特有字段。
- OpenAI Responses API 支持 `input_video` / `input_audio`。
- 各 provider 的 `try_stream_inner` 可自行覆盖字段映射；`openai_compatible_stream` 提供跨 provider 通用默认值。

### 3.4 跨 Provider 降级（transform.rs）

**`TransformOptions` 新增字段：**

```rust
/// Options controlling message transformation behavior.
#[derive(Debug, Clone, Default)]
pub struct TransformOptions {
    pub target_api: Option<String>,
    pub supports_images: bool,
    /// 新增：目标模型是否支持 video 输入
    pub supports_video_input: bool,
    /// 新增：目标模型是否支持 audio 输入
    pub supports_audio_input: bool,
    pub preserve_thinking: bool,
}
```

> **推导逻辑**：`transform_messages()` 调用方（如 `AgentLoop` 或 provider 实现）应根据 `model_meta.input_modalities` 设置这三个布尔字段。例如：
> ```rust
> let opts = TransformOptions {
>     supports_images: model.input_modalities.contains(&Modality::Image),
>     supports_video_input: model.input_modalities.contains(&Modality::Video),
>     supports_audio_input: model.input_modalities.contains(&Modality::Audio),
>     ..Default::default()
> };
> ```

**降级函数实现：**

```rust
/// 若目标 provider 不支持 video/audio 输入，降级为文本占位符。
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

**`transform_messages()` 流水线更新：**

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
```

- `transform_messages()` 的 `TransformOptions` 通过 `model_meta.input_modalities` 推导 `supports_video_input` / `supports_audio_input`（模型粒度），比 provider 级别检测更准确。
- 不支持 video/audio 的 provider 在 `transform.rs` 中降级为 `[video: {mime_type}]` / `[audio: {mime_type}]` 文本占位符。
- ToolResult 中的媒体内容**无论目标 provider 是否支持 vision**，都先降级为文本描述。因为 OpenAI Chat Completions API 的 `role: "tool"` 不支持多模态数组，无法直接传递图片。

### 3.5 API Gateway 媒体输入协议

理解型多模态（Video/Audio 输入）的媒体数据通过现有 REST API 端点传输，**不新增专门的上传端点**。

**方案：内联 base64（Phase 1 采用）**

客户端在发送消息时，将视频/音频文件编码为 base64，通过 `POST /sessions/{id}/messages` 直接内联传输：

```json
{
  "role": "user",
  "content": [
    { "type": "text", "text": "分析这段视频" },
    { "type": "video", "data": "<base64-encoded-data>", "mime_type": "video/mp4" }
  ]
}
```

**前置条件**：本协议要求 API Gateway 的消息发送接口已升级为接受结构化 `Vec<Content>`（见「前置条件」第 2 条）。若接口仍为 `content: String`，客户端无法直接发送 Video/Audio 数据。

**约束：**
- HTTP 层：单条消息总大小受 Nginx/Load Balancer 的 `client_max_body_size` 限制（建议 ≥ 50MB，确保基础视频片段可传输）。
- 应用层：API Gateway 在解析消息前需额外限制单条消息中 media 内容的总大小（建议 ≤ 10MB），防止超大 base64 进入 `SessionStore` 导致 DB/内存爆炸。两个限制为互补关系：Nginx 负责网络层兜底，应用层负责业务级精细化控制。
- 对于超大文件（> 10MB），未来可考虑新增预签名 URL 上传端点（Phase 3 后评估）。

### 3.6 SessionStore 存储策略

理解型多模态的 base64 数据直接进入 `SessionStore`（PostgreSQL/Redis），带来存储和 context window 膨胀风险：

| 阶段 | 策略 | 说明 |
|---|---|---|
| Phase 1 | 直接内联 | Video/Audio base64 随消息序列化存入 `SessionStore`。单条消息大小由 API Gateway 限制（≤ 10MB）。 |
| Phase 3 | 大媒体外移 | 当 `SessionActor` 收到单条 media > 100KB 时，自动提取到 `AgentSpace::media_dir(tenant_id)`，`SessionStore` 中替换为 `Content::Text { text: "[media: /workspaces/{tenant_id}/media/...]" }`。自定义 Deserializer 需兼容历史内联数据。 |

**`AgentSpace` 扩展：**

```rust
impl AgentSpace {
    /// `{root}/workspaces/{tenant_id}/media/`
    pub fn media_dir(&self, tenant_id: &str) -> PathBuf {
        self.workspace_for(tenant_id).join("media")
    }
}
```

`ensure_dirs()` 不需要预先创建 `media` 子目录（每个租户独立，按需创建）。`MediaGenerationTool` 和 Phase 3 的外移逻辑均使用 `media_dir()` 而非直接拼接路径。

> **compaction 估算**：`Video`/`Audio` 按 4800 token/条估算（与 `Image` 同权），不反映实际存储开销。外移后 compaction 仍需按原始语义估算，或根据外移文件大小动态调整。

### 3.7 TUI 客户端兼容性

TUI 通过 `GET /sessions/{id}/messages` 获取历史消息并反序列化为 `HistoricalContent`。当前 TUI 使用 derive `Deserialize`，新增 `Image`/`Video`/`Audio` 变体后旧版 TUI 会反序列化失败。

**缓解措施：**

1. **API Gateway 响应降级（默认行为）**：在 `api-gateway/src/routes/sessions.rs` 的 `messages` 端点中，返回 `Json<Vec<AgentMessage>>` 前**默认**遍历消息内容，将 `Content::Image`/`Video`/`Audio` 降级为 `Content::Text { text: "[图片/视频/音频内容]" }`。降级只影响响应，**不修改** `SessionStore` 中的原始数据。

   未来若 TUI 支持多媒体显示，可通过 Query Param（如 `?format=raw`）或 HTTP Header（如 `X-Client-Supports-Multimedia`）请求未降级数据。

   ```rust
   // api-gateway/src/routes/sessions.rs
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

2. **TUI `HistoricalContent` 扩展（Phase 2）**：TUI 的 `HistoricalContent` 需同步扩展以支持 `Image`/`Video`/`Audio` 变体，或至少添加 `Image`：

   ```rust
   // tui/src/client/model.rs
   #[derive(Debug, Clone, Serialize, Deserialize)]
   #[serde(tag = "type")]
   pub enum HistoricalContent {
       #[serde(rename = "text")]
       Text { text: String },
       #[serde(rename = "image")]
       Image { data: String, mime_type: String },
       #[serde(rename = "video")]
       Video { data: String, mime_type: String },
       #[serde(rename = "audio")]
       Audio { data: String, mime_type: String },
       #[serde(rename = "thinking")]
       Thinking { thinking: String },
       #[serde(rename = "toolCall")]
       ToolCall { id: String, name: String, arguments: serde_json::Value },
   }
   ```

   > 注意：若旧版 TUI 未升级，`HistoricalContent` 新增变体仍会导致反序列化失败。因此**API Gateway 默认降级是必需的**，TUI 扩展仅作为正向能力增强。

---

## 4. 生成型多模态：MediaProvider

### 4.1 MediaProvider trait

```rust
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use crate::media::MediaError;

/// 生成型多模态任务的统一抽象。
/// 与 LlmProvider 平行，但语义完全不同：单次请求 → 异步任务 → 媒体结果。
#[async_trait]
pub trait MediaProvider: Send + Sync {
    fn provider_name(&self) -> &str;

    /// 该 provider 支持的任务类型列表
    fn supported_tasks(&self) -> Vec<MediaTaskType>;

    /// 执行一次媒体生成任务。
    ///
    /// 内部封装：创建任务 → 轮询状态 → 获取结果 → 超时/取消处理。
    /// 对调用方表现为单个 async 调用，隐藏异步任务的复杂性。
    async fn generate(
        &self,
        model: &str,
        request: MediaRequest,
        signal: CancellationToken,
    ) -> Result<MediaResponse, MediaError>;

    /// 下载远程媒体文件。
    ///
    /// 默认实现使用 provider 内部的 HTTP client 进行流式下载，并检查大小限制。
    /// 大小检查基于**实际接收字节数**，不依赖 `Content-Length`（某些 CDN 可能返回不准确的长度）。
    /// Provider 实现可覆盖此方法以使用自定义下载逻辑（如预签名 URL、内部 CDN）。
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

    /// 返回内部 HTTP client（供 `download` 默认实现使用）。
    ///
    /// 设计说明：当前所有 provider 实现均基于 `reqwest`，因此统一暴露 `&reqwest::Client`。
    /// 若未来引入非 reqwest 的 provider，可改为返回泛型 client 抽象或让 `download` 成为纯虚方法。
    fn client(&self) -> &reqwest::Client;
}
```

### 4.2 任务类型

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

### 4.3 请求类型

```rust
pub enum MediaRequest {
    ImageGeneration {
        prompt: String,
        size: Option<String>,          // e.g. "1024x1024", "1792x1024"
        style: Option<String>,         // e.g. "vivid", "natural"
        quality: Option<String>,       // e.g. "hd", "standard"
        n: Option<u32>,                // 生成数量（默认 1）
    },

    VideoGeneration {
        prompt: String,
        duration: Option<u32>,         // 秒
        resolution: Option<String>,    // e.g. "1080p", "720p", "480p"
        aspect_ratio: Option<String>,  // e.g. "16:9", "9:16", "1:1"
        /// 参考图 / 首帧 / 尾帧
        image_refs: Vec<MediaReference>,
    },

    AudioGeneration {
        prompt: String,
        voice: Option<String>,         // e.g. "alloy", "echo", "fable"
        format: Option<String>,        // e.g. "mp3", "wav", "ogg"
    },
}

pub struct MediaReference {
    pub url: String,
    pub mime_type: String,
}
```

### 4.4 响应类型

```rust
pub enum MediaResponse {
    /// 数据内联返回（base64 编码）
    Inline {
        data: String,
        mime_type: String,
    },
    /// 返回可下载的 URL（有有效期）
    Reference {
        url: String,
        mime_type: String,
    },
}
```

### 4.5 异步任务封装

对于异步任务型 API（如 Seedance），`MediaProvider::generate()` 内部封装轮询逻辑：

```rust
/// 异步任务句柄（仅供 Provider 内部实现使用，不暴露给调用方）。
pub(crate) struct MediaTaskHandle {
    pub task_id: String,
    pub model: String,
}

/// 异步任务状态（仅供 Provider 内部实现使用）。
pub(crate) enum MediaTaskStatus {
    Queued,
    Running,
    Completed,
    Failed { reason: String },
}
```

轮询策略：
- 初始间隔：1s
- 指数退避：1s → 2s → 4s → 8s → cap at 30s
- 最大轮询时间：由 `MediaProvider` 实现内部控制（如 5 分钟超时）
- 取消：通过 `CancellationToken` 立即停止轮询

---

## 5. 生成型多模态：工具层集成

### 5.1 MediaGenerationTool

> **前置条件**：本实现假设 `AgentTool::execute` 签名已扩展 `CancellationToken` 参数（见「前置条件」第 1 条）。若该变更尚未完成，需先同步更新 trait 定义及所有现有实现。

```rust
use std::sync::Arc;
use async_trait::async_trait;
use ai_provider::media::{MediaProvider, MediaRequest, MediaResponse, MediaTaskType};
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
    /// 若用户未指定 model，则使用 default_model；若 default_model 不支持该 media_type，
    /// 则查询 registry 自动选择第一个支持的模型。
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
                // data 为 base64 编码字符串；data.len() 为 base64 字符串字节长度
                // 1MB 原始数据 ≈ 1.33MB base64 字符串，此处按字符串长度简单估算
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

        // 注入成本信息（成本由 MediaModelRegistry 元数据决定，非 MediaProvider 实时计算）
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
        use crate::space::AgentSpace;

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

### 5.2 结果处理策略

| 场景 | 处理方式 | 返回给 LLM |
|---|---|---|
| 图片 Inline (< 1MB) | 直接放入 `Content::Image` | base64 图片，LLM 后续可引用 |
| 图片 URL / 大文件 | 下载后写入 `AgentSpace::workspace_for(tenant_id)` | `Content::Text { text: "图片已保存至 /workspaces/..." }` |
| 视频生成 | 始终写入 workspace（视频通常 > 1MB） | `Content::Text { text: "视频已保存至 /workspaces/..." }` |
| 音频生成 | 写入 workspace | `Content::Text { text: "音频已保存至 /workspaces/..." }` |

**PathGuard 自动生效**：`MediaGenerationTool` 写入的路径在 `AgentSpace::workspace_for(tenant_id)` 下，`DefaultHookDispatcher::PathGuard` 无需额外配置即可拦截越界访问。

**跨轮引用说明**：`Content::Image` 内联返回仅适用于单轮对话。LLM 后续 turn 中如需引用已生成的图片，应通过文件路径使用文件读取工具（如 `read_file`），而不是依赖 base64 长期留在上下文中。随着对话进行，内联 base64 会迅速耗尽 context window（compaction 按 4800 token/张估算）。

**参考图（image_refs）说明**：`MediaRequest::VideoGeneration` 支持传入 `image_refs`（首帧/尾帧/参考图），但 `generate_media` 工具的 JSON Schema 在 Phase 2 暂不暴露该参数。如需使用参考图功能，可在后续迭代中扩展 Schema 和参数解析逻辑。

### 5.3 进度回调

```rust
if let Some(cb) = on_progress {
    cb(AgentToolProgressUpdate {
        content: "图片生成中...".to_string(),
    });
}
```

- 当前 `AgentToolProgressUpdate { content: String }` 已够用。
- 未来如需结构化进度（percent / stage），可扩展为 `AgentToolProgressUpdate { content: String, stage: Option<String>, percent: Option<u8> }`。

---

## 6. Hook 系统兼容性

生成型多模态通过 `MediaGenerationTool` 接入，现有 Hook 自动覆盖：

| Hook | 对 MediaGenerationTool 的作用 |
|---|---|
| `on_tool_call` | 可拦截 `generate_media` 调用（配额检查、关键词过滤、Block） |
| `on_tool_result` | 可审计生成记录（文件路径、mime_type、成本），但**无法直接审计像素/音频内容**（除非 Hook 实现主动读取文件） |
| `on_tool_execution_start/end` | 记录生成耗时、成本 |
| `on_turn_end` | 统计每轮 media 生成次数 |

**无需新增 Hook 类型。**

> **审计边界说明**：`MediaGenerationTool` 返回给 LLM 的是文本描述（如"视频已保存至 /workspaces/..."），实际媒体文件在磁盘上。`on_tool_result` 的 `ToolResultCtx.content` 中只包含文本，不包含二进制媒体数据。若业务需要内容级审计（如图片合规检测），需在 `on_tool_result` Hook 中根据 `details` 中的文件路径主动读取并分析。

---

## 7. 模型注册表

### 7.1 设计原则

生成型多模态模型（MediaProvider）与理解型多模态模型（LlmProvider）使用**独立注册表**，避免污染 LLM 路由逻辑。

- **LLM 注册表**（已有）：`ModelRegistry` / `models_data.rs`，管理文本/理解型多模态模型。
- **Media 注册表**（新增）：`MediaModelRegistry`，管理图片/视频/音频生成模型。

### 7.2 MediaModel 结构

```rust
#[derive(Debug, Clone)]
pub struct MediaModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    /// Provider 基础 URL（元数据用途）。
    /// 实际 HTTP 请求的 endpoint 由 `MediaProvider` 实现内部决定，不强制使用此字段。
    pub base_url: String,
    pub supported_tasks: Vec<MediaTaskType>,
    /// 按调用计费（如 0.2元/张图片），不按 token
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

### 7.3 使用示例

```rust
use ai_provider::media::MediaModelRegistry;

let registry = Arc::new(MediaModelRegistry::build_default());
```

### 7.4 成本模型说明

- 生成型多模态通常按**调用次数/输出量**计费（如 0.2元/张图片），不按 token。
- `MediaModel` 使用 `cost_per_call: Option<f64>` 记录单次调用成本，与 LLM 的 `TokenCost` 完全分离。
- 当前版本成本由 `MediaModelRegistry` 的静态元数据决定（`MediaGenerationTool` 从 registry 读取并注入 `details`），非 `MediaProvider` 实时计算。未来如需要动态成本（如按输出分辨率计费），可在 `MediaProvider::generate` 返回的 `MediaResponse` 中增加 `cost: Option<MediaCost>`。
- 成本通过 `MediaGenerationTool` 的 `on_tool_execution_end` Hook 记录到 tenant 计量系统。

**计量接口现状与扩展建议：**

经审查，`tenant/src/meter.rs` 当前仅提供 `SlidingWindowMeter`（滑动窗口计数器），**不支持按调用计费（per-call billing）**。需扩展 tenant 层计量接口：

```rust
// tenant/src/meter.rs（建议新增）
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-tenant cumulative cost tracker.
pub struct CostTracker {
    media_cost_cny: AtomicU64,  // 以 0.001 CNY 为最小单位，避免浮点精度问题
    llm_cost_cny: AtomicU64,
}

impl CostTracker {
    pub fn record_media_call(&self, cost_cny: f64) {
        let micros = (cost_cny * 1000.0) as u64;
        self.media_cost_cny.fetch_add(micros, Ordering::Relaxed);
    }

    pub fn media_cost_cny(&self) -> f64 {
        self.media_cost_cny.load(Ordering::Relaxed) as f64 / 1000.0
    }
}
```

> **成本记录实现路径（修正）**：
>
> 由于 `on_tool_execution_end` 当前未被调用，成本记录必须在 `on_tool_result` 中完成：
>
> ```rust
> // DefaultHookDispatcher 扩展
> #[derive(Debug)]
> pub struct DefaultHookDispatcher {
>     // ... 现有字段 ...
>     /// 可选：媒体成本回调 (tenant_id, cost_cny)
>     pub cost_callback: Option<Arc<dyn Fn(&str, f64) + Send + Sync>>,
> }
>
> #[async_trait]
> impl HookDispatcher for DefaultHookDispatcher {
>     // ...
>
>     async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
>         // 现有 PathGuard / ContentFilter 逻辑 ...
>
>         // 媒体成本记录
>         if let Some(ref cb) = self.cost_callback {
>             if let Some(ref details) = ctx.details {
>                 if let Some(cost) = details.get("cost_per_call").and_then(|v| v.as_f64()) {
>                     cb(&ctx.tenant_id, cost);
>                 }
>             }
>         }
>
>         ToolResultMutation::default()
>     }
> }
> ```
>
> ```rust
> // TenantManagerImpl::create_session 中注入回调
> let cost_tracker = Arc::new(CostTracker::new());
> let cost_tracker_clone = cost_tracker.clone();
> let mut dispatcher = DefaultHookDispatcher::new();
> dispatcher.cost_callback = Some(Arc::new(move |tenant_id, cost| {
>     cost_tracker_clone.record_media_call(cost);
>     tracing::info!(tenant_id, cost, "media cost recorded");
> }));
> let hook_dispatcher = Arc::new(dispatcher) as Arc<dyn HookDispatcher>;
> ```
>
> `TenantMetrics` 需新增 `media_cost_cny` 指标，与 LLM token 成本分项展示。

---

## 8. 事件层（可选扩展）

### 8.1 AgentEvent 扩展

```rust
pub enum AgentEvent {
    // ... existing variants ...

    /// 媒体生成任务进度（可选，Phase 3）
    MediaGenerationProgress {
        tool_call_id: String,
        stage: String,      // "queued" | "running" | "downloading" | "completed"
        percent: Option<u8>,
    },
}
```

- **非必需**。当前 `ToolExecutionUpdate { content: String }` 已足够表达进度。
- 如 TUI 需要结构化进度条，可在 Phase 3 引入。

### 8.2 SSE 事件流中的多媒体内容传递

**当前问题**：`api-gateway/src/routes/events.rs` 的 `map_agent_event` 将 `AgentEvent::ToolExecutionEnd` 映射为 `ServerEvent::ToolCallDone` 时，通过 `extract_text_content` 只提取 `Content::Text`，丢弃 `Image`/`Video`/`Audio`：

```rust
fn extract_text_content(contents: &[Content]) -> Option<String> {
    let texts: Vec<String> = contents
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    if texts.is_empty() { None } else { Some(texts.join("")) }
}
```

这意味着客户端通过 SSE 接收到的 `ToolCallDone` 事件中**不包含任何多媒体数据**。

**设计决策（SSE 不传输二进制媒体）**：

SSE（Server-Sent Events）不适合传输大体积 base64 数据（可能达数 MB）。因此：

1. **SSE 流只传递文本描述**：`extract_text_content` 改进为 `extract_tool_result_text`，将非文本内容转换为文本占位符，确保客户端知道有媒体内容但不在 SSE 中传输：

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

2. **客户端通过历史消息 API 获取完整内容**：TUI 在收到 `ToolCallDone` 后，如需显示图片/视频/音频，应调用 `GET /sessions/{id}/messages` 获取完整消息历史（含 `Content::Image`/`Video`/`Audio` 的原始数据）。该端点已由 API Gateway 默认降级保护旧版客户端，新版客户端可通过 `?format=raw` 获取未降级数据。

3. **未来扩展（Phase 3 后评估）**：如需在 SSE 中实时传输多媒体，可考虑：
   - 新增 `ToolCallMedia { call_id, mime_type, data }` SSE 事件类型
   - 或使用 WebSocket 替代 SSE 进行二进制流传输
   - 但当前 REST + SSE 架构下，历史消息 API 轮询是更简单的方案

> **对 TUI 的影响**：当前 TUI 的 `ServerEvent::ToolCallDone { result: Option<String> }` 结构保持不变。TUI 收到 `[image: image/png]` 占位符后，可选择性地调用历史消息 API 获取 base64 数据并渲染。Phase 2 中 TUI 扩展 `HistoricalContent` 枚举后，即可支持内联图片显示。

---

## 9. 安全约束

1. **API Key 不进入日志**：`MediaProvider` 使用 `secrecy::SecretString`，与 `LlmProvider` 一致。`DoubaoMediaProvider` 复用现有的 `DOUBAO_API_KEY` 环境变量，无需额外配置。
2. **生成内容审计**：`on_tool_result` hook 可拦截并审计生成的媒体记录（URL、文件路径、成本），但**不直接包含二进制内容**。
3. **路径沙箱**：`MediaGenerationTool` 写入文件必须在 `AgentSpace::workspace_for(tenant_id)` 下。
4. **大文件限制**：视频生成结果可能达几十 MB，需配置单文件大小上限（如 100MB），超限则返回错误。`MediaProvider::download` 默认实现已内置流式下载和大小检查。
5. **租户隔离**：`MediaProvider` 的 tracing span 必须携带 `tenant_id` 和 `session_id`。
6. **理解型多模态消息大小限制**：Video/Audio 的 base64 数据会进入 `SessionStore`。需在 API Gateway 层限制单条消息大小（建议 ≤ 10MB），防止 DB/内存爆炸。

---

## 10. 错误处理

### 10.1 MediaError

```rust
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

- `MediaProvider` 直接返回 `MediaError`，由 `MediaGenerationTool` 在边界处转换为 `AgentError::ToolExecutionFailed`。
- 这样 `ai-provider` 内部 `LlmProvider` 与 `MediaProvider` 的错误类型互不污染。

### 10.2 异步任务错误映射

| Seedance API 错误 | MediaError |
|---|---|
| `status: "failed"` | `TaskFailed(reason)` |
| 轮询超时 | `Timeout(duration)` |
| 取消信号 | `Cancelled` |
| 结果下载失败 | `DownloadFailed(err)` |

**Seedream（同步图片生成）错误映射：**

| Seedream API 错误 | MediaError |
|---|---|
| API 返回 4xx/5xx | `TaskFailed(status_text)` |
| 响应解析失败 | `TaskFailed(parse_error)` |

---

## 11. Provider 实现细节

### 11.1 DoubaoMediaProvider

**图片生成（Seedream）**

```
POST https://ark.cn-beijing.volces.com/api/v3/contents/generations
Header: Authorization: Bearer <key>
Body: {
  "model": "doubao-seedream-5-0",
  "prompt": "...",
  "width": 1024,
  "height": 1024
}
```

- 响应通常同步返回图片 URL（非异步任务）。
- 典型响应：
  ```json
  { "data": [{ "url": "https://.../image.png", "revised_prompt": "..." }] }
  ```
- `generate()` 直接 POST → 解析响应 → 提取 `data[0].url` → 返回 `MediaResponse::Reference { url, mime_type: "image/png" }`。
- `size` 字段（如 `"1024x1024"`）需在 Provider 实现中解析为 `width`/`height` 整数后填入请求体。

**视频生成（Seedance）**

```
POST https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks
→ { task_id: "..." }

GET https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks/{task_id}
→ { status: "queued" | "running" | "completed" | "failed", result: { url: "..." } }
```

- `generate()` 内部：POST 创建 → 轮询 GET → 下载结果 → 返回 `MediaResponse::Reference`。
- 轮询间隔：1s → 2s → 4s → 8s → cap 30s。

### 11.2 OpenAiMediaProvider（可选）

**图片生成（DALL-E 3）**

```
POST https://api.openai.com/v1/images/generations
Body: { "model": "dall-e-3", "prompt": "...", "size": "1024x1024" }
→ { data: [{ url: "...", revised_prompt: "..." }] }
```

- 同步 API，无异步任务轮询。

---

## 12. 测试策略

### 12.1 单元测试

| 模块 | 测试内容 |
|---|---|
| `Content` serde | Video/Audio 序列化/反序列化 round-trip |
| `transform.rs` | video downgrade 逻辑（支持/不支持 provider） |
| `MediaProvider` | `supported_tasks()` 返回正确列表 |
| `MediaProvider::download` | mock HTTP 测试：流式下载、大小限制、取消信号 |
| `DoubaoMediaProvider` | mock HTTP 测试：创建任务 → 轮询 → 获取结果 |
| `MediaGenerationTool` | mock `MediaProvider` 测试参数解析和结果转换 |

### 12.2 集成测试

- `test_mock_image_generation`：wiremock 模拟 Seedream API，验证完整流程。
- `test_mock_video_generation_async`：wiremock 模拟 Seedance 异步任务 API，验证轮询逻辑。
- `test_path_guard_on_media_tool`：验证生成的文件路径在 workspace 内。

### 12.3 向后兼容测试

- `test_legacy_tui_with_multimedia_gateway`：启动完整服务栈，使用未升级的旧版 TUI 客户端连接已启用 multimodal 的 API Gateway，验证基本消息发送和接收功能正常（依赖 API Gateway 默认降级）。
- `test_old_session_store_deserialization`：构造含 `Video`/`Audio` 变体的 `SessionStore` 序列化数据，验证旧版 `Content` derive `Deserialize` 在自定义 Deserializer 替换前的兼容性（或验证自定义 Deserializer 正确兜底）。
- `test_transform_options_backward_compat`：验证未设置 `supports_video_input`/`supports_audio_input` 的 `TransformOptions`（默认值 `false`）会正确降级 Video/Audio 内容，不会意外发送到不支持的 provider。
