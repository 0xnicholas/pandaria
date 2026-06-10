# Tools Module Refactor

> Status: Draft  
> Date: 2026-06-09  
> Updated: 2026-06-09 (review 2: minimal scope — cut ToolRegistry, cut type changes)  
> Target: agent-core v0.2+

---

## 1. 问题陈述

| 问题 | 细节 |
|------|------|
| **trait 位置不对** | `AgentTool` / `AgentToolResult` / `ToolExecutionMode` 在 `types.rs`，与 `AgentMessage` 混在一起 |
| **缺少内置工具入口** | `SessionBuilder` 只有 `with_external_tools()`，没有 `with_builtin_tools()` |
| **去重逻辑缺失** | builtin / media / external 三类工具直接 `Vec::push`，无名称冲突处理 |
| **工具函数散落** | `build_tool_defs()` / `build_tool_value_defs()` 在 `agent_loop.rs` 中是私有函数 |

---

## 2. 目标

| ID | 目标 |
|----|------|
| G1 | 将 `AgentTool` trait 及附属类型迁入 `tools/types.rs`，`types.rs` re-export 保持兼容 |
| G2 | `SessionBuilder` 新增 `with_builtin_tools()`，`build()` 内按优先级合并去重 |
| G3 | `build_tool_defs()` / `build_tool_value_defs()` 提取到 `tools/` 为公共函数 |
| G4 | 零 API breakage：所有现有 struct literal、测试、外部调用方不改动直接编译 |

---

## 3. 模块结构

```
agent-core/src/tools/
  ├── mod.rs              ← pub mod + pub use
  ├── types.rs            ← AgentTool trait, AgentToolResult, AgentToolProgressUpdate, ToolExecutionMode（迁入）
  ├── helpers.rs          ← build_tool_defs(), build_tool_value_defs()（从 agent_loop 提取）
  ├── http_proxy.rs       ← 不变
  └── media_generation.rs ← 不变
```

### 3.1 `tools/types.rs`

从 `crate::types` 迁入，内容不变：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolExecutionMode { Sequential, #[default] Parallel }

#[derive(Debug, Clone)]
pub struct AgentToolProgressUpdate { pub content: String }

#[derive(Debug, Clone)]
pub struct AgentToolResult { ... }

#[async_trait]
pub trait AgentTool: Send + Sync { ... }

pub type AgentToolRef = Arc<dyn AgentTool>;
```

### 3.2 `tools/helpers.rs` — 从 `agent_loop.rs` 提取

```rust
use crate::types::AgentToolRef;
use ai_provider::ToolDef;

/// Build the LLM function-definition list from a tool set.
pub fn build_tool_defs(tools: &[AgentToolRef]) -> Option<Vec<ToolDef>> {
    if tools.is_empty() { return None; }
    Some(tools.iter().map(|t| ToolDef {
        name: t.name().to_string(),
        description: t.description().to_string(),
        parameters: t.parameters(),
    }).collect())
}

/// Build serde_json::Value representations for hook contexts.
pub fn build_tool_value_defs(tools: &[AgentToolRef]) -> Vec<serde_json::Value> {
    tools.iter().map(|t| json!({
        "name": t.name(),
        "description": t.description(),
        "parameters": t.parameters(),
    })).collect()
}
```

### 3.3 `tools/mod.rs`

```rust
pub mod helpers;
pub mod http_proxy;
pub mod media_generation;
pub mod types;

pub use helpers::{build_tool_defs, build_tool_value_defs};
pub use http_proxy::{HttpProxyTool, ToolConfig};
pub use media_generation::MediaGenerationTool;
pub use types::*;
```

---

## 4. `crate::types` re-export

```rust
// agent-core/src/types.rs
pub use crate::tools::{
    AgentTool, AgentToolProgressUpdate, AgentToolRef, AgentToolResult,
    ToolExecutionMode,
};
pub type AgentMessage = ai_provider::Message;
pub use crate::persistence::entry::{...};
```

所有现有 `use crate::types::AgentTool` 路径不改动直接编译。

---

## 5. `agent_loop.rs` 清理

```diff
- fn build_tool_defs(tools: &[AgentToolRef]) -> Option<Vec<ai_provider::ToolDef>> { ... }
- fn build_tool_value_defs(tools: &[AgentToolRef]) -> Vec<serde_json::Value> { ... }

+ use crate::tools::{build_tool_defs, build_tool_value_defs};
```

调用处不变，只是函数来源变了。

---

## 6. SessionBuilder 变更

### 6.1 字段

```diff
  pub struct SessionBuilder {
      config: HarnessConfig,
      tenant_id: String,
      session_id: String,
      system_prompt: String,
      model: String,
-     external_tools: Vec<ToolConfig>,
+     builtin_tools: Vec<AgentToolRef>,
+     external_tools: Vec<ToolConfig>,
  }
```

### 6.2 新增方法

```rust
/// Register built-in tools implemented in-process.
///
/// Registered before media generation and external tools. External tools
/// with the same name can intentionally shadow builtins.
pub fn with_builtin_tools(mut self, tools: Vec<AgentToolRef>) -> Self {
    self.builtin_tools = tools;
    self
}
```

### 6.3 `build()` 中工具组装逻辑

```rust
pub async fn build(self) -> Result<BuiltSession, AgentError> {
    // ... hook dispatcher, skills 不变 ...

    // 2. 工具组装：builtin → media → external（后面覆盖前面同名）
    use std::collections::HashSet;
    let mut tools: Vec<AgentToolRef> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 2a. Built-in tools
    for tool in &self.builtin_tools {
        let name = tool.name().to_string();
        if seen.contains(&name) {
            tracing::warn!(%name, "builtin tool name collision, keeping first");
            continue;
        }
        seen.insert(name);
        tools.push(tool.clone());
    }

    // 2b. Media generation tool（如果配置了 media provider）
    if let (Some(media_provider), Some(media_registry)) = (...) {
        let media_tool = Arc::new(MediaGenerationTool::new(
            media_provider.clone(),
            media_registry.clone(),
            self.model.clone(),
            &tenant_id,
        ));
        let name = media_tool.name().to_string();
        if seen.contains(&name) {
            tracing::warn!(%name, "media tool shadowed by builtin");
        } else {
            seen.insert(name);
        }
        tools.push(media_tool);
    }

    // 2c. External HTTP proxy tools（最后，可覆盖前面所有同名）
    for tc in &self.external_tools {
        let proxy = Arc::new(HttpProxyTool::new(
            tc.clone(),
            tenant_id.clone(),
            session_id.clone(),
            self.config.http_client.clone(),
        ));
        let name = proxy.name().to_string();
        if seen.contains(&name) {
            tracing::info!(%name, "external tool shadows earlier definition");
        }
        seen.insert(name);
        tools.push(proxy);
    }

    // 3. Session actor
    let actor = SessionActor::new(SessionConfig {
        tools: tools.clone(),
        ...  // 其余字段不变
    });

    Ok(BuiltSession { actor, tools })
}
```

**所有现有 struct 类型不变**：`SessionConfig.tools: Vec<AgentToolRef>` 不变，`AgentLoopConfig.tools: Vec<AgentToolRef>` 不变，`BuiltSession { actor, tools: Vec<AgentToolRef> }` 不变。

---

## 7. 向后兼容

| 接口 | 是否变更 |
|------|---------|
| `AgentTool` trait | ❌ 内容不变，位置迁移，`types.rs` re-export |
| `SessionConfig` | ❌ 字段类型不变 |
| `AgentLoopConfig` | ❌ 字段类型不变 |
| `SessionBuilder` | ✅ 新增 `builtin_tools` 字段 + `with_builtin_tools()` 方法（additive） |
| `ToolExecutor` | ❌ 不动 |
| 所有现有测试 | ❌ 不改任何 import |

---

## 8. 测试策略

| 层级 | 内容 | 文件 |
|------|------|------|
| 单元 | `build_tool_defs` 空列表返回 None | `tools/helpers.rs` `#[cfg(test)]` |
| 单元 | `build_tool_defs` 非空列表正确生成 | 同上 |
| 单元 | `build_tool_value_defs` 生成正确 | 同上 |
| 集成 | `SessionBuilder` builtin + external 混合，同名覆盖 | `harness/builder.rs` `mod tests` |
| 集成 | `SessionBuilder::with_builtin_tools()` 空列表不报错 | 同上 |

---

## 9. 实施顺序

| Phase | 内容 | 预计改动 |
|-------|------|---------|
| 1 | 创建 `tools/types.rs`，迁入 trait + 类型；`types.rs` 加 re-export | ~60 行迁入 + ~5 行 re-export |
| 2 | 创建 `tools/helpers.rs`，从 `agent_loop.rs` 迁入两个函数；更新 `agent_loop.rs` 的 use | ~30 行 |
| 3 | `SessionBuilder` 加 `builtin_tools` + `with_builtin_tools()` + build 中合并去重 | ~35 行 |
| 4 | 全量测试 | — |

总计 ~130 行改动，三个文件新增，三个文件修改。无类型变更，无 API breakage。

---

## 10. 不做的事（明确排除）

| 不做 | 原因 |
|------|------|
| `ToolRegistry`（HashMap + Vec 双重索引） | 当前无足够消费方。按名查找只在 `agent_loop.rs` 一处，`HashSet<String>` 满足去重需求。等 compaction / HookDispatcher 需要查工具元数据时再引入 |
| `SessionConfig.tools` 类型变更 | 破坏 ~15 个 struct literal 调用点，收益为零 |
| `ToolExecutor` 迁移或改动 | 职责正确（单工具流水线），不需要动 |
| 工具执行模式 batch 逻辑 | 当前在 `AgentLoop::execute_tools()` 中，位置正确 |
