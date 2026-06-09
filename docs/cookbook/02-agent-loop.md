# 第二章：Agent Loop 与 Tool Use

> **目标读者**：想理解 agent 内部如何驱动多轮对话、工具调用、上下文压缩的开发者。  
> **前提**：已读第一章（核心架构），了解 LLM tool calling 协议。

---

## 2.1 Agent Loop 协议

Pandaria 的 agent loop 基于 LLM 原生 tool calling：

```
UserMessage
    │
    ▼
┌──────────────────────────────────────────────────┐
│                  AgentLoop::run()                 │
│                                                  │
│  ① LLM 返回 AssistantMessage { ToolCall[] }      │
│  ② ToolExecutor 并行执行所有 ToolCall             │
│  ③ 结果注入为 ToolResultMessage[]                │
│  ④ 循环回到 ①，直到 stop_reason = "stop"         │
│                                                  │
│  单个 turn = 一次 LLM 响应（可能含 N 个 ToolCall）  │
│  agent loop = 多个 turn 的迭代直到 LLM 说 stop     │
└──────────────────────────────────────────────────┘
    │
    ▼
Final AssistantMessage (text response)
```

**关键特性**：

- **原生 Tool Use**：LLM 直接返回 `ToolCall[]`（不是 Pandaria 自己解析文本），与 Anthropic/OpenAI 的 tool use API 完全对齐
- **并行执行**：单个 `AssistantMessage` 内的所有 `ToolCall` 通过 `ToolExecutor` 并行执行（`tokio::spawn` 每个 tool call）
- **多轮迭代**：工具执行结果 → 下一轮 LLM 调用 → 可能的更多工具调用 → ... → 最终文本回复
- **终止条件**：LLM 返回 `stop_reason = "stop"` 或达到最大 turn 限制

---

## 2.2 完整执行流

```
SessionActor::prompt(user_message)
  │
  ├─→ 1. on_before_agent_start()  ← hook（链式合并）
  │      └─ MemoryHook::recall()  ← 从 Emerald 拉取用户记忆/画像
  │
  ├─→ 2. PromptBuilder::build()   ← 组装 system prompt + 历史 + user message
  │
  ├─→ 3. AgentLoop::run()
  │      │
  │      ├─→ on_before_provider_request()  ← hook（链式合并）
  │      ├─→ LlmProvider::stream()         ← 调用 LLM
  │      ├─→ on_after_provider_response()  ← hook（链式合并）
  │      │
  │      ├─→ 如果有 ToolCall:
  │      │     ├─→ on_tool_call()     ← 阻断型 hook（first-block-wins）
  │      │     ├─→ ToolExecutor::execute()  ← 并行执行所有工具
  │      │     │     └─→ 每个工具: AgentTool::execute()
  │      │     └─→ on_tool_result()   ← 链式 hook
  │      │
  │      └─→ 如果 stop_reason = "stop":
  │            └─→ 退出 loop，返回最终消息
  │
  ├─→ 4. on_turn_end()  ← 观测型 hook
  │      └─ MemoryHook::remember()  ← 保存对话到 Emerald
  │
  └─→ 5. 返回 AssistantMessage 给调用方
```

---

## 2.3 ToolExecutor：并行工具执行

当 LLM 在一次响应中返回多个 `ToolCall`（例如「读文件 A + 搜索 B」），`ToolExecutor` 并行执行：

```rust
// 伪代码：agent_loop.rs 中的工具执行逻辑

let tool_calls = assistant_message.tool_calls();

// 并行执行所有工具调用
let mut handles = Vec::new();
for tool_call in &tool_calls {
    let executor = ToolExecutor::new(
        tenant_id.clone(),
        session_id.clone(),
        hook_dispatcher.clone(),
        tool.clone(),
    );
    handles.push(tokio::spawn(async move {
        executor.execute(tool_call).await
    }));
}

// 等待全部完成
let results = futures::future::join_all(handles).await;
```

**ToolExecutor 内部流程**：

```
execute(tool_call)
  │
  ├─→ 1. on_tool_call()  ← 阻断型 hook
  │      ├─ PathGuard: 检查路径是否在 workspace 内
  │      ├─ ToolGuard: 检查工具是否在 allow/deny list
  │      └─ 若任一返回 Block → 立即返回错误，不执行工具
  │
  ├─→ 2. AgentTool::execute(input)
  │      └─ 实际工具逻辑（如读取文件、HTTP 请求）
  │
  ├─→ 3. on_tool_result()  ← 链式 hook
  │      └─ ContentFilter: PII 脱敏
  │
  └─→ 4. 返回 AgentToolResult
```

---

## 2.4 Compaction（上下文压缩）

当对话历史超过 token 预算时触发自动压缩：

```
CompactionActor::compact(messages, max_tokens)
  │
  ├─→ on_before_compact()  ← 阻断型 hook
  │      └─ 返回 Block → 取消压缩
  │
  ├─→ 调用 LLM 将历史消息压缩为摘要
  │      └─ 使用专门的 compaction prompt
  │
  ├─→ 将被压缩的消息替换为 CompactionSummary
  │
  └─→ 更新 session 的消息历史
```

**压缩保留策略**：
- 系统 prompt 保持不动
- 最近 N 轮消息保持完整（可配置）
- 早期消息被摘要替代
- `CompactionSummary` 作为特殊消息类型保留

---

## 2.5 Prompt 构建流程

`PromptBuilder` 负责在每次 LLM 调用前组装 prompt：

```
PromptBuilder::build(session_context)
  │
  ├─→ 1. System Prompt Template
  │      ├─ 注入 tools 的 name/description/parameters
  │      ├─ 注入 skills 目录
  │      ├─ 注入 memory context（Emerald recall 结果）
  │      └─ 注入用户自定义 instructions
  │
  ├─→ 2. 消息历史
  │      ├─ 完整的最近 N 轮消息
  │      ├─ CompactionSummary（如果做过压缩）
  │      └─ 当前 user message
  │
  └─→ 3. 通过 on_context() hook（链式合并）
         └─ ContextMutation 可追加/替换内容
```

---

## 2.6 事件系统

Session 内所有关键事件通过 `tokio::mpsc` 通道发送：

```rust
pub enum AgentEvent {
    TurnStarted { turn_index: u32 },
    ToolCallStarted { tool_name: String, input: Value },
    ToolCallCompleted { tool_name: String, result: AgentToolResult },
    ToolCallFailed { tool_name: String, error: String },
    CompactionTriggered { reason: String },
    CompactionCompleted { original_count: usize, compacted_count: usize },
    TurnCompleted { turn_index: u32, usage: Usage },
    SessionError { error: AgentError },
    SessionCompleted { total_turns: u32, total_tokens: u64 },
}
```

外部使用者可通过 SSE 订阅这些事件（`GET /api/v1/sessions/{id}/events/stream`）。

---

## 2.7 AgentTool 接口

Pandaria 内部工具必须实现的 trait：

```rust
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Vec<ToolParameter>;
    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, AgentToolError>;
}

pub struct ToolContext {
    pub tenant_id: String,
    pub session_id: String,
    pub abort_token: CancellationToken,  // 用于中断长时间运行的工具
}
```

通过 `PawbunToolAdapter`（[第五章](./05-integration.md)），Pawbun 的 `Tool` / `AsyncTool` trait 实现也可以无缝接入。

---

## 2.8 下一步

- 理解 hook 的三种调用模式 → [第三章：Hook 系统](./03-hooks.md)
- 理解工具如何接入 → [第五章：集成指南](./05-integration.md)
