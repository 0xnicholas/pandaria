# Spec: Hook Context Struct #[non_exhaustive]

## 背景

`crates/agent-core/src/hook/context.rs` 定义了 13 个 hook context struct（如 `ToolCallCtx`、`TurnEndCtx` 等），用于在 Extension trait 的 hook 方法中传递运行时上下文。这些 struct 的字段全部是 `pub`，且没有 `#[non_exhaustive]` 属性。

当前问题：当 agent-core 新增 hook context 字段时（例如 `ToolCallCtx` 增加 `metadata: Option<Value>`），所有以 struct literal 方式构造这些 struct 的外部代码都会编译失败。根据代码库审计，存在 **130+ 处**外部 struct literal 构造，分布在 extensions 和 tenant 的源码与测试文件中。

## 目标

1. 给所有 13 个 hook context struct 添加 `#[non_exhaustive]`，使新增字段不再造成外部 crate 的 breaking change
2. 为每个 struct 提供稳定的 `new()` 构造函数，作为外部 crate 的唯一构造入口
3. 批量迁移现有测试代码中的 struct literal → `new()` + 字段赋值
4. 保持现有行为的 100% 兼容性

## 设计决策

### ADR-1: #[non_exhaustive] 只应用于 context struct，不应用于 mutation struct

- **context struct**（`ToolCallCtx` 等）是框架向 Extension 传递的输入，字段由框架决定，Extension 实现者不应自行构造（除了测试）
- **mutation struct**（`ToolResultMutation`、`ToolCallMutation` 等）已经是 `#[derive(Default)]`，Extension 实现者返回它们作为输出。这些 struct 的字段全部是 `Option<T>`，新增字段天然兼容（Default 会处理）
- **结论**：mutation struct 不需要 `#[non_exhaustive]`

### ADR-2: 构造函数采用 "mandatory 参数 + pub 字段赋值" 模式

- `#[non_exhaustive]` 阻止 struct literal 构造和模式匹配，但**不阻止 pub 字段访问和修改**
- 因此外部 crate 可以调用 `new()` 获得实例，然后直接修改字段：
  ```rust
  let mut ctx = ToolCallCtx::new("t1", "s1", "tool", "call_1");
  ctx.input = json!({"key": "value"});
  ```
- 这比 builder pattern 更简洁，且与现有测试代码风格一致
- `new()` 接收所有 mandatory 字段（框架在 hook 调用时必定已知的数据），有默认值的字段在 `new()` 中赋予默认值

### ADR-3: 统一使用 `impl Into<String>` 参数类型

- 现有测试中大量构造使用 `"t1".to_string()` 等模式
- 构造函数接受 `impl Into<String>` 可减少 `.to_string()` 噪音：
  ```rust
  ToolCallCtx::new("t1", "s1", "tool", "call_1")
  ```
- 对于 `u64`、`u32`、`bool` 等基础类型保持原类型

### ADR-4: 测试 helper 不放在 agent-core 中

- 虽然测试中 `tenant_id: "t1"` 和 `session_id: "s1"` 是高频重复模式，但将测试 helper 放入 agent-core 会增加 crate 的 public API 负担
- 各 crate 的测试可自行定义 `fn test_ctx()` 等 helper
- 迁移阶段以机械替换为主，不引入新的共享 helper（避免增加理解成本）

## 构造函数设计

### 高频 struct（迁移重点）

#### `ToolCallCtx`
```rust
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ToolCallCtx {
    pub tenant_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub input: serde_json::Value,
}

impl ToolCallCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            input: serde_json::Value::Null,
        }
    }
}
```

**迁移示例：**
```rust
// 替换前
let ctx = ToolCallCtx {
    tenant_id: "t1".to_string(),
    session_id: "s1".to_string(),
    tool_name: "read".to_string(),
    tool_call_id: "call_1".to_string(),
    input: json!({"path": "/tmp"}),
};

// 替换后
let mut ctx = ToolCallCtx::new("t1", "s1", "read", "call_1");
ctx.input = json!({"path": "/tmp"});
```

#### `ToolResultCtx`
```rust
impl ToolResultCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            input: serde_json::Value::Null,
            content: vec![],
            details: None,
            is_error: false,
        }
    }
}
```

#### `TurnEndCtx`
```rust
impl TurnEndCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        turn_index: u64,
        usage: ai_provider::Usage,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            turn_index,
            messages: vec![],
            usage,
        }
    }
}
```

**注意**：`messages` 和 `usage` 在测试中经常需要自定义，但 `usage` 没有明显的默认值（`ai_provider::Usage` 本身没有 `Default`），所以放入 `new()` 参数。`messages` 默认空 vec，测试可后续赋值。

### 中频 struct

#### `ContextCtx`
```rust
impl ContextCtx {
    pub fn new(tenant_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            messages: vec![],
        }
    }
}
```

#### `AgentEndCtx`
```rust
impl AgentEndCtx {
    pub fn new(tenant_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            messages: vec![],
        }
    }
}
```

#### `SessionCtx`
```rust
impl SessionCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            system_prompt: String::new(),
            tools: vec![],
        }
    }
}
```

### 低频 struct

#### `ToolExecutionStartCtx`
```rust
impl ToolExecutionStartCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            input: serde_json::Value::Null,
        }
    }
}
```

#### `ToolExecutionEndCtx`
```rust
impl ToolExecutionEndCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            success: false,
        }
    }
}
```

#### `CompactCtx`
```rust
impl CompactCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        preparation: crate::compaction::CompactionPreparation,
        reason: CompactReason,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            preparation,
            entries: vec![],
            reason,
        }
    }
}
```

#### `CompactEndCtx`
```rust
impl CompactEndCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            compacted_messages: vec![],
            token_savings: 0,
        }
    }
}
```

#### `BeforeAgentStartCtx`
```rust
impl BeforeAgentStartCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            system_prompt: None,
            messages: vec![],
            tools: vec![],
            model: model.into(),
        }
    }
}
```

#### `ProviderRequestCtx`
```rust
impl ProviderRequestCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        model: impl Into<String>,
        turn_index: u64,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            model: model.into(),
            system_prompt: None,
            messages: vec![],
            turn_index,
            tools: None,
            options: crate::utils::provider_opts::ProviderStreamOptions::default(),
        }
    }
}
```

#### `ProviderResponseCtx`
```rust
impl ProviderResponseCtx {
    pub fn new(
        tenant_id: impl Into<String>,
        session_id: impl Into<String>,
        model: impl Into<String>,
        turn_index: u64,
        stop_reason: ai_provider::StopReason,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            model: model.into(),
            content: vec![],
            turn_index,
            attempt: 0,
            messages_before: vec![],
            stop_reason,
        }
    }
}
```

## 迁移策略

### 替换规则

每处 struct literal 按以下规则替换：

1. `StructName { field1: val1, field2: val2, ... }` → `StructName::new(mandatory_args)`
2. 如果某些字段在 `new()` 中有默认值但测试中覆盖了它们，追加字段赋值语句
3. 如果测试中使用了临时变量（如 `let tool_name = "read".to_string();`），可以内联到 `new()` 调用中

### 机械替换示例

```rust
// 替换前（extensions/tests/hook_router_tests.rs）
let ctx = ToolCallCtx {
    tenant_id: "t1".to_string(),
    session_id: "s1".to_string(),
    tool_name: "test_tool".to_string(),
    tool_call_id: "call_1".to_string(),
    input: serde_json::json!({}),
};

// 替换后
let mut ctx = ToolCallCtx::new("t1", "s1", "test_tool", "call_1");
ctx.input = serde_json::json!({});
```

```rust
// 替换前（extensions/tests/integration_router.rs）
let ctx = TurnEndCtx {
    tenant_id: "t1".to_string(),
    session_id: "s1".to_string(),
    turn_index: 1,
    messages: vec![msg1, msg2],
    usage: ai_provider::Usage { input_tokens: 10, output_tokens: 5, ... },
};

// 替换后
let mut ctx = TurnEndCtx::new("t1", "s1", 1, ai_provider::Usage { input_tokens: 10, output_tokens: 5, ... });
ctx.messages = vec![msg1, msg2];
```

## 风险

| 风险 | 概率 | 缓解措施 |
|---|---|---|
| 批量替换时遗漏某些 struct literal（如多行格式化异常） | 中 | 编译器会报错，通过 `cargo check` 逐 crate 验证 |
| `new()` 参数顺序与现有测试中的字段顺序不一致导致心理负担 | 低 | `new()` 参数顺序遵循 struct 定义中的字段声明顺序 |
| 外部第三方 Extension（未来 WASM/RPC）如果直接构造 context | 低 | ADR-002 明确当前 Extension 仅内部 Rust crate；外部 Extension 通过 RPC 序列化时不涉及 Rust struct literal |
| `ai_provider::Usage` 等外部类型没有 `Default` 导致 `new()` 签名复杂 | 低 | 这些类型放入 `new()` 的 mandatory 参数，保持显式 |

## 回滚

此改动是纯 additive（添加 `#[non_exhaustive]` 和 `new()` 方法），不涉及删除或修改现有行为。如果发现严重问题，可以：
1. 暂时移除 `#[non_exhaustive]`（保留 `new()` 方法供后续使用）
2. 或者在 `new()` 中添加 `#[doc(hidden)]` 标记为内部 API
