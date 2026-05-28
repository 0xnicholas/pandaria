# Plan: Hook Context Struct #[non_exhaustive]

> **Status:** Completed ✅ — 14 `#[non_exhaustive]` annotations applied to all context structs

## 关联 Spec

[docs/specs/hook-context-non-exhaustive.md](../specs/hook-context-non-exhaustive.md)

## 总体策略

分 3 个阶段执行，每阶段结束后 `cargo check --workspace` 验证编译：

- **Phase 1**: 在 `agent-core` 中添加 `#[non_exhaustive]` 和 `new()` 构造函数
- **Phase 2**: 迁移 `extensions` 所有测试中的 struct literal
- **Phase 3**: 迁移 `tenant` 和 `agent-core/tests/` 中的 struct literal

每阶段独立可编译通过，降低风险。

---

## Phase 1: agent-core 修改（~30 min）

### 步骤 1.1: 修改 `crates/agent-core/src/hook/context.rs`

给所有 13 个 struct 添加 `#[non_exhaustive]`，并添加 `new()` 构造函数。

**文件**: `crates/agent-core/src/hook/context.rs`
**改动量**: ~300 行（添加 `#[non_exhaustive]` + `impl` 块）
**注意**: `CompactReason` enum 也需要 `#[non_exhaustive]`（因为 `CompactCtx` 依赖它，且外部测试中可能 match 它）

**验证**:
```bash
cargo check -p agent-core
```

### 步骤 1.2: 更新 agent-core 内部构造点

`agent-core/src/` 内部（非测试）的 context struct literal 构造点是否需要修改？

**答案：不需要**。`#[non_exhaustive]` 只影响外部 crate，agent-core 内部仍然可以自由构造 struct literal。

agent-core 内部存在以下构造点（均不受影响）：
- `src/harness/tool.rs`：`ToolCallCtx`、`ToolResultCtx`
- `src/harness/agent_loop.rs`：`BeforeAgentStartCtx`、`AgentEndCtx`、`ContextCtx`、`ProviderRequestCtx`、`ProviderResponseCtx`、`TurnEndCtx`
- `src/harness/session.rs`：`SessionCtx`、`CompactCtx`

**但需要注意**：agent-core 内部的 unit tests（`#[cfg(test)]` 模块在 `src/` 下）属于 agent-core 自身，也不受影响。

### 步骤 1.3: 编译验证

```bash
cargo check -p agent-core
```

---

## Phase 2: extensions 迁移（~70 min）

### 步骤 2.1: 迁移 `extensions/src/` 中的 `#[cfg(test)]` 测试

| 文件 | Struct | 数量 |
|---|---|---|
| `src/builtins/rate_limit.rs` | `ToolCallCtx` | 5 |
| `src/builtins/content_filter.rs` | `ToolCallCtx`, `ToolResultCtx` | 3 |
| `src/builtins/path_guard.rs` | `ToolCallCtx`, `ToolResultCtx` | 4 |
| `src/builtins/token_budget.rs` | `TurnEndCtx`, `ProviderRequestCtx` | 7 |
| `src/host/hook_router.rs` | `ToolCallCtx`, `ContextCtx` | 3 |
| `src/host/extension_actor.rs` | `ToolCallCtx`, `ToolResultCtx`, `ContextCtx` | 7 |

**总计**: ~29 处

**替换策略**: 使用 Python/sed 脚本批量替换，然后手工 review。

**验证**:
```bash
cargo test -p extensions --lib
```

### 步骤 2.2: 迁移 `extensions/tests/` 中的 integration tests

| 文件 | Struct | 数量 |
|---|---|---|
| `builtin_audit_tests.rs` | `ToolCallCtx`, `ToolResultCtx`, `TurnEndCtx` | 3 |
| `builtin_rate_limit_tests.rs` | `ToolCallCtx` | 3 |
| `builtin_rate_limit_concurrent_tests.rs` | `ToolCallCtx` | 4 |
| `builtin_tool_guard_tests.rs` | `ToolCallCtx` | 4 |
| `extension_actor_tests.rs` | `ToolCallCtx`, `ToolResultCtx`, `ContextCtx` | 9 |
| `extension_manager_tests.rs` | `ToolCallCtx` | 2 |
| `hook_router_tests.rs` | `ToolCallCtx`, `ToolResultCtx`, `TurnEndCtx`, `AgentEndCtx`, `SessionCtx`, `ContextCtx` | 18 |
| `hook_router_mutation_tests.rs` | `ToolCallCtx` | 2 |
| `hook_router_observation_tests.rs` | `ToolExecutionStartCtx`, `ToolExecutionEndCtx`, `CompactEndCtx` | 3 |
| `hook_router_compact_tests.rs` | `CompactCtx` | 1 |
| `integration_router.rs` | `ToolCallCtx`, `ToolResultCtx`, `TurnEndCtx`, `AgentEndCtx`, `SessionCtx`, `ContextCtx` | 12 |
| `integration_multi_ext.rs` | `ToolCallCtx`, `ToolResultCtx`, `TurnEndCtx`, `AgentEndCtx`, `SessionCtx`, `ContextCtx` | 14 |
| `integration_tool_execution_hooks.rs` | `ToolExecutionStartCtx`, `ToolExecutionEndCtx` | 4 |
| `integration_lifecycle_hooks.rs` | `ToolExecutionStartCtx`, `ToolExecutionEndCtx` | 2 |

**总计**: ~80 处

**替换策略**: 由于 integration tests 中的 struct literal 格式更多样（有些字段引用临时变量），建议分文件逐个替换，使用 `cargo test -p extensions --test <test_name>` 逐个验证。

**验证**:
```bash
cargo test -p extensions
```

---

## Phase 3: tenant 和 agent-core/tests 迁移（~30 min）

### 步骤 3.1: 迁移 `tenant/tests/`

| 文件 | Struct | 数量 |
|---|---|---|
| `quota_extension.rs` | `ToolCallCtx` | 3 |
| `token_meter.rs` | `TurnEndCtx` | 2 |

**总计**: 5 处

### 步骤 3.2: 迁移 `agent-core/tests/`

| 文件 | Struct | 数量 |
|---|---|---|
| `hook_dispatcher_tests.rs` | `ToolCallCtx`, `ToolResultCtx`, `TurnEndCtx`, `AgentEndCtx`, `SessionCtx`, `ContextCtx` | 7 |

**总计**: 7 处

**验证**:
```bash
cargo test -p tenant
cargo test -p agent-core
```

---

## Phase 4: 最终验证（~10 min）

### 步骤 4.1: Workspace 编译

```bash
cargo check --workspace
```

### 步骤 4.2: Workspace 测试

```bash
cargo test --workspace --exclude storage
```

（storage 的 PostgreSQL 测试需要 Docker，与本次改动无关）

### 步骤 4.3: 确认无遗漏

```bash
for struct in ToolCallCtx ToolResultCtx TurnEndCtx AgentEndCtx SessionCtx ContextCtx ToolExecutionStartCtx ToolExecutionEndCtx CompactCtx CompactEndCtx BeforeAgentStartCtx ProviderRequestCtx ProviderResponseCtx; do
  echo "=== $struct ==="
  grep -rn "${struct} {" crates/ --include="*.rs" | grep -v "agent-core/src/hook/context.rs"
done
```

如果返回空，说明所有外部 struct literal 已清理完毕。

---

## 预估时间

| 阶段 | 预估时间 | 依赖 |
|---|---|---|
| Phase 1: agent-core | ~30 min | 无 |
| Phase 2: extensions | ~70 min | Phase 1 |
| Phase 3: tenant + agent-core/tests | ~30 min | Phase 1 |
| Phase 4: 验证 | ~10 min | Phase 2, 3 |
| **总计** | **~2.5 h** | — |

> 注：外部 struct literal 实际约 121 处（原 Plan 统计 ~114 处），增量主要来自 `integration_multi_ext.rs` 和 `extension_actor_tests.rs`。

## 执行建议

1. **不要试图一次性完成所有替换**。Phase 1 完成后先 `cargo check -p agent-core`，确认编译通过再继续
2. **extensions 测试数量大，建议分 2-3 个 commit**：
   - commit 1: `extensions/src/` 的 `#[cfg(test)]` 模块
   - commit 2: `extensions/tests/` 的 integration tests（前半）
   - commit 3: `extensions/tests/` 的 integration tests（后半）
3. **使用 Python 脚本做机械替换**，但务必逐文件 `cargo check` 验证
4. **遇到特殊格式**（如 `ProviderRequestCtx` 只有 1 处，且字段复杂），可以手工处理

## 回滚方案

如果在任何阶段遇到无法快速修复的编译错误：
1. 保留已完成阶段的修改
2. 对未完成的 struct 暂时不添加 `#[non_exhaustive]`（仅添加 `new()` 方法）
3. 待问题修复后再统一加 `#[non_exhaustive]`

`#[non_exhaustive]` 和 `new()` 是解耦的——可以单独存在。
