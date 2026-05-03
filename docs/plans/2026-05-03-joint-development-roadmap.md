# 联合开发顺序：llm-client / agent-core / extensions

**Date:** 2026-05-03
**Status:** Draft（基于三模块计划审查结果）
**Purpose:** 明确三个 crate 的开发依赖关系、执行顺序和并行机会

---

## 依赖图

```
               llm-client v0.2 P1 (P1)
                    │
                    │ 全程并行，零依赖
                    ▼
extensions ──→ agent-core Phase 0 (P0) ──→ agent-core Phase 1-9 (P0) ──→ agent-core Phase 10-11 (P1)
     │                │                           │
     │                │ 阻塞解除                   │ 可并行
     │                ▼                           ▼
     └─── 等待 Phase 0 完成 ───→ extensions Phase 1-4 (P0) ──→ extensions Phase 5-7 (P1)
                                        │
                                        │ 可并行
                                        ▼
                              llm-client v0.2 P3 (P3 — 可选)
```

---

## 执行路线图

### 第 1 周：阻塞解除（Week 1）

| 模块 | 任务 | 优先级 | 时长 | 产出 |
|---|---|---|---|---|
| **agent-core** | Phase 0: Foundation Types | **P0** | ~1h | 8 Ctx + 5 Mutation 类型 |
| llm-client | v0.2 Phase 1: 测试补全 | P1 | ~7h | 89 → 150+ tests |

**关键决策**: agent-core Phase 0 完成后立即通知 extensions 开发者启动。

**⚠️ 阻塞点**: 在 agent-core Phase 0 完成前，extensions 无法启动任何有意义的工作（除阅读 spec 和准备测试用例外）。

---

### 第 1-2 周：核心并行开发（Week 1-2）

| 模块 | 任务 | 优先级 | 时长 |
|---|---|---|---|
| **agent-core** | Phase 1-4: Events + SessionEntry + ErrorRecovery + FileOps | **P0** | ~3h |
| **extensions** | Phase 1-4: trait + Actor + EventBus + HookRouter | **P0** | ~5h |
| llm-client | v0.2 Phase 1 收尾（如尚未完成） | P1 | ~2h |

**并行机会**:
- agent-core Phase 1-4 和 extensions Phase 1-4 完全独立，可并行
- llm-client v0.2 P1 与上述两者并行

**验证检查点**:
```bash
# 第 2 周末执行
cargo build --workspace              # 全量编译通过
cargo test -p llm-client            # 测试通过
cargo test -p agent-core            # 测试通过
cargo test -p extensions            # 测试通过
```

---

### 第 2-3 周：核心功能完成（Week 2-3）

| 模块 | 任务 | 优先级 | 时长 | 风险 |
|---|---|---|---|---|
| **agent-core** | Phase 5-7: Compaction + ToolExecutor + AgentLoop | **P0** | ~8h | AgentLoop 重构高风险 |
| **extensions** | Phase 5: ExtensionManager + ExtensionTool | **P1** | ~1.5h | 低 |
| **agent-core** | Phase 8-9: Error类型 + SessionActor | **P0** | ~3.5h | SessionActor 数据模型变更 |
| **extensions** | Phase 6-7: 内置扩展 + 测试 | **P1** | ~4.5h | 中 |

**并行机会**:
- agent-core Phase 5-7 与 extensions Phase 5 并行
- agent-core Phase 8-9 与 extensions Phase 6-7 并行

**风险缓解**:
- AgentLoop 重构（Task 7.2）建议拆分为子任务，保留旧实现作为 fallback
- SessionActor 数据模型变更需更新所有现有测试

---

### 第 3-4 周：收尾与可选功能（Week 3-4）

| 模块 | 任务 | 优先级 | 时长 |
|---|---|---|---|
| agent-core | Phase 10-11: lib.rs + 文档 | P1 | ~1h |
| llm-client | v0.2 Phase 2-3: Mistral + Bedrock + OAuth | P3 | ~16h |

**建议**: llm-client v0.2 P3 在所有核心模块 P0 完成后启动，避免并行维护多个大型变更集。

---

## 优先级总览

### P0 — 阻塞级（必须先完成）

| 模块 | 任务 | 预估时长 | 阻塞谁 |
|---|---|---|---|
| agent-core | Phase 0: Foundation Types | ~1h | extensions 全部 |
| agent-core | Phase 1-9: 核心功能 | ~16h | - |
| extensions | Phase 1-4: 核心基础设施 | ~5h | - |
| **P0 总计** | | **~22h** | |

### P1 — 重要（不阻塞，但建议尽早完成）

| 模块 | 任务 | 预估时长 |
|---|---|---|
| llm-client | v0.2 Phase 1: 测试补全 + thinking_format | ~7.5h |
| agent-core | Phase 10-11: lib.rs + 文档 | ~1h |
| extensions | Phase 5-7: Manager + Builtins + Tests | ~6h |
| **P1 总计** | | **~14.5h** |

### P3 — 可选（核心模块完成后启动）

| 模块 | 任务 | 预估时长 |
|---|---|---|
| llm-client | v0.2 Phase 2: MistralProvider | ~4h |
| llm-client | v0.2 Phase 2: AwsBedrockProvider | ~10h |
| llm-client | v0.2 Phase 3: OAuth | ~2h |
| **P3 总计** | | **~16h** |

**全部总计**: ~52.5h（约 6.5 人天，假设单人全栈开发）

---

## 跨模块依赖矩阵

| 需要方 | 所需类型/接口 | 提供方 | 提供任务 | 优先级 |
|---|---|---|---|---|
| extensions | `CompactCtx` | agent-core | Phase 0.2 | **P0** |
| extensions | `BeforeAgentStartCtx` | agent-core | Phase 0.2 | **P0** |
| extensions | `ProviderRequestCtx` | agent-core | Phase 0.2 | **P0** |
| extensions | `ProviderResponseCtx` | agent-core | Phase 0.2 | **P0** |
| extensions | `ToolExecution*Ctx` | agent-core | Phase 0.2 | **P0** |
| extensions | `CompactEndCtx` | agent-core | Phase 0.2 | **P0** |
| extensions | `CompactDecision` | agent-core | Phase 0.3 | **P0** |
| extensions | `*Mutation` (5 个) | agent-core | Phase 0.3 | **P0** |
| extensions | `HookDispatcher` (14 方法) | agent-core | Phase 0.5 | **P0** |
| extensions | `AgentToolResult` | agent-core | 需确认 | **P0** |
| agent-core | `with_retry()` | llm-client | retry.rs (已存在) | P1 |
| agent-core | `AssistantMessageEventStream` | llm-client | v0.1 (已存在) | P0 |

---

## 风险与缓解

| 风险 | 影响 | 缓解措施 |
|---|---|---|
| agent-core Phase 0 延迟 | extensions 无法启动 | Phase 0 仅约 1h，建议第一天优先完成 |
| AgentLoop 重构引入 bug | agent-core 测试失效 | 拆分为子任务，保留旧实现作为 fallback |
| Bedrock Provider 复杂度超预期 | llm-client P3 延期 | 预留 10h（而非 6h），拆分为两个里程碑 |
| 类型定义冲突 | 编译失败 | agent-core 统一定义所有共享类型，extensions 直接复用 |

---

## 验证检查点（联合）

### 检查点 1：Phase 0 完成（第 1 周）
```bash
cargo build -p agent-core          # 编译通过
cargo test -p llm-client           # 89+ tests passing
# extensions 尚不能编译（等待 Phase 0）
```

### 检查点 2：核心并行完成（第 2 周）
```bash
cargo build --workspace            # 全量编译通过
cargo test --workspace             # 全量测试通过
```

### 检查点 3：P0 全部完成（第 3 周）
```bash
cargo clippy --workspace -- -D warnings  # 零 lint 警告
cargo test --workspace                   # 200+ tests passing
```

### 检查点 4：全部完成（第 4 周+）
```bash
cargo test -p llm-client --all-features  # 含 Bedrock feature
cargo build --workspace                  # 最终验证
```

---

## 相关文档

- `docs/plans/2026-05-03-agent-core-implementation.md` — agent-core 详细计划
- `docs/plans/2026-05-03-extensions-implementation.md` — extensions 详细计划
- `docs/plans/2026-05-03-llm-client-v0.2.md` — llm-client v0.2 详细计划
- `docs/specs/2026-05-02-agent-core.md` — agent-core 规格
- `docs/specs/2026-05-02-extensions.md` — extensions 规格
- `AGENTS.md` — 架构决策记录
