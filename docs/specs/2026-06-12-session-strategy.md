# SessionStrategy — 三维执行策略

> Status: Draft  
> Date: 2026-06-12  
> Target: agent-core v0.3+

---

## 1. 设计

每个维度对应一个 CLI 命令。单独看是功能，组合起来是 session 的运行策略：

| 维度 | 命令 | 策略语义 | 默认值 |
|------|------|----------|--------|
| termination | `/goal` | "做到 X 为止" | `Once` — 跑一次就停 |
| rhythm | `/loop` | "每隔 X 时间做一次" | `Once` — 立刻执行 |
| context | `/compact` `/clear` | "压缩记忆继续跑" / "从零开始新一轮" | `Accumulate` — 保留全部历史 |

```
                ┌─ Once         → 跑一次就停
termination ────┤
  (/goal)        └─ Goal         → 做到 X 为止

                ┌─ Once         → 立刻执行
rhythm ─────────┤
  (/loop)        └─ Loop         → 每隔 X 时间做一次

                ┌─ Accumulate   → 保留全部历史
context ────────┤
  (/compact)     ├─ Compact      → 压缩记忆，继续跑
  (/clear)       └─ Clear        → 从零开始新一轮
```

### 1.1 Loop 关键特性

| 特性 | 说明 |
|------|------|
| **后台执行** | `prompt()` 立即返回首轮结果，后续迭代在后台 `tokio::spawn` 中运行 |
| **上下文控制** | 每次迭代的上下文行为由 `ContextStrategy` 决定：`Clear` = 独立上下文（不撑爆窗口），`Accumulate` = 累积全量历史，`Compact` = 自动压缩 |
| **默认间隔** | 未指定 `interval` 时默认 **10 分钟** |
| **全局开关** | `PANDARIA_DISABLE_CRON=1` 禁用调度器，所有 Loop 不可用 |

| 组合示例 | 行为 |
|----------|------|
| Once + Once + Accumulate | 默认：跑一次，保留历史 |
| Goal + Once + Accumulate | 目标驱动自验证，历史累积 |
| Once + Loop + Clear | 每 30s 跑一次，每次 fresh context |
| Goal + Loop + Clear | 轮询直到满足验收标准，每次独立上下文 |
| Once + Once + Clear | 无状态执行，跑完清空 |

---

## 2. 类型定义

### 2.1 SessionStrategy

```rust
#[derive(Debug, Clone)]
pub struct SessionStrategy {
    pub termination: TerminationStrategy,
    pub rhythm: RhythmStrategy,
    pub context: ContextStrategy,
}

impl Default for SessionStrategy {
    fn default() -> Self {
        Self {
            termination: TerminationStrategy::Once,
            rhythm: RhythmStrategy::Once,
            context: ContextStrategy::Accumulate,
        }
    }
}
```

### 2.2 TerminationStrategy

```rust
#[derive(Debug, Clone)]
pub enum TerminationStrategy {
    Once,

    Goal {
        criteria: Vec<GoalCriterion>,
        max_attempts: u32,
        on_exhausted: GoalExhaustedAction,
    },
}

#[derive(Debug, Clone)]
pub struct GoalCriterion {
    pub id: String,
    pub description: String,
    pub verification: GoalVerification,
}

#[derive(Debug, Clone)]
pub enum GoalVerification {
    /// Agent 自我评估，必须在输出中写入 [CRITERION_RESULT: id: PASS|FAIL]
    SelfAssessment,
    /// 运行命令，exit 0 = 通过（不需要 agent 参与判断）
    Command { command: String },
    /// 检查输出是否包含指定文本（不需要 agent 参与判断）
    OutputContains { text: String },
}

#[derive(Debug, Clone)]
pub enum GoalExhaustedAction {
    Abort,
    /// 再跑一次原始 task（不走验证 prompt），返回结果
    ReturnLast,
}

#[derive(Debug, Clone)]
pub enum GoalOutcome {
    Passed { messages: Vec<AgentMessage>, attempts: u32 },
    Exhausted { messages: Vec<AgentMessage>, attempts: u32 },
}
```

### 2.3 RhythmStrategy

```rust
pub const DEFAULT_LOOP_INTERVAL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub enum RhythmStrategy {
    Once,

    /// `PANDARIA_DISABLE_CRON=1` 时 Loop 直接返回 LoopDisabled 错误。
    Loop {
        interval: Option<Duration>,
        max_iterations: Option<u32>,
    },
}
```

### 2.4 ContextStrategy

```rust
#[derive(Debug, Clone)]
pub enum ContextStrategy {
    Accumulate,
    /// 每次 run 前压缩，保留最近 N 条 SessionEntry。
    /// 实际压缩委托给 CompactionActor，不重复实现。
    Compact { keep_last_n: usize },
    /// 每次 run 前清空全部 entries + 重建 PromptBuilder。
    Clear,
}
```

### 2.5 API 序列化

`interval` 在 JSON 中以 `interval_ms: u64` 传输，api-gateway 负责与 `Option<Duration>` 互转。

---

## 3. 执行流

### 3.1 分派表

| termination | rhythm | 行为 |
|-------------|--------|------|
| Once | Once | `run_with_messages(task)` — 现有默认行为 |
| Goal | Once | `run_goal_sync(task)` — 同步验证循环 |
| Once | Loop | 首轮同步返回 → spawn 后台循环 |
| Goal | Loop | 首轮同步返回 → spawn 后台循环（每轮内部走 Goal 验证） |

```rust
async fn prompt(&mut self, task: String) -> Result<Vec<AgentMessage>> {
    match (&self.strategy.termination, &self.strategy.rhythm) {
        (TerminationStrategy::Once, RhythmStrategy::Once) => {
            self.run_with_messages(Some(task)).await
        }
        (TerminationStrategy::Goal { .. }, RhythmStrategy::Once) => {
            let outcome = self.run_goal_sync(task).await?;
            Ok(outcome.into_messages())
        }

        (_, RhythmStrategy::Loop { interval, max_iterations }) => {
            if std::env::var("PANDARIA_DISABLE_CRON")
                .as_deref() == Ok("1") {
                return Err(AgentError::LoopDisabled);
            }
            let delay = interval.unwrap_or(DEFAULT_LOOP_INTERVAL);

            let first = match &self.strategy.termination {
                TerminationStrategy::Once =>
                    self.run_with_messages(Some(task.clone())).await?,
                TerminationStrategy::Goal { .. } =>
                    self.run_goal_sync(task.clone()).await?.into_messages(),
            };

            self.spawn_background_loop(task, delay, *max_iterations);
            Ok(first)
        }
    }
}
```

### 3.2 后台循环

```rust
fn spawn_background_loop(&self, task: String, delay: Duration, max: Option<u32>) {
    let abort = self.abort_token.clone();
    let event_tx = self.event_tx.clone();
    let termination = self.strategy.termination.clone();
    // ... clone other needed state

    tokio::spawn(async move {
        let mut iteration: u32 = 1;

        loop {
            if abort.is_cancelled() { break; }
            if let Some(max) = max && iteration >= max { break; }

            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = abort.cancelled() => { break; }
            }
            if abort.is_cancelled() { break; }

            iteration += 1;

            let result = match &termination {
                TerminationStrategy::Once =>
                    run_with_messages(Some(task.clone())).await,
                TerminationStrategy::Goal { .. } =>
                    run_goal_sync(task.clone()).await.map(|o| o.into_messages()),
            };

            match result {
                Ok(msgs) => {
                    let _ = event_tx.send(AgentEvent::LoopIterationComplete {
                        iteration, messages: msgs,
                    });
                }
                Err(e) => {
                    tracing::error!(iteration, error = %e,
                        "background iteration failed, continuing");
                    let _ = event_tx.send(AgentEvent::LoopIterationError {
                        iteration, error: e.to_string(),
                    });
                }
            }
        }
    });
}
```

| 规则 | 行为 |
|------|------|
| 单轮失败 | error 日志 + `LoopIterationError` 事件 → **继续**下一轮 |
| Cancelled | 退出 loop |
| 结果可观测 | 首轮：`prompt()` 返回值；后续轮：SSE event stream |

### 3.3 Goal 同步验证

```rust
async fn run_goal_sync(&mut self, task: String) -> Result<GoalOutcome> {
    let (criteria, max_attempts, on_exhausted) = match &self.strategy.termination {
        TerminationStrategy::Goal { criteria, max_attempts, on_exhausted } =>
            (criteria.clone(), *max_attempts, on_exhausted.clone()),
        _ => unreachable!(),
    };

    for attempt in 0..max_attempts {
        self.apply_context_strategy_before_run().await?;

        let prompt = if attempt == 0 {
            build_initial_goal_prompt(&task, &criteria)       // §3.4
        } else {
            build_retry_prompt(&task, &criteria, attempt, max_attempts, &last_result)
        };

        let result = self.run_with_messages(Some(prompt)).await?;

        // 运行 Command/OutputContains verification，解析 SelfAssessment
        let eval = self.evaluate_criteria(&result, &criteria).await;
        if eval.all_passed() {
            return Ok(GoalOutcome::Passed {
                messages: result,
                attempts: attempt + 1,
            });
        }
        // 保存评估结果，用于下一轮的重试 prompt
        last_result = eval;
    }

    match on_exhausted {
        GoalExhaustedAction::Abort => Err(AgentError::GoalNotMet {
            criteria: criteria.iter().map(|c| c.id.clone()).collect(),
            attempts: max_attempts,
        }),
        GoalExhaustedAction::ReturnLast => {
            // 最后一轮用原始 task（不走验证 prompt），给 agent 干净机会
            let result = self.run_with_messages(Some(task)).await?;
            Ok(GoalOutcome::Exhausted {
                messages: result,
                attempts: max_attempts,
            })
        }
    }
}
```

### 3.4 Goal 验证 prompt

**首轮** — 注入 criteria + 要求结构化输出：

```
## Task
{task}

## Acceptance Criteria
Satisfy ALL of the following:

1. [tests-pass] All tests pass — `cargo test` must exit 0
2. [no-unwrap] No unwrap() calls remain in production code

After completing, end with:

[CRITERION_RESULT: tests-pass: PASS|FAIL]
[CRITERION_RESULT: no-unwrap: PASS|FAIL]
```

**重试** — 注入上一轮的实际评估结果（由框架运行 verification 得出，非 agent 自报）：

```
## Acceptance Criteria Check (attempt 2/5)

✗ tests-pass — `cargo test` returned exit code 1
✓ no-unwrap

Fix the failing criteria. End with [CRITERION_RESULT: ...] as before.
```

注入方式：`PromptBuilder::upsert_fragment`，`FragmentKind::RuntimeInjection`，priority 200。

### 3.5 验证执行

```rust
async fn evaluate_criteria(&self, result: &[AgentMessage], criteria: &[GoalCriterion])
    -> CriteriaEvaluation
{
    let mut results = Vec::new();
    for c in criteria {
        let passed = match &c.verification {
            GoalVerification::SelfAssessment => {
                parse_criterion_result(result, &c.id).unwrap_or(false)
            }
            GoalVerification::Command { command } => {
                run_command(command).await.success()
            }
            GoalVerification::OutputContains { text } => {
                assistant_text(result).contains(text.as_str())
            }
        };
        results.push((c.id.clone(), passed));
    }
    CriteriaEvaluation { results }
}
```

**关键**：`Command` 和 `OutputContains` 验证由**框架执行**，不依赖 agent 自评。`SelfAssessment` 才解析 agent 的输出标记。三种 verification 可以混合——criteria 里既有 `Command` 也有 `SelfAssessment`。

### 3.6 Context 策略

```rust
async fn apply_context_strategy_before_run(&mut self) -> Result<(), AgentError> {
    match &self.strategy.context {
        ContextStrategy::Accumulate => {}
        ContextStrategy::Compact { keep_last_n } => {
            // 委托给现有 CompactionActor，不重复实现压缩逻辑
            self.compaction_actor.compact_to_n(&mut self.entries, *keep_last_n).await?;
        }
        ContextStrategy::Clear => {
            self.entries.clear();
            self.prompt_builder =
                PromptBuilder::from_base(self.base_persona.clone());
            crate::skills::inject_skills_into_builder(
                &mut self.prompt_builder, &self.skills,
            );
        }
    }
    Ok(())
}
```

**注意**：`Compact` 策略复用现有的 `CompactionActor`（已支持 `keep_recent_tokens` 配置）。不在 strategy 层重新实现压缩——它调用 compactor 的现有 API，确保与自动 threshold compaction 的行为一致。

---

## 4. 与现有系统的关系

```
SessionStrategy (新增)
  │
  │  "跑几次、多久跑一次、记多少"
  │
  └→ run_with_messages() (现有)
       │
       │  每轮都走完整 hook 链路 + CompactionActor 自动压缩
       │
       ├── on_before_agent_start
       ├── on_tool_call / on_tool_result
       ├── on_before_provider_request / on_after_provider_response
       ├── on_turn_end / on_agent_end
       └── CompactionActor::should_compact() (threshold-based)
```

- **Hook 系统**：不知道 strategy 的存在——每轮 run 就是一个普通 agent 执行。
- **CompactionActor**：threshold-based 自动压缩**仍然工作**。`ContextStrategy::Compact` 是额外的策略层压缩（保留 N 条），和自动压缩互补不冲突——自动压缩在 run 内部触发，策略压缩在 run 之间触发。
- **MemoryStore**：不变。`remember()` / `recall()` 在每轮 run 的 hook 中照常触发。

---

## 5. API

```json
{
  "strategy": {
    "termination": {
      "type": "goal",
      "criteria": [
        {"id": "deploy-ok", "description": "Deployment healthy",
         "verification": {"type": "command", "command": "curl -s deploy/status | grep healthy"}}
      ],
      "max_attempts": 10,
      "on_exhausted": "abort"
    },
    "rhythm": {
      "type": "loop",
      "interval_ms": 30000,
      "max_iterations": null
    },
    "context": {
      "type": "clear"
    }
  }
}
```

---

## 6. 实施

| Phase | 内容 | ~行数 |
|-------|------|------|
| 1 | 类型定义：`SessionStrategy` + 子 enum + `GoalOutcome` + `CriteriaEvaluation` | 130 |
| 2 | `SessionActor`: `strategy` 字段 + 分派逻辑 + `apply_context_strategy_before_run` | 120 |
| 3 | `run_goal_sync` + prompt 构建 + `evaluate_criteria` | 100 |
| 4 | `spawn_background_loop` + `AgentEvent::LoopIterationComplete/Error` | 80 |
| 5 | API gateway types + `interval_ms` ↔ `Duration` | 50 |
| 6 | 测试 | 100 |

---

## 7. 不做

| 不做 | 原因 |
|------|------|
| 跨 session 编排 | Tavern 职责 |
| 自定义 termination callback | Phase 1 仅 built-in |
| `Compact` 策略自行实现压缩 | 复用现有 `CompactionActor` |
