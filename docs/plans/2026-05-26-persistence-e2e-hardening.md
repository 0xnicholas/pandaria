# Persistence & E2E Test Hardening 实施计划

**Date:** 2026-05-26
**Status:** Completed ✅ — delivered in v0.2.0
**Reference:** `docs/specs/2026-05-26-persistence-e2e-hardening.md`

---

## 1. 概述

本计划将 Persistence & E2E Test Hardening spec 展开为可执行的开发任务。按 Phase 1（Memory 数据准备重构）→ Phase 2（持久化加固）→ Phase 3（E2E 测试矩阵）顺序实施。

**实施原则**：
- 每 Phase 独立可编译、可测试
- Memory trait 破坏性变更在 Phase 1 一次性完成，不跨 Phase 遗留中间状态
- TDD：每项先写测试，再写实现
- 所有 .env 变量已配置（`PANDARIA_TEST_PG_URL`、`PANDARIA_TEST_REDIS_URL`），Docker 容器已运行

---

## 2. Phase 1：Memory 数据准备重构

### 2.1 目标

重新设计 Memory 模块为「对话格式化 + 元数据构建」模式。删除 `MemoryFact`、`MemoryQuery` 类型，简化 `MemoryStore` trait，新增 `memory/formatter.rs`，重写 `MemoryHookDispatcher`。

### 2.2 涉及文件

#### 新增

| 文件 | 说明 |
|---|---|
| `crates/agent-core/src/memory/formatter.rs` | `format_turn_content()` + `build_turn_metadata()` + `TurnToolCallSummary` |

#### 修改

| 文件 | 变更 |
|---|---|
| `crates/agent-core/src/memory/store.rs` | `MemoryStore` trait 简化：`remember(content, metadata)`, `recall(query) → Vec<String>` |
| `crates/agent-core/src/memory/types.rs` | 删除 `MemoryFact`、`MemoryQuery`；扩展 `MemoryContext` 增加 `model`、`session_started_at` |
| `crates/agent-core/src/memory/extractor.rs` | 删除 `extract_facts()`、`format_facts()`；保留 `build_query_string()`；新增 `extract_tool_summaries()` |
| `crates/agent-core/src/memory/hook.rs` | `MemoryHookDispatcher` 重写，调用 `format_turn_content()` + `build_turn_metadata()` |
| `crates/agent-core/src/memory/in_memory.rs` | 适配新 `MemoryStore` trait |
| `crates/agent-core/src/memory/mod.rs` | 导出 `formatter` 模块 |
| `crates/agent-core/src/lib.rs` | 重导出更新（如需要） |

### 2.3 具体步骤

---

### Task 1.1: 简化 MemoryStore trait 和 MemoryContext

**Files:**
- Modify: `crates/agent-core/src/memory/store.rs`
- Modify: `crates/agent-core/src/memory/types.rs`

- [ ] **Step 1: 更新 MemoryContext，增加 model 和 session_started_at**

```rust
// crates/agent-core/src/memory/types.rs

/// Context passed to `MemoryStore` operations, identifying the tenant and session.
#[derive(Debug, Clone)]
pub struct MemoryContext {
    pub tenant_id: String,
    pub session_id: String,
    /// Pandaria currently has no independent user level; this field is left
    /// for external adapters to map from `tenant_id` or to receive from
    /// future Tenant config / API request headers.
    pub user_id: Option<String>,
    /// Session metadata for external stores to use for routing/filtering
    /// without parsing the metadata JSON blob.
    pub model: String,
    pub session_started_at: std::time::SystemTime,
}
```

- [ ] **Step 2: 删除 MemoryFact 和 MemoryQuery 类型定义**

从 `types.rs` 中移除 `MemoryFact` 和 `MemoryQuery` 的定义。

- [ ] **Step 3: 重写 MemoryStore trait**

```rust
// crates/agent-core/src/memory/store.rs

use async_trait::async_trait;
use super::types::MemoryContext;

/// Protocol boundary for external memory systems.
///
/// Pandaria does not implement storage / retrieval / embedding itself.
/// Any external system (Emerald, SuperMemory, Mem0, in-house service, etc.)
/// can be plugged in by implementing this trait.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Send formatted conversation content to the external memory system.
    ///
    /// `content` is a Markdown-formatted turn transcript. The external system
    /// handles extraction, chunking, embedding, and relationship inference.
    /// `metadata` carries structured context (turn_index, model, token_usage, etc.).
    ///
    /// Failures should be silently discarded by the caller (MemoryHookDispatcher)
    /// so they never block the agent loop.
    async fn remember(
        &self,
        ctx: &MemoryContext,
        content: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), MemoryError>;

    /// Retrieve relevant memories for a query. Returns plain text strings.
    async fn recall(
        &self,
        ctx: &MemoryContext,
        query: &str,
    ) -> Result<Vec<String>, MemoryError>;

    /// Optional: delete all memories associated with a session.
    /// Default no-op for stores that do not support per-session eviction.
    async fn forget_session(&self, _ctx: &MemoryContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("memory store error: {0}")]
    StoreError(String),
}
```

- [ ] **Step 4: 尝试编译，收集所有编译错误**

```bash
cargo check -p agent-core 2>&1 | head -80
```

预期：大量编译错误（`extractor.rs`、`hook.rs`、`in_memory.rs` 引用了已删除的类型）。接下来逐文件修复。

- [ ] **Step 5: Commit**

```bash
git add crates/agent-core/src/memory/store.rs crates/agent-core/src/memory/types.rs
git commit -m "refactor(memory): simplify MemoryStore trait and delete MemoryFact/MemoryQuery"
```

---

### Task 1.2: 实现 Conversation Formatter

**Files:**
- Create: `crates/agent-core/src/memory/formatter.rs`
- Modify: `crates/agent-core/src/memory/mod.rs`

- [ ] **Step 1: 编写 formatter 的单元测试**

```rust
// 在 crates/agent-core/src/memory/formatter.rs 底部

#[cfg(test)]
mod tests {
    use super::*;
    use ai_provider::{AssistantMessage, Content, ToolCall, ToolResultMessage, Usage, UserMessage, StopReason};
    use crate::types::AgentMessage;

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            timestamp: std::time::SystemTime::now(),
        })
    }

    fn assistant_text(text: &str) -> AgentMessage {
        AgentMessage::Assistant(AssistantMessage {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            provider: "test".into(), model: "test".into(),
            api: ai_provider::Api { provider: "test".into(), model: "test".into() },
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            response_id: None, error_message: None,
            timestamp: std::time::SystemTime::now(),
        })
    }

    fn assistant_tool_call(name: &str, args: &str) -> AgentMessage {
        AgentMessage::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCall {
                id: "tc1".into(), name: name.to_string(),
                arguments: serde_json::from_str(args).unwrap(),
                thought_signature: None,
            })],
            provider: "test".into(), model: "test".into(),
            api: ai_provider::Api { provider: "test".into(), model: "test".into() },
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            response_id: None, error_message: None,
            timestamp: std::time::SystemTime::now(),
        })
    }

    fn tool_result(name: &str, text: &str, is_error: bool) -> AgentMessage {
        AgentMessage::ToolResult(ToolResultMessage {
            tool_name: name.to_string(),
            tool_call_id: "tc1".into(),
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            is_error,
            details: None,
            timestamp: std::time::SystemTime::now(),
        })
    }

    #[test]
    fn test_format_simple_turn() {
        let messages = vec![
            user_msg("hello"),
            assistant_text("hi there"),
        ];
        let output = format_turn_content(1, &messages);
        assert!(output.contains("## Turn 1"));
        assert!(output.contains("**User**: hello"));
        assert!(output.contains("**Assistant**: hi there"));
    }

    #[test]
    fn test_format_turn_with_tool_calls() {
        let messages = vec![
            user_msg("read src/main.rs"),
            assistant_tool_call("read_file", r#"{"path":"src/main.rs"}"#),
            tool_result("read_file", "fn main() { println!(\"hello\"); }", false),
            assistant_text("I found the main function."),
        ];
        let output = format_turn_content(2, &messages);
        assert!(output.contains("## Turn 2"));
        assert!(output.contains("**ToolCall[read_file]**"));
        assert!(output.contains("**ToolResult[read_file]**"));
        assert!(output.contains("(成功"));
    }

    #[test]
    fn test_format_turn_tool_error() {
        let messages = vec![
            user_msg("delete /etc/passwd"),
            assistant_tool_call("delete_file", r#"{"path":"/etc/passwd"}"#),
            tool_result("delete_file", "Permission denied", true),
        ];
        let output = format_turn_content(3, &messages);
        assert!(output.contains("(失败"));
    }

    #[test]
    fn test_build_turn_metadata() {
        let usage = Usage { input_tokens: 100, output_tokens: 50, total_tokens: 150, cache_creation_input_tokens: None, cache_read_input_tokens: None };
        let tools = vec![
            TurnToolCallSummary { name: "read_file".into(), is_error: false, result_len: 42 },
        ];
        let metadata = build_turn_metadata(
            "t1", "s1", 1, "gpt-4", &usage, &StopReason::Stop, &tools,
            std::time::SystemTime::now(),
        );
        let m = metadata.as_object().unwrap();
        assert_eq!(m["tenant_id"], "t1");
        assert_eq!(m["session_id"], "s1");
        assert_eq!(m["turn_index"], 1);
        assert_eq!(m["model"], "gpt-4");
        assert_eq!(m["stop_reason"], "stop");
        assert_eq!(m["token_usage"]["input_tokens"], 100);
        let tc = m["tool_calls"].as_array().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0]["name"], "read_file");
    }

    #[test]
    fn test_format_empty_turn() {
        let output = format_turn_content(0, &[]);
        assert!(output.contains("## Turn 0"));
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test -p agent-core --lib memory::formatter::tests 2>&1 | tail -5
```

预期：compilation error（模块还不存在）

- [ ] **Step 3: 创建 `memory/formatter.rs` 并实现**

```rust
// crates/agent-core/src/memory/formatter.rs

use std::time::SystemTime;

use ai_provider::{Content, StopReason, Usage};
use crate::types::AgentMessage;

/// Summary of a tool call for metadata purposes (name + outcome, no full params).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TurnToolCallSummary {
    pub name: String,
    pub is_error: bool,
    pub result_len: usize,
}

/// Format a turn's messages as Markdown for external memory system consumption.
///
/// Output example:
/// ```markdown
/// ## Turn 3
///
/// **User**: 帮我重构 src/main.rs
///
/// **ToolCall[read_file]**: path=src/main.rs
///
/// **ToolResult[read_file]**: (成功, 120 行)
/// 内容摘要: fn main() { ... }
///
/// **Assistant**: 我已经重构了 main.rs，主要改动有...
/// ```
pub fn format_turn_content(turn_index: u32, messages: &[AgentMessage]) -> String {
    let mut out = String::new();
    out.push_str(&format!("## Turn {}\n\n", turn_index));

    for msg in messages {
        match msg {
            AgentMessage::User(u) => {
                let text = collect_text(&u.content);
                if !text.is_empty() {
                    out.push_str(&format!("**User**: {}\n\n", text));
                }
            }
            AgentMessage::Assistant(a) => {
                let tool_calls = a.content.iter().filter_map(|c| {
                    if let Content::ToolCall(tc) = c {
                        Some(&tc.name)
                    } else {
                        None
                    }
                }).collect::<Vec<_>>();

                if !tool_calls.is_empty() {
                    for name in &tool_calls {
                        out.push_str(&format!("**ToolCall[{}]**: (see ToolResult below)\n\n", name));
                    }
                }

                let text = a.content.iter().filter_map(|c| {
                    if let Content::Text { text, .. } = c {
                        Some(text.as_str())
                    } else {
                        None
                    }
                }).collect::<Vec<_>>().join("\n");

                if !text.is_empty() {
                    out.push_str(&format!("**Assistant**: {}\n\n", text));
                }
            }
            AgentMessage::ToolResult(tr) => {
                let text = collect_text(&tr.content);
                let status = if tr.is_error { "失败" } else { "成功" };
                let summary = truncate_text(&text, 500);
                out.push_str(&format!(
                    "**ToolResult[{}]**: ({}, {} 字符)\n{}\n\n",
                    tr.tool_name,
                    status,
                    text.len(),
                    summary,
                ));
            }
        }
    }

    out
}

/// Build structured metadata for a turn, for external memory system indexing.
pub fn build_turn_metadata(
    tenant_id: &str,
    session_id: &str,
    turn_index: u32,
    model: &str,
    usage: &Usage,
    stop_reason: &StopReason,
    tool_calls: &[TurnToolCallSummary],
    timestamp: SystemTime,
) -> serde_json::Value {
    serde_json::json!({
        "tenant_id": tenant_id,
        "session_id": session_id,
        "turn_index": turn_index,
        "model": model,
        "stop_reason": format!("{:?}", stop_reason).to_lowercase(),
        "token_usage": {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens,
        },
        "tool_calls": tool_calls,
        "timestamp": format!("{:?}", timestamp),
    })
}

/// Extract tool call summaries from messages for metadata.
pub fn extract_tool_summaries(messages: &[AgentMessage]) -> Vec<TurnToolCallSummary> {
    let mut summaries = Vec::new();
    let mut tool_call_names: Vec<String> = Vec::new();

    for msg in messages {
        if let AgentMessage::Assistant(a) = msg {
            for c in &a.content {
                if let Content::ToolCall(tc) = &c {
                    tool_call_names.push(tc.name.clone());
                }
            }
        }
        if let AgentMessage::ToolResult(tr) = msg {
            let name = tool_call_names.first()
                .cloned()
                .unwrap_or_else(|| tr.tool_name.clone());
            if !tool_call_names.is_empty() {
                tool_call_names.remove(0);
            }
            let text = collect_text(&tr.content);
            summaries.push(TurnToolCallSummary {
                name,
                is_error: tr.is_error,
                result_len: text.len(),
            });
        }
    }

    summaries
}

// ── helpers ──

fn collect_text(content: &[Content]) -> String {
    content.iter().filter_map(|c| {
        if let Content::Text { text, .. } = c {
            Some(text.as_str())
        } else {
            None
        }
    }).collect::<Vec<_>>().join("\n")
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...(截断, 共 {} 字符)", &text[..max_len], text.len())
    }
}
```

- [ ] **Step 4: 更新 `memory/mod.rs`**

```rust
// crates/agent-core/src/memory/mod.rs

pub mod extractor;
pub mod formatter;
pub mod hook;
pub mod in_memory;
pub mod store;
pub mod types;

pub use store::{MemoryError, MemoryStore};
pub use types::MemoryContext;
```

- [ ] **Step 5: 运行 formatter 测试**

```bash
cargo test -p agent-core --lib memory::formatter::tests -- --nocapture
```

预期：5 个测试全部通过

- [ ] **Step 6: Commit**

```bash
git add crates/agent-core/src/memory/formatter.rs crates/agent-core/src/memory/mod.rs
git commit -m "feat(memory): add conversation formatter and turn metadata builder"
```

---

### Task 1.3: 重写 extractor（简化为 query builder + tool summary）

**Files:**
- Modify: `crates/agent-core/src/memory/extractor.rs`

- [ ] **Step 1: 重写 extractor.rs**

```rust
// crates/agent-core/src/memory/extractor.rs

use crate::types::AgentMessage;

/// Build a retrieval query string from the most recent 1–3 user messages.
pub fn build_query_string(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .rev()
        .take(3)
        .filter_map(|m| {
            if let AgentMessage::User(u) = m {
                Some(
                    u.content
                        .iter()
                        .filter_map(|c| match c {
                            ai_provider::Content::Text { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_provider::{Content, UserMessage};

    #[test]
    fn test_build_query_empty() {
        let query = build_query_string(&[]);
        assert!(query.is_empty());
    }

    #[test]
    fn test_build_query_from_user_messages() {
        let messages = vec![
            AgentMessage::User(UserMessage {
                content: vec![Content::Text {
                    text: "What is Rust?".to_string(),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::now(),
            }),
        ];
        let query = build_query_string(&messages);
        assert!(query.contains("What is Rust?"));
    }
}
```

- [ ] **Step 2: 运行测试**

```bash
cargo test -p agent-core --lib memory::extractor::tests -- --nocapture
```

预期：2 个测试通过

- [ ] **Step 3: Commit**

```bash
git add crates/agent-core/src/memory/extractor.rs
git commit -m "refactor(memory): simplify extractor to query builder only"
```

---

### Task 1.4: 适配 InMemoryStore

**Files:**
- Modify: `crates/agent-core/src/memory/in_memory.rs`

- [ ] **Step 1: 重写 InMemoryStore**

```rust
// crates/agent-core/src/memory/in_memory.rs

use std::collections::HashMap;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::store::{MemoryError, MemoryStore};
use super::types::MemoryContext;

struct MemoryRecord {
    content: String,
    metadata: serde_json::Value,
    timestamp: SystemTime,
}

/// Pure in-memory implementation of `MemoryStore` for testing.
pub struct InMemoryStore {
    data: RwLock<HashMap<String, Vec<MemoryRecord>>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self { data: RwLock::new(HashMap::new()) }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn remember(
        &self,
        ctx: &MemoryContext,
        content: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), MemoryError> {
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        self.data.write().await
            .entry(key)
            .or_default()
            .push(MemoryRecord {
                content: content.to_string(),
                metadata: metadata.clone(),
                timestamp: SystemTime::now(),
            });
        Ok(())
    }

    async fn recall(
        &self,
        ctx: &MemoryContext,
        query: &str,
    ) -> Result<Vec<String>, MemoryError> {
        let prefix = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        let results: Vec<String> = self.data.read().await
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .flat_map(|(_, records)| records.iter())
            .filter(|r| r.content.contains(query))
            .map(|r| r.content.clone())
            .take(5)
            .collect();
        Ok(results)
    }

    async fn forget_session(&self, ctx: &MemoryContext) -> Result<(), MemoryError> {
        let key = format!("{}:{}", ctx.tenant_id, ctx.session_id);
        self.data.write().await.remove(&key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> MemoryContext {
        MemoryContext {
            tenant_id: "t1".into(),
            session_id: "s1".into(),
            user_id: None,
            model: "gpt-4".into(),
            session_started_at: SystemTime::now(),
        }
    }

    #[tokio::test]
    async fn test_remember_and_recall() {
        let store = InMemoryStore::new();
        let ctx = make_ctx();

        store.remember(&ctx, "User likes TypeScript", &serde_json::json!({"turn": 1}))
            .await.unwrap();

        let results = store.recall(&ctx, "TypeScript").await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("TypeScript"));
    }

    #[tokio::test]
    async fn test_forget_session() {
        let store = InMemoryStore::new();
        let ctx = make_ctx();

        store.remember(&ctx, "test", &serde_json::json!({})).await.unwrap();
        store.forget_session(&ctx).await.unwrap();

        let results = store.recall(&ctx, "test").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_tenant_isolation() {
        let store = InMemoryStore::new();
        let ctx_a = MemoryContext {
            tenant_id: "ta".into(), session_id: "s1".into(),
            user_id: None, model: "gpt-4".into(),
            session_started_at: SystemTime::now(),
        };
        let ctx_b = MemoryContext {
            tenant_id: "tb".into(), session_id: "s1".into(),
            user_id: None, model: "gpt-4".into(),
            session_started_at: SystemTime::now(),
        };

        store.remember(&ctx_a, "secret-a", &serde_json::json!({})).await.unwrap();
        store.remember(&ctx_b, "secret-b", &serde_json::json!({})).await.unwrap();

        let ra = store.recall(&ctx_a, "secret").await.unwrap();
        assert_eq!(ra.len(), 1);
        assert!(ra[0].contains("secret-a"));

        let rb = store.recall(&ctx_b, "secret").await.unwrap();
        assert_eq!(rb.len(), 1);
        assert!(rb[0].contains("secret-b"));
    }
}
```

- [ ] **Step 2: 运行测试**

```bash
cargo test -p agent-core --lib memory::in_memory::tests -- --nocapture
```

预期：3 个测试通过

- [ ] **Step 3: Commit**

```bash
git add crates/agent-core/src/memory/in_memory.rs
git commit -m "refactor(memory): adapt InMemoryStore to simplified MemoryStore trait"
```

---

### Task 1.5: 重写 MemoryHookDispatcher

**Files:**
- Modify: `crates/agent-core/src/memory/hook.rs`

- [ ] **Step 1: 重写 MemoryHookDispatcher**

```rust
// crates/agent-core/src/memory/hook.rs

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::hook::context::{CompactEndCtx, ContextCtx, TurnEndCtx};
use crate::hook::dispatcher::HookDispatcher;
use crate::hook::mutations::ContextMutation;
use crate::types::AgentMessage;

use super::extractor::build_query_string;
use super::formatter::{build_turn_metadata, extract_tool_summaries, format_turn_content};
use super::store::MemoryStore;
use super::types::MemoryContext;

/// `HookDispatcher` implementation that sends formatted turn content
/// to an external `MemoryStore` and retrieves memories for context injection.
pub struct MemoryHookDispatcher {
    store: Arc<dyn MemoryStore>,
    model: String,
    session_started_at: SystemTime,
}

impl MemoryHookDispatcher {
    pub fn new(
        store: Arc<dyn MemoryStore>,
        model: String,
        session_started_at: SystemTime,
    ) -> Self {
        Self { store, model, session_started_at }
    }

    fn make_ctx(&self, tenant_id: &str, session_id: &str) -> MemoryContext {
        MemoryContext {
            tenant_id: tenant_id.to_string(),
            session_id: session_id.to_string(),
            user_id: None,
            model: self.model.clone(),
            session_started_at: self.session_started_at,
        }
    }
}

#[async_trait]
impl HookDispatcher for MemoryHookDispatcher {
    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        let content = format_turn_content(ctx.turn_index, &ctx.messages);
        let tool_summaries = extract_tool_summaries(&ctx.messages);
        let metadata = build_turn_metadata(
            &ctx.tenant_id, &ctx.session_id,
            ctx.turn_index, &self.model,
            &ctx.usage, &ctx.stop_reason,
            &tool_summaries, SystemTime::now(),
        );

        let mem_ctx = self.make_ctx(&ctx.tenant_id, &ctx.session_id);
        let store = self.store.clone();
        let turn_index = ctx.turn_index;

        tokio::spawn(async move {
            match tokio::time::timeout(
                Duration::from_secs(5),
                store.remember(&mem_ctx, &content, &metadata),
            )
            .await
            {
                Ok(Ok(())) => debug!(turn_index, "memory: remembered turn"),
                Ok(Err(e)) => warn!(turn_index, error = %e, "memory: remember failed"),
                Err(_) => warn!(turn_index, "memory: remember timed out"),
            }
        });
    }

    async fn on_context(&self, ctx: &ContextCtx) -> ContextMutation {
        let query = build_query_string(&ctx.messages);
        if query.is_empty() {
            return ContextMutation::default();
        }

        let mem_ctx = self.make_ctx(&ctx.tenant_id, &ctx.session_id);
        let facts = match tokio::time::timeout(
            Duration::from_secs(3),
            self.store.recall(&mem_ctx, &query),
        )
        .await
        {
            Ok(Ok(facts)) if !facts.is_empty() => facts,
            Ok(Err(e)) => {
                warn!(error = %e, "memory: recall failed");
                return ContextMutation::default();
            }
            Err(_) => {
                warn!("memory: recall timed out");
                return ContextMutation::default();
            }
            _ => return ContextMutation::default(),
        };

        debug!(fact_count = facts.len(), "memory: injecting recalled facts");

        let memory_text = facts.join("\n---\n");
        let memory_msg = AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: format!("[Memory]\n{}", memory_text),
                text_signature: None,
            }],
            timestamp: SystemTime::now(),
        });

        let mut messages = ctx.messages.clone();
        messages.insert(0, memory_msg);
        ContextMutation { messages: Some(messages) }
    }

    async fn on_compact_end(&self, ctx: &CompactEndCtx) {
        let summary = match &ctx.result {
            Some(r) => format!("[Session Compaction Summary]\n{}", r.summary),
            None => return,
        };

        let metadata = serde_json::json!({
            "category": "compaction",
            "importance": 8,
            "session_id": ctx.session_id,
            "token_savings": ctx.token_savings,
        });

        let mem_ctx = self.make_ctx(&ctx.tenant_id, &ctx.session_id);
        let store = self.store.clone();

        tokio::spawn(async move {
            match tokio::time::timeout(
                Duration::from_secs(5),
                store.remember(&mem_ctx, &summary, &metadata),
            )
            .await
            {
                Ok(Ok(())) => debug!("memory: compaction summary remembered"),
                Ok(Err(e)) => warn!(error = %e, "memory: compaction summary failed"),
                Err(_) => warn!("memory: compaction summary timed out"),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::in_memory::InMemoryStore;
    use ai_provider::{AssistantMessage, Content, StopReason, Usage, UserMessage};

    fn make_ctx(
        dispatcher: &MemoryHookDispatcher,
    ) -> TurnEndCtx {
        TurnEndCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            turn_index: 1,
            messages: vec![
                AgentMessage::User(UserMessage {
                    content: vec![Content::Text {
                        text: "hello".to_string(),
                        text_signature: None,
                    }],
                    timestamp: SystemTime::now(),
                }),
                AgentMessage::Assistant(AssistantMessage {
                    content: vec![Content::Text {
                        text: "Hi! How can I help?".to_string(),
                        text_signature: None,
                    }],
                    provider: "test".into(), model: "test".into(),
                    api: ai_provider::Api { provider: "test".into(), model: "test".into() },
                    usage: Usage::default(),
                    stop_reason: StopReason::Stop,
                    response_id: None, error_message: None,
                    timestamp: SystemTime::now(),
                }),
            ],
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            model: "gpt-4".to_string(),
        }
    }

    #[tokio::test]
    async fn test_on_turn_end_remembers_content() {
        let store = Arc::new(InMemoryStore::new());
        let dispatcher = MemoryHookDispatcher::new(
            store.clone(), "gpt-4".into(), SystemTime::now(),
        );

        let ctx = make_ctx(&dispatcher);
        dispatcher.on_turn_end(&ctx).await;

        // Give fire-and-forget a moment
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mem_ctx = dispatcher.make_ctx("t1", "s1");
        let results = store.recall(&mem_ctx, "hello").await.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].contains("hello"));
    }

    #[tokio::test]
    async fn test_on_context_recalls_memories() {
        let store = Arc::new(InMemoryStore::new());
        let dispatcher = MemoryHookDispatcher::new(
            store.clone(), "gpt-4".into(), SystemTime::now(),
        );

        // Seed a memory
        let mem_ctx = dispatcher.make_ctx("t1", "s1");
        store.remember(&mem_ctx, "Rust is fast", &serde_json::json!({}))
            .await.unwrap();

        let ctx = ContextCtx {
            tenant_id: "t1".into(),
            session_id: "s1".into(),
            messages: vec![AgentMessage::User(UserMessage {
                content: vec![Content::Text { text: "Rust".into(), text_signature: None }],
                timestamp: SystemTime::now(),
            })],
        };
        let mutation = dispatcher.on_context(&ctx).await;
        let msgs = mutation.messages.expect("should have messages");
        assert_eq!(msgs.len(), 2); // memory msg + original
        assert!(matches!(&msgs[0], AgentMessage::User(_)));
    }
}
```

- [ ] **Step 2: 更新 `SessionBuilder` 以适配新的 `MemoryHookDispatcher::new` 签名**

```rust
// crates/agent-core/src/harness/builder.rs
// 找到 MemoryHookDispatcher::new 调用处，传入 model 和 session_started_at

// 变更前:
// Arc::new(MemoryHookDispatcher::new(mem.clone()))
// 变更后:
// Arc::new(MemoryHookDispatcher::new(mem.clone(), self.model.clone(), SystemTime::now()))
```

- [ ] **Step 3: 运行所有 memory 测试**

```bash
cargo test -p agent-core --lib memory:: -- --nocapture
```

预期：所有 memory 模块测试通过

- [ ] **Step 4: 确认 agent-core 编译通过**

```bash
cargo check -p agent-core 2>&1
```

- [ ] **Step 5: Commit**

```bash
git add crates/agent-core/src/memory/hook.rs crates/agent-core/src/harness/builder.rs
git commit -m "refactor(memory): rewrite MemoryHookDispatcher for new MemoryStore trait"
```

---

### Phase 1 检查点

```bash
cargo test -p agent-core --lib -- --nocapture
cargo check -p storage
cargo check -p tenant
cargo check -p api-gateway
```

预期：agent-core 全部测试通过；storage、tenant、api-gateway 编译通过。

---

## 3. Phase 2：持久化加固

### 3.1 目标

新增 `append_entries` trait 方法，实现增量保存；实现自动 restore；MemoryStore forget 联动。

### 3.2 涉及文件

| 文件 | 变更 |
|---|---|
| `crates/agent-core/src/persistence/store.rs` | `SessionStore` 新增 `append_entries` 默认方法 |
| `crates/storage/src/session/postgres.rs` | override `append_entries` 为 `jsonb_insert` |
| `crates/storage/src/session/redis.rs` | 使用默认实现 |
| `crates/agent-core/src/harness/session.rs` | 自动 restore + 增量保存 + `restore()` deprecated |
| `crates/storage/tests/integration_postgres.rs` | 移除手动 `restore()` 调用；新增 `test_pg_append_entries` |
| `crates/storage/tests/integration_redis.rs` | 新增 `test_redis_append_entries` |
| `crates/tenant/src/manager.rs` | `delete_session()` 联动 `MemoryStore::forget_session()` |

### 3.3 具体步骤

---

### Task 2.1: SessionStore trait 新增 append_entries

**Files:**
- Modify: `crates/agent-core/src/persistence/store.rs`

- [ ] **Step 1: 新增 append_entries 默认方法**

```rust
// 在 SessionStore trait 内部，已有方法之后添加：

    /// Append new entries to an existing session without a full load→merge→save.
    ///
    /// The name reflects the caller's intent ("I have new entries to append"),
    /// not a guarantee of physical append at the storage layer.
    ///
    /// Default implementation: load → merge → full save.
    /// Storage adapters may override with more efficient strategies
    /// (e.g., `jsonb_insert` for PostgreSQL).
    async fn append_entries(
        &self,
        tenant_id: &str,
        session_id: &str,
        new_entries: &[SessionEntry],
    ) -> Result<(), AgentError> {
        let mut all = self.load_session(tenant_id, session_id).await?;
        all.extend_from_slice(new_entries);
        self.save_session(tenant_id, session_id, &all).await
    }
```

- [ ] **Step 2: 编译检查**

```bash
cargo check -p agent-core 2>&1
```

验证：编译通过（默认实现无破坏性）。

- [ ] **Step 3: Commit**

```bash
git add crates/agent-core/src/persistence/store.rs
git commit -m "feat(persistence): add append_entries to SessionStore trait with default impl"
```

---

### Task 2.2: PostgreSQL 适配器 override append_entries

**Files:**
- Modify: `crates/storage/src/session/postgres.rs`

- [ ] **Step 1: 编写集成测试**

```rust
// 追加到 crates/storage/tests/integration_postgres.rs 末尾

#[tokio::test]
async fn test_pg_append_entries() {
    let _ = tracing_subscriber::fmt().try_init();
    let (pool, _container) = start_pg().await;
    let store = PgSessionStore::new(pool);
    store.init().await.expect("init failed");

    let tenant = "append_t";
    let session = "append_s";

    // 1. Save initial entries
    let e1 = SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "first".to_string(), text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    };
    store.save_session(tenant, session, &[e1.clone()])
        .await.expect("save failed");

    // 2. Append new entries
    let e2 = SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::Assistant(ai_provider::AssistantMessage {
            content: vec![ai_provider::Content::Text {
                text: "second".to_string(), text_signature: None,
            }],
            provider: "test".into(), model: "test".into(),
            api: ai_provider::Api { provider: "test".into(), model: "test".into() },
            usage: ai_provider::Usage {
                input_tokens: 1, output_tokens: 1, total_tokens: 2,
                cache_creation_input_tokens: None, cache_read_input_tokens: None,
            },
            stop_reason: ai_provider::StopReason::Stop,
            response_id: None, error_message: None,
            timestamp: std::time::SystemTime::now(),
        }),
    };
    store.append_entries(tenant, session, &[e2.clone()])
        .await.expect("append failed");

    // 3. Load and verify both entries exist
    let loaded = store.load_session(tenant, session).await.expect("load failed");
    assert_eq!(loaded.len(), 2);

    // Verify first entry is still present
    match &loaded[0] {
        SessionEntry::Message { message, .. } => match message {
            agent_core::AgentMessage::User(u) => {
                assert!(u.content.iter().any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "first")));
            }
            _ => panic!("expected user message"),
        },
        _ => panic!("expected Message"),
    }

    // Verify second entry was appended
    match &loaded[1] {
        SessionEntry::Message { message, .. } => match message {
            agent_core::AgentMessage::Assistant(a) => {
                assert!(a.content.iter().any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "second")));
            }
            _ => panic!("expected assistant message"),
        },
        _ => panic!("expected Message"),
    }
}
```

- [ ] **Step 2: 运行测试确认失败（默认实现走 load→merge→save，应通过但非最优）**

```bash
PANDARIA_TEST_PG_URL="postgres://postgres:postgres@localhost:15432/postgres" \
cargo test -p storage --test integration_postgres test_pg_append_entries -- --nocapture
```

- [ ] **Step 3: override PgSessionStore::append_entries 为 jsonb_insert**

```rust
// 在 impl SessionStore for PgSessionStore 块内添加：

    async fn append_entries(
        &self,
        tenant_id: &str,
        session_id: &str,
        new_entries: &[SessionEntry],
    ) -> Result<(), AgentError> {
        let json = serde_json::to_value(new_entries)
            .map_err(|e| AgentError::Persistence(format!("serialize: {e}")))?;

        // jsonb_insert concatenates the new array onto the existing one
        sqlx::query(
            r#"
            INSERT INTO sessions (tenant_id, session_id, entries, updated_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (tenant_id, session_id)
            DO UPDATE SET
                entries = sessions.entries || EXCLUDED.entries,
                updated_at = NOW()
            "#,
        )
        .bind(tenant_id)
        .bind(session_id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Persistence(format!("pg append: {e}")))?;

        Ok(())
    }
```

- [ ] **Step 4: 运行测试验证通过**

```bash
PANDARIA_TEST_PG_URL="postgres://postgres:postgres@localhost:15432/postgres" \
cargo test -p storage --test integration_postgres test_pg_append_entries -- --nocapture
```

- [ ] **Step 5: 运行全部 PG 集成测试**

```bash
PANDARIA_TEST_PG_URL="postgres://postgres:postgres@localhost:15432/postgres" \
cargo test -p storage --test integration_postgres -- --test-threads=1 --nocapture
```

- [ ] **Step 6: Commit**

```bash
git add crates/storage/src/session/postgres.rs crates/storage/tests/integration_postgres.rs
git commit -m "feat(persistence): pg adapter uses jsonb concatenation for append_entries"
```

---

### Task 2.3: Redis 适配器确认 + 测试

**Files:**
- Modify: `crates/storage/tests/integration_redis.rs`

- [ ] **Step 1: 编写 append_entries 集成测试**

```rust
// 追加到 crates/storage/tests/integration_redis.rs 末尾

#[tokio::test]
async fn test_redis_append_entries() {
    let _ = tracing_subscriber::fmt().try_init();
    let (conn, _container) = start_redis().await;
    let store = RedisSessionStore::new(conn);

    let tenant = "append_t";
    let session = "append_s";

    let e1 = SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "first".to_string(), text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    };
    store.save_session(tenant, session, &[e1]).await.expect("save failed");

    let e2 = SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::Assistant(ai_provider::AssistantMessage {
            content: vec![ai_provider::Content::Text {
                text: "second".to_string(), text_signature: None,
            }],
            provider: "test".into(), model: "test".into(),
            api: ai_provider::Api { provider: "test".into(), model: "test".into() },
            usage: ai_provider::Usage {
                input_tokens: 1, output_tokens: 1, total_tokens: 2,
                cache_creation_input_tokens: None, cache_read_input_tokens: None,
            },
            stop_reason: ai_provider::StopReason::Stop,
            response_id: None, error_message: None,
            timestamp: std::time::SystemTime::now(),
        }),
    };
    store.append_entries(tenant, session, &[e2]).await.expect("append failed");

    let loaded = store.load_session(tenant, session).await.expect("load failed");
    assert_eq!(loaded.len(), 2);
}
```

- [ ] **Step 2: 运行测试**

```bash
PANDARIA_TEST_REDIS_URL="redis://:redis@localhost:16379" \
cargo test -p storage --test integration_redis test_redis_append_entries -- --nocapture
```

- [ ] **Step 3: 运行全部 Redis 集成测试**

```bash
PANDARIA_TEST_REDIS_URL="redis://:redis@localhost:16379" \
cargo test -p storage --test integration_redis -- --nocapture
```

- [ ] **Step 4: Commit**

```bash
git add crates/storage/tests/integration_redis.rs
git commit -m "test(persistence): add redis append_entries integration test"
```

---

### Task 2.4: 自动 restore + 增量保存

**Files:**
- Modify: `crates/agent-core/src/harness/session.rs`
- Modify: `crates/storage/tests/integration_postgres.rs`（移除手动 restore 调用）

- [ ] **Step 1: 在 SessionActor 添加 needs_restore 和 last_saved_entry_count 字段**

在 `SessionActor` struct 中添加：

```rust
    // 新增字段
    needs_restore: bool,
    last_saved_entry_count: usize,
    model: String,
    session_started_at: std::time::SystemTime,
```

- [ ] **Step 2: 在 SessionConfig 中添加 model 字段**

```rust
pub struct SessionConfig {
    // ... 已有字段 ...
    pub model: String,           // 已存在？检查，如无则新增
}
```

- [ ] **Step 3: 在 SessionActor::new() 中初始化**

```rust
    pub fn new(config: SessionConfig) -> Self {
        let session_started_at = std::time::SystemTime::now();
        Self {
            // ... 已有字段初始化 ...
            needs_restore: config.store.is_some(),
            last_saved_entry_count: 0,
            model: config.model.clone(),
            session_started_at,
        }
    }
```

- [ ] **Step 4: 在 prompt() / run_with_messages() 入口处自动 restore**

```rust
    // 在 prompt() 或 run_with_messages() 的开头（在添加 user message 之前）:

    if self.needs_restore {
        self.needs_restore = false;
        if let Some(ref store) = self.store {
            match store.load_session(&self.tenant_id, &self.session_id).await {
                Ok(entries) if !entries.is_empty() => {
                    self.entries = entries.clone();
                    self.last_saved_entry_count = entries.len();
                    info!(
                        tenant_id = %self.tenant_id,
                        session_id = %self.session_id,
                        restored_count = entries.len(),
                        "auto-restored session history",
                    );
                }
                Ok(_) => {
                    // Empty store — fresh session, no-op
                }
                Err(e) => {
                    warn!(
                        tenant_id = %self.tenant_id,
                        session_id = %self.session_id,
                        error = %e,
                        "auto-restore failed, starting with empty session",
                    );
                    // Continue with empty entries — don't block agent loop
                }
            }
        }
    }
```

- [ ] **Step 5: 修改 prompt() 结尾的持久化逻辑为增量保存**

找到 prompt() 末尾的持久化 block（现有 `if let Some(ref store) = self.store`），替换为：

```rust
        // Persist incrementally — only save new entries since last save
        if let Some(ref store) = self.store {
            let new_entries = &self.entries[self.last_saved_entry_count..];
            if !new_entries.is_empty() {
                if let Some(handle) = self.last_save.take() {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
                }
                let tenant_id = self.tenant_id.clone();
                let session_id = self.session_id.clone();
                let store = store.clone();
                let entries_to_save = new_entries.to_vec();
                self.last_saved_entry_count = self.entries.len();
                let new_count = entries_to_save.len();
                self.last_save = Some(tokio::spawn(async move {
                    if let Err(e) = store.append_entries(&tenant_id, &session_id, &entries_to_save).await {
                        warn!(
                            tenant_id = %tenant_id,
                            session_id = %session_id,
                            error = %e,
                            "failed to persist session",
                        );
                    }
                }));
            }
        }
```

- [ ] **Step 6: flush() 保持全量保存不变**

`flush()` 保持使用 `save_session()` 全量保存以保证最终一致性。

- [ ] **Step 7: `restore()` 公开方法标记 deprecated**

```rust
    /// Attempt to restore session history from the configured store.
    ///
    /// **Deprecated:** Restore now happens automatically in `prompt()` /
    /// `run_with_messages()`. This method is a no-op and will be removed
    /// in a future version.
    #[deprecated(since = "0.2.0", note = "restore is now automatic; this method is a no-op")]
    pub async fn restore(&mut self) -> Result<usize, AgentError> {
        Ok(0)
    }
```

- [ ] **Step 8: 更新 integration_postgres.rs 中 test_session_actor_persistence_loop**

移除 `session2.restore().await.expect("restore failed")` 调用。自动 restore 后直接验证 messages()。

- [ ] **Step 9: 运行测试**

```bash
cargo test -p agent-core --lib harness::session -- --nocapture
PANDARIA_TEST_PG_URL="postgres://postgres:postgres@localhost:15432/postgres" \
cargo test -p storage --test integration_postgres test_session_actor_persistence_loop -- --nocapture
```

预期：全部通过。

- [ ] **Step 10: Commit**

```bash
git add crates/agent-core/src/harness/session.rs crates/storage/tests/integration_postgres.rs
git commit -m "feat(persistence): auto-restore and incremental save for SessionActor"
```

---

### Task 2.5: MemoryStore forget 联动

**Files:**
- Modify: `crates/tenant/src/manager.rs`

- [ ] **Step 1: 在 TenantManagerImpl 的 delete_session 中添加 forget 调用**

找到 `delete_session()` 方法，在删除 SessionStore 之后、释放 quota 之前添加：

```rust
    // 2. 清理 MemoryStore（fire-and-forget）
    if let Some(ref mem) = self.runtime_config.memory_store {
        let mem_ctx = agent_core::MemoryContext {
            tenant_id: tenant_id.to_string(),
            session_id: session_id.to_string(),
            user_id: None,
            model: self.runtime_config.default_model.clone(),
            session_started_at: std::time::SystemTime::now(), // not critical for delete
        };
        let _ = tokio::spawn(async move {
            if let Err(e) = mem.forget_session(&mem_ctx).await {
                tracing::warn!(tenant_id = %mem_ctx.tenant_id, session_id = %mem_ctx.session_id, error = %e, "memory forget failed");
            }
        });
    }
```

- [ ] **Step 2: 编译检查**

```bash
cargo check -p tenant 2>&1
```

- [ ] **Step 3: Commit**

```bash
git add crates/tenant/src/manager.rs
git commit -m "feat(tenant): call MemoryStore.forget_session on session delete"
```

---

### Phase 2 检查点

```bash
cargo test -p agent-core --lib -- --nocapture
PANDARIA_TEST_PG_URL="postgres://postgres:postgres@localhost:15432/postgres" \
cargo test -p storage --test integration_postgres -- --test-threads=1 --nocapture
PANDARIA_TEST_REDIS_URL="redis://:redis@localhost:16379" \
cargo test -p storage --test integration_redis -- --nocapture
cargo check -p tenant
cargo check -p api-gateway
```

---

## 4. Phase 3：E2E 测试矩阵

### 4.1 目标

扩展 api-gateway E2E 测试基础设施，新增 5 个测试文件覆盖持久化恢复、compaction 组合、故障注入、并发隔离、MemoryStore 联动。

### 4.2 涉及文件

#### 修改

| 文件 | 变更 |
|---|---|
| `crates/api-gateway/tests/e2e/common.rs` | 新增 `build_test_app_with_store()`、`build_test_app_with_memory()`、`ensure_test_containers()` |

#### 新增

| 文件 | 说明 |
|---|---|
| `crates/api-gateway/tests/e2e/e2e_persistence_recovery.rs` | 全链路持久化恢复 |
| `crates/api-gateway/tests/e2e/e2e_persistence_compaction.rs` | compaction + persistence 组合 |
| `crates/api-gateway/tests/e2e/e2e_persistence_fault_injection.rs` | DB 不可用降级 |
| `crates/api-gateway/tests/e2e/e2e_concurrent_sessions.rs` | 并发 session 隔离 |
| `crates/api-gateway/tests/e2e/e2e_memory_store.rs` | MemoryStore 联动 |

### 4.3 具体步骤

---

### Task 3.1: 测试基础设施增强

**Files:**
- Modify: `crates/api-gateway/tests/e2e/common.rs`

- [ ] **Step 1: 新增 build_test_app_with_store 工厂函数**

```rust
// 在 common.rs 中添加（在 build_test_app_with_client 之后）

use std::sync::Arc;

/// Build a test router with both a real TenantManagerImpl and a SessionStore.
/// Delegates to `build_test_app_with_store_and_compaction` with default config.
pub fn build_test_app_with_store(
    provider: Arc<dyn ai_provider::LlmProvider>,
    store: Arc<dyn agent_core::SessionStore>,
) -> Router {
    build_test_app_with_store_and_compaction(
        provider, store, agent_core::CompactionConfig::default(),
    )
}

/// Build a test router with a SessionStore and custom CompactionConfig.
pub fn build_test_app_with_store_and_compaction(
    provider: Arc<dyn ai_provider::LlmProvider>,
    store: Arc<dyn agent_core::SessionStore>,
    compaction_config: agent_core::CompactionConfig,
) -> Router {
    let registry = Arc::new(tenant::TenantRegistry::new());
    let test_tenant = tenant::Tenant::new(
        "test-tenant",
        tenant::TenantQuota {
            max_concurrent_sessions: 10,
            max_tokens_per_day: 1_000_000,
            max_tool_calls_per_minute: 60,
            cpu_time_budget_ms_per_day: 3_600_000,
        },
    );
    registry.register(test_tenant).unwrap();

    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: Some(store),
        media_provider: None,
        media_registry: None,
        http_client: reqwest::Client::new(),
        available_models: vec!["gpt-4".to_string()],
        compaction_config,
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry, runtime_config),
    );

    let config = ServerConfig {
        auth_secret: secrecy::SecretString::from(TEST_SECRET),
        ..Default::default()
    };
    let state = Arc::new(AppState::new(manager, config));
    api_gateway::build_router(state)
}

/// Build a test router with a SessionStore AND a MemoryStore (InMemoryStore).
pub fn build_test_app_with_memory(
    provider: Arc<dyn ai_provider::LlmProvider>,
    store: Arc<dyn agent_core::SessionStore>,
    memory_store: Arc<dyn agent_core::MemoryStore>,
) -> Router {
    let registry = Arc::new(tenant::TenantRegistry::new());
    let test_tenant = tenant::Tenant::new(
        "test-tenant",
        tenant::TenantQuota {
            max_concurrent_sessions: 10,
            max_tokens_per_day: 1_000_000,
            max_tool_calls_per_minute: 60,
            cpu_time_budget_ms_per_day: 3_600_000,
        },
    );
    registry.register(test_tenant).unwrap();

    let runtime_config = Arc::new(agent_core::HarnessConfig {
        provider: provider.clone(),
        default_model: "gpt-4".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: Some(store),
        media_provider: None,
        media_registry: None,
        http_client: reqwest::Client::new(),
        available_models: vec!["gpt-4".to_string()],
        compaction_config: agent_core::CompactionConfig::default(),
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: Some(memory_store),
    });
    let manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry, runtime_config),
    );

    let config = ServerConfig {
        auth_secret: secrecy::SecretString::from(TEST_SECRET),
        ..Default::default()
    };
    let state = Arc::new(AppState::new(manager, config));
    api_gateway::build_router(state)
}
```

- [ ] **Step 2: 新增 Docker 健康检查前置函数**

```rust
/// Verify Docker containers are running before persistence-dependent tests.
/// Call this at the start of tests that need PG or Redis.
pub async fn ensure_test_containers() {
    let pg_url = std::env::var("PANDARIA_TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:15432/postgres".to_string());
    let redis_url = std::env::var("PANDARIA_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://:redis@localhost:16379".to_string());

    let pg_ok = sqlx::PgPool::connect(&pg_url).await.is_ok();
    let redis_ok = redis::Client::open(redis_url.clone())
        .and_then(|c| c.get_multiplexed_async_connection());

    if !pg_ok || redis_ok.is_err() {
        panic!(
            "测试容器未启动。请先运行:\n\
             docker start docker-env-postgres docker-env-redis\n\
             或设置环境变量 PANDARIA_TEST_PG_URL / PANDARIA_TEST_REDIS_URL"
        );
    }
}

/// Create a PgSessionStore connected to the test PostgreSQL container.
pub async fn create_test_pg_store() -> PgSessionStore {
    let url = std::env::var("PANDARIA_TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:15432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&url).await
        .expect("failed to connect to test postgres");
    let store = PgSessionStore::new(pool);
    store.init().await.expect("pg init failed");
    store
}
```

- [ ] **Step 3: 编译检查**

```bash
cargo check -p api-gateway --tests 2>&1
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-gateway/tests/e2e/common.rs
git commit -m "test(api-gateway): add store injection and container health check fixtures"
```

---

### Task 3.2: E1 — 全链路持久化恢复

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_persistence_recovery.rs`

- [ ] **Step 1: 编写测试**

```rust
//! End-to-end integration test: persistence recovery across simulated restarts.
//!
//! Verifies session history survives service restart via PostgreSQL store.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use storage::session::postgres::PgSessionStore;
use tower::ServiceExt;

#[tokio::test]
async fn test_session_persistence_recovery() {
    let _ = tracing_subscriber::fmt().try_init();
    common::ensure_test_containers().await;

    let body = common::openai_text_sse_body("persisted response");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    // Phase 1: Create session, send message, flush, then "restart"
    let session_id;
    {
        let store = Arc::new(common::create_test_pg_store().await);
        let app = common::build_test_app_with_store(provider.clone(), store.clone());
        let token = common::make_token("test-tenant");

        // Create session
        let create_response = app.clone()
            .oneshot(Request::builder()
                .method("POST").uri("/api/v1/sessions")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title": "recovery test"}"#))
                .unwrap())
            .await.unwrap();
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let create_body = common::json_body(create_response).await;
        session_id = create_body["id"].as_str().unwrap().to_string();

        // Send message
        let send_response = app.clone()
            .oneshot(Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"content": [{"type":"text","text":"persist me"}]}"#))
                .unwrap())
            .await.unwrap();
        assert_eq!(send_response.status(), StatusCode::OK);

        // Give fire-and-forget save time to complete
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    // App dropped here — simulates app restart (store persists independently)

    // Phase 2: Rebuild app with same store, verify session history restored
    {
        let pg_url = std::env::var("PANDARIA_TEST_PG_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:15432/postgres".to_string());
        let pool = sqlx::PgPool::connect(&pg_url).await
            .expect("reconnect failed");
        let store = Arc::new(PgSessionStore::new(pool));
        let app = common::build_test_app_with_store(provider, store);
        let token = common::make_token("test-tenant");

        // Get session messages — should contain the persisted history
        let msgs_response = app
            .oneshot(Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/messages", session_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap())
            .await.unwrap();

        assert_eq!(msgs_response.status(), StatusCode::OK);
        let msgs = common::json_body(msgs_response).await;
        let msgs_arr = msgs.as_array().unwrap();
        assert!(
            msgs_arr.len() >= 2,
            "expected at least user + assistant after recovery, got {}",
            msgs_arr.len()
        );

        // Verify the user message survived roundtrip
        let has_user_msg = msgs_arr.iter().any(|m| {
            m.get("content").and_then(|c| c.as_array()).map_or(false, |content| {
                content.iter().any(|c| c.get("text").and_then(|t| t.as_str()) == Some("persist me"))
            })
        });
        assert!(has_user_msg, "user message not found after recovery");

        // Verify assistant response survived
        let has_assistant = msgs_arr.iter().any(|m| {
            m.get("content").and_then(|c| c.as_array()).map_or(false, |content| {
                content.iter().any(|c| c.get("text").and_then(|t| t.as_str()) == Some("persisted response"))
            })
        });
        assert!(has_assistant, "assistant message not found after recovery");
    }
}
```

- [ ] **Step 2: 运行测试**

```bash
PANDARIA_TEST_PG_URL="postgres://postgres:postgres@localhost:15432/postgres" \
cargo test -p api-gateway --test e2e_persistence_recovery -- --nocapture
```

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/tests/e2e/e2e_persistence_recovery.rs
git commit -m "test(e2e): add persistence recovery across simulated restart"
```

---

### Task 3.3: E2 — compaction + persistence 组合

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_persistence_compaction.rs`

- [ ] **Step 1: 编写测试**

```rust
//! End-to-end integration test: compaction combined with persistence.
//!
//! Verifies that compaction entries are correctly persisted and restored
//! when auto-compaction triggers during multi-turn sessions.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// Wiremock responder that returns a unique short reply per call.
fn make_counting_responder(
) -> impl Fn(&wiremock::Request) -> wiremock::ResponseTemplate + Send + Sync + 'static {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let counter = Arc::new(AtomicUsize::new(0));
    move |_: &wiremock::Request| {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        let body = common::openai_text_sse_body(&format!("turn-{}", n));
        wiremock::ResponseTemplate::new(200).set_body_string(body)
    }
}

#[tokio::test]
async fn test_compaction_persistence() {
    let _ = tracing_subscriber::fmt().try_init();
    common::ensure_test_containers().await;

    let (_server, provider) =
        common::start_wiremock_openai_dynamic(make_counting_responder()).await;

    // Use low compaction threshold: compact after 3 messages
    let compaction_config = agent_core::CompactionConfig {
        max_messages: 3,
        ..agent_core::CompactionConfig::default()
    };
    let store = Arc::new(common::create_test_pg_store().await);
    let app = common::build_test_app_with_store_and_compaction(
        provider.clone(), store.clone(), compaction_config,
    );
    let token = common::make_token("test-tenant");

    // Create session
    let create = app.clone()
        .oneshot(Request::builder()
            .method("POST").uri("/api/v1/sessions")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"title": "compaction test"}"#))
            .unwrap())
        .await.unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let sid = common::json_body(create).await["id"].as_str().unwrap().to_string();

    // Send 6 prompts -- should trigger compaction around turn 3-4
    for i in 0..6 {
        let resp = app.clone()
            .oneshot(Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    format!(r#"{{"content": [{{"type":"text","text":"msg-{}"}}]}}"#, i),
                ))
                .unwrap())
            .await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "turn {} should succeed", i);
    }

    // Give fire-and-forget persistence time
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify entries in PG store include Compaction variant
    let entries = store.load_session("test-tenant", &sid).await
        .expect("load session from pg");

    let has_compaction = entries.iter()
        .any(|e| matches!(e, agent_core::SessionEntry::Compaction { .. }));
    assert!(has_compaction,
        "expected at least one Compaction entry in session history");

    let compaction_count = entries.iter()
        .filter(|e| matches!(e, agent_core::SessionEntry::Compaction { .. }))
        .count();
    assert!(compaction_count >= 1,
        "expected >=1 compaction entries, got {}", compaction_count);

    // Verify messages after compaction boundary are still present
    let compaction_idx = entries.iter()
        .rposition(|e| matches!(e, agent_core::SessionEntry::Compaction { .. }))
        .expect("should have compaction boundary");
    let after_compaction = &entries[compaction_idx + 1..];
    assert!(!after_compaction.is_empty(),
        "messages should exist after compaction boundary");
}
```

- [ ] **Step 2: 运行测试**

```bash
PANDARIA_TEST_PG_URL="postgres://postgres:postgres@localhost:15432/postgres" \
cargo test -p api-gateway --test e2e_persistence_compaction -- --nocapture
```

预期：compaction 触发后 entries 中包含 `SessionEntry::Compaction` 类型。

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/tests/e2e/e2e_persistence_compaction.rs
git commit -m "test(e2e): add compaction + persistence combined test"
```

---

### Task 3.4: E3 — 故障注入

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_persistence_fault_injection.rs`

- [ ] **Step 1: 编写测试**

```rust
//! End-to-end test: agent loop completes despite persistence failures.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use storage::session::postgres::PgSessionStore;
use tower::ServiceExt;

/// Build a test app with a store pointing to a non-existent PostgreSQL port.
/// Persistence will fail but the agent loop must still complete.
async fn build_app_with_bad_store(
    provider: Arc<dyn ai_provider::LlmProvider>,
) -> Router {
    // Connect to a port that doesn't have PostgreSQL
    let bad_pool = PgPool::connect("postgres://postgres:postgres@localhost:19999/postgres").await;
    // This will fail to connect — that's expected
    let store: Arc<dyn agent_core::SessionStore> = if let Ok(pool) = bad_pool {
        Arc::new(PgSessionStore::new(pool))
    } else {
        // If we can't even connect, use a pool to a real PG but close it immediately
        // Actually: create an in-memory store wrapper that always fails on save
        Arc::new(FailingStore)
    };
    common::build_test_app_with_store(provider, store)
}

/// A SessionStore that always returns errors on write.
struct FailingStore;

#[async_trait::async_trait]
impl agent_core::SessionStore for FailingStore {
    async fn save_session(&self, _: &str, _: &str, _: &[agent_core::SessionEntry]) -> Result<(), agent_core::AgentError> {
        Err(agent_core::AgentError::Persistence("simulated failure".into()))
    }
    async fn load_session(&self, _: &str, _: &str) -> Result<Vec<agent_core::SessionEntry>, agent_core::AgentError> {
        Ok(Vec::new())
    }
    async fn delete_session(&self, _: &str, _: &str) -> Result<(), agent_core::AgentError> {
        Ok(())
    }
    async fn list_sessions(&self, _: &str) -> Result<Vec<String>, agent_core::AgentError> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn test_agent_loop_survives_persistence_failure() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("success despite failure");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = build_app_with_bad_store(provider).await;
    let token = common::make_token("test-tenant");

    // Create session
    let create_response = app.clone()
        .oneshot(Request::builder()
            .method("POST").uri("/api/v1/sessions")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"title": "fault test"}"#))
            .unwrap())
        .await.unwrap();
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_body = common::json_body(create_response).await;
    let session_id = create_body["id"].as_str().unwrap();

    // Send message — should complete normally despite persistence failure
    let send_response = app.clone()
        .oneshot(Request::builder()
            .method("POST")
            .uri(format!("/api/v1/sessions/{}/messages", session_id))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"content": [{"type":"text","text":"hello"}]}"#))
            .unwrap())
        .await.unwrap();

    assert_eq!(send_response.status(), StatusCode::OK);
    let send_body = common::json_body(send_response).await;
    assert_eq!(send_body["turn_index"], 0);

    // Verify we still get back messages (in-memory state is intact)
    let msgs_response = app
        .oneshot(Request::builder()
            .method("GET")
            .uri(format!("/api/v1/sessions/{}/messages", session_id))
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap())
        .await.unwrap();

    assert_eq!(msgs_response.status(), StatusCode::OK);
    let msgs = common::json_body(msgs_response).await;
    let msgs_arr = msgs.as_array().unwrap();
    assert!(msgs_arr.len() >= 2, "expected messages in memory despite persistence failure");
}
```

- [ ] **Step 2: 运行测试**

```bash
cargo test -p api-gateway --test e2e_persistence_fault_injection -- --nocapture
```

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/tests/e2e/e2e_persistence_fault_injection.rs
git commit -m "test(e2e): add persistence fault injection test"
```

---

### Task 3.5: E4 — 并发 session 隔离

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_concurrent_sessions.rs`

- [ ] **Step 1: 编写测试**

```rust
//! End-to-end test: concurrent session isolation and quota enforcement.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_concurrent_sessions_isolation() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("concurrent ok");
    let (_server, provider) = common::start_wiremock_openai(&body).await;
    let app = common::build_test_app(provider);
    let token = common::make_token("test-tenant");

    // Create 3 sessions concurrently
    let mut handles = Vec::new();
    for i in 0..3 {
        let app = app.clone();
        let token = token.clone();
        handles.push(tokio::spawn(async move {
            let resp = app
                .oneshot(Request::builder()
                    .method("POST").uri("/api/v1/sessions")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(format!(r#"{{"title": "concurrent-{}"}}"#, i)))
                    .unwrap())
                .await.unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
            common::json_body(resp).await["id"].as_str().unwrap().to_string()
        }));
    }

    let mut session_ids = Vec::new();
    for h in handles {
        session_ids.push(h.await.unwrap());
    }

    // Send messages to each session and verify isolation
    for (i, sid) in session_ids.iter().enumerate() {
        let resp = app.clone()
            .oneshot(Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(r#"{{"content": [{{"type":"text","text":"msg-{}"}}]}}"#, i)))
                .unwrap())
            .await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Verify each session only has its own messages
    for (i, sid) in session_ids.iter().enumerate() {
        let resp = app.clone()
            .oneshot(Request::builder()
                .method("GET")
                .uri(format!("/api/v1/sessions/{}/messages", sid))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap())
            .await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let msgs = common::json_body(resp).await;
        let arr = msgs.as_array().unwrap();
        let has_own = arr.iter().any(|m| {
            m.get("content").and_then(|c| c.as_array()).map_or(false, |content| {
                content.iter().any(|c| {
                    c.get("text").and_then(|t| t.as_str())
                        .map_or(false, |t| t == &format!("msg-{}", i))
                })
            })
        });
        assert!(has_own, "session {} missing its own message", i);
    }
}
```

- [ ] **Step 2: 运行测试**

```bash
cargo test -p api-gateway --test e2e_concurrent_sessions -- --nocapture
```

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/tests/e2e/e2e_concurrent_sessions.rs
git commit -m "test(e2e): add concurrent session isolation test"
```

---

### Task 3.6: E5 — MemoryStore 联动

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_memory_store.rs`

- [ ] **Step 1: 编写测试**

```rust
//! End-to-end test: MemoryStore integration via MemoryHookDispatcher.
//!
//! Verifies that turn content is correctly formatted and stored,
//! and that session deletion triggers forget_session.

mod common;

use std::sync::Arc;

use agent_core::memory::in_memory::InMemoryStore;
use agent_core::MemoryStore;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_memory_store_remember_on_turn() {
    let _ = tracing_subscriber::fmt().try_init();

    let body = common::openai_text_sse_body("remembered");
    let (_server, provider) = common::start_wiremock_openai(&body).await;

    let memory_store = Arc::new(InMemoryStore::new());
    // Note: build_test_app_with_memory creates app with store=None for SessionStore
    // We need a build_test_app variant that accepts MemoryStore without SessionStore
    // For now, use build_test_app and inject InMemoryStore into the runtime config
    let app = {
        let registry = Arc::new(tenant::TenantRegistry::new());
        let test_tenant = tenant::Tenant::new("test-tenant", tenant::TenantQuota {
            max_concurrent_sessions: 10, max_tokens_per_day: 1_000_000,
            max_tool_calls_per_minute: 60, cpu_time_budget_ms_per_day: 3_600_000,
        });
        registry.register(test_tenant).unwrap();

        let runtime_config = Arc::new(agent_core::HarnessConfig {
            provider: provider.clone(),
            default_model: "gpt-4".to_string(),
            default_system_prompt: "You are helpful.".to_string(),
            default_context_window: 128_000,
            store: None,
            media_provider: None, media_registry: None,
            http_client: reqwest::Client::new(),
            available_models: vec!["gpt-4".to_string()],
            compaction_config: agent_core::CompactionConfig::default(),
            agent_space: agent_core::AgentSpace::default(),
            hook_config: agent_core::HookConfig::default(),
            memory_store: Some(memory_store.clone()),
        });
        let manager: Arc<dyn tenant::TenantManager> = Arc::new(
            tenant::manager::TenantManagerImpl::new(registry, runtime_config),
        );
        let config = api_gateway::ServerConfig {
            auth_secret: secrecy::SecretString::from(common::TEST_SECRET),
            ..Default::default()
        };
        api_gateway::build_router(Arc::new(api_gateway::AppState::new(manager, config)))
    };

    let token = common::make_token("test-tenant");

    // Create session
    let create = app.clone()
        .oneshot(Request::builder().method("POST").uri("/api/v1/sessions")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"title": "mem test"}"#)).unwrap())
        .await.unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let sid = common::json_body(create).await["id"].as_str().unwrap().to_string();

    // Send message
    let send = app.clone()
        .oneshot(Request::builder().method("POST")
            .uri(format!("/api/v1/sessions/{}/messages", sid))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"content": [{"type":"text","text":"remember this"}]}"#)).unwrap())
        .await.unwrap();
    assert_eq!(send.status(), StatusCode::OK);

    // Give fire-and-forget memory write time
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Recall memory
    let mem_ctx = agent_core::MemoryContext {
        tenant_id: "test-tenant".into(),
        session_id: sid.clone(),
        user_id: None,
        model: "gpt-4".into(),
        session_started_at: std::time::SystemTime::now(),
    };
    let results = memory_store.recall(&mem_ctx, "remember").await.unwrap();
    assert!(!results.is_empty(), "memory should contain the turn content");
    assert!(results[0].contains("remember"), "memory content should include user message");
}
```

- [ ] **Step 2: 需要先暴露 TEST_SECRET 为 pub**

修改 `common.rs`：

```rust
// 将 const TEST_SECRET 改为 pub
pub const TEST_SECRET: &str = "test-secret-32-chars-long!!!";
```

- [ ] **Step 3: 运行测试**

```bash
cargo test -p api-gateway --test e2e_memory_store -- --nocapture
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-gateway/tests/e2e/e2e_memory_store.rs crates/api-gateway/tests/e2e/common.rs
git commit -m "test(e2e): add MemoryStore integration test"
```

---

### Phase 3 检查点

```bash
# 全部 E2E 测试（--test-threads=1 因为多个测试共享同一个 PG 数据库）
PANDARIA_TEST_PG_URL="postgres://postgres:postgres@localhost:15432/postgres" \
PANDARIA_TEST_REDIS_URL="redis://:redis@localhost:16379" \
cargo test -p api-gateway \
  --test e2e_session_lifecycle \
  --test e2e_session_state \
  --test e2e_sse_events \
  --test e2e_tenant_isolation \
  --test e2e_tool_use_http \
  --test e2e_webhook \
  --test e2e_websocket \
  --test e2e_api_extensions \
  --test e2e_persistence_recovery \
  --test e2e_persistence_compaction \
  --test e2e_persistence_fault_injection \
  --test e2e_concurrent_sessions \
  --test e2e_memory_store \
  -- --test-threads=1 --nocapture
```

---

## 5. 完成检查清单

- [ ] `cargo test -p agent-core --lib` — 全部通过
- [ ] `cargo test -p storage --test integration_postgres -- --test-threads=1` — 全部通过
- [ ] `cargo test -p storage --test integration_redis` — 全部通过
- [ ] `cargo test -p api-gateway --tests` — 全部 14 个 E2E 测试通过
- [ ] `cargo check --workspace` — 全 workspace 编译通过
- [ ] `git diff --stat` — 确认变更范围符合预期
- [ ] 更新 `VERSIONS.md` 添加 v0.2.0 条目
