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
| **后台执行** | Loop 启动后立即返回，后续迭代在后台运行。会话保持活跃直到手动停止或 session 结束 |
| **独立上下文** | 每次迭代拥有独立的上下文——不会把上一次迭代的消息堆到下一次，long-running loop 不会撑爆 context window |
| **默认间隔** | 未指定 `interval` 时默认 **10 分钟** |
| **全局开关** | `PANDARIA_DISABLE_CRON=1` 禁用调度器，所有 Loop 不可用 |

| 组合示例 | 行为 |
|----------|------|
| Once + Once + Accumulate | 默认：跑一次，保留历史 |
| Goal + Once + Accumulate | 目标驱动自验证，历史累积 |
| Once + Loop + Compact | 每 30s 跑一次，自动压缩 |
| Goal + Loop + Compact | 轮询直到满足验收标准，每次记得压缩 |
| Once + Once + Clear | 无状态执行，每次 fresh start |

---

## 2. 类型定义

### 2.1 SessionStrategy

```rust
/// 三维执行策略。
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
    /// 默认：agent 返回 stop 就结束。
    Once,

    /// 目标驱动：agent 自我验证是否满足验收标准。
    /// 每轮 run 结束后注入验证 prompt，直到通过或超限。
    Goal {
        criteria: Vec<GoalCriterion>,
        /// 最大尝试次数（含首轮）。
        max_attempts: u32,
        /// 超限后的行为。
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
    /// Agent 自我评估。要求 agent 输出 `[CRITERION_RESULT: id: PASS|FAIL]` 标记。
    SelfAssessment,
    /// 运行命令，exit 0 = 通过。
    Command { command: String },
    /// 上一个 assistant message 包含指定文本。
    OutputContains { text: String },
}

#[derive(Debug, Clone)]
pub enum GoalExhaustedAction {
    Abort,
    ReturnLast,
}

/// Goal 执行结果，可区分"通过"和"耗尽次数"。
#[derive(Debug, Clone)]
pub enum GoalOutcome {
    Passed { messages: Vec<AgentMessage>, attempts: u32 },
    Exhausted { messages: Vec<AgentMessage>, attempts: u32 },
}
```

### 2.3 RhythmStrategy

```rust
/// 默认 Loop 间隔：10 分钟。
pub const DEFAULT_LOOP_INTERVAL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub enum RhythmStrategy {
    /// 默认：立即执行一次。
    Once,

    /// 后台循环执行。
    ///
    /// 首次 prompt 调用立即返回，后续迭代在后台运行。
    /// 每个后台迭代的结果通过 SSE 事件推送。
    ///
    /// 当 `PANDARIA_DISABLE_CRON=1` 时，Loop 请求直接返回错误。
    Loop {
        /// 两次迭代之间的等待时间。省略时默认 10 分钟。
        interval: Option<Duration>,
        /// 最大迭代次数。None = 无限（直到终止条件触发或手动 abort）。
        max_iterations: Option<u32>,
    },
}
```

### 2.4 ContextStrategy

```rust
#[derive(Debug, Clone)]
pub enum ContextStrategy {
    /// 默认：保留全部会话历史。
    Accumulate,

    /// 每次 run 前自动压缩，保留最近 N 条 SessionEntry。
    /// 被压缩的历史通过 CompactionActor 生成 summary 注入 prompt。
    Compact {
        keep_last_n: usize,  // 保留的 SessionEntry 数量
    },

    /// 每次 run 前清空全部历史，下一次 run 从空白上下文开始。
    Clear,
}
```

### 2.5 API 序列化说明

`interval` 字段在 API 中以毫秒整数 (`interval_ms`) 传输，Rust 侧为 `Option<Duration>`。api-gateway types 层负责转换：

```rust
#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
enum RhythmStrategyApi {
    Once,
    Loop {
        #[serde(default)]
        interval_ms: Option<u64>,  // JSON 整数
        max_iterations: Option<u32>,
    },
}
```

---

## 3. SessionActor 执行流

### 3.1 分派表

`prompt()` 根据三个维度的组合选择执行路径：

| termination | rhythm | 行为 |
|-------------|--------|------|
| Once | Once | `run_once(task)` — 现有默认行为 |
| Goal | Once | `run_goal_sync(task)` — 同步验证循环 |
| Once | Loop | 首轮同步返回 → spawn 后台循环 |
| Goal | Loop | 首轮同步返回 → spawn 后台循环（每轮内部走 Goal 验证） |

```rust
async fn prompt(&mut self, task: String) -> Result<Vec<AgentMessage>> {
    match (&self.strategy.termination, &self.strategy.rhythm) {
        // ── 同步路径 ──
        (TerminationStrategy::Once, RhythmStrategy::Once) => {
            self.run_once(task).await
        }
        (TerminationStrategy::Goal { .. }, RhythmStrategy::Once) => {
            self.run_goal_sync(task).await.map(|outcome| outcome.messages())
        }

        // ── 后台路径 ──
        (_, RhythmStrategy::Loop { interval, max_iterations }) => {
            if std::env::var("PANDARIA_DISABLE_CRON").as_deref() == Ok("1") {
                return Err(AgentError::LoopDisabled);
            }
            let delay = interval.unwrap_or(DEFAULT_LOOP_INTERVAL);

            // 首轮同步执行，立即返回结果给调用方
            let first = match &self.strategy.termination {
                TerminationStrategy::Once => self.run_one_iteration(task.clone()).await?,
                TerminationStrategy::Goal { .. } =>
                    self.run_goal_sync(task.clone()).await?.messages(),
            };

            // 后续迭代在后台运行，结果通过 SSE 推送
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
    let context = self.strategy.context.clone();
    // ... clone other needed state

    tokio::spawn(async move {
        let mut iteration: u32 = 1; // 首轮已由 prompt() 执行

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
                TerminationStrategy::Once => run_one_iteration(...).await,
                TerminationStrategy::Goal { .. } =>
                    run_goal_sync(...).await.map(|o| o.messages()),
            };

            match result {
                Ok(msgs) => {
                    let _ = event_tx.send(AgentEvent::LoopIterationComplete {
                        iteration,
                        messages: msgs,
                    });
                }
                Err(e) => {
                    tracing::error!(iteration, error = %e,
                        "background loop iteration failed, continuing");
                    let _ = event_tx.send(AgentEvent::LoopIterationError {
                        iteration,
                        error: e.to_string(),
                    });
                    // 继续下一轮，不停止 loop
                }
            }
        }
    });
}
```

| 规则 | 行为 |
|------|------|
| 单轮失败 | 记录 error 日志 + 推送 `LoopIterationError` → **继续**下一轮 |
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
        self.apply_context_strategy().await?;

        let prompt = if attempt == 0 {
            self.build_initial_goal_prompt(&task, &criteria)
        } else {
            self.build_retry_prompt(&task, &criteria, attempt, max_attempts)
        };

        let result = self.run_with_messages(Some(prompt)).await?;

        if self.goal_satisfied(&result, &criteria).await {
            return Ok(GoalOutcome::Passed { messages: result, attempts: attempt + 1 });
        }
    }

    match on_exhausted {
        GoalExhaustedAction::Abort => Err(AgentError::GoalNotMet {
            criteria: criteria.iter().map(|c| c.id.clone()).collect(),
            attempts: max_attempts,
        }),
        GoalExhaustedAction::ReturnLast => {
            let result = self.run_with_messages(Some(task)).await?;
            Ok(GoalOutcome::Exhausted { messages: result, attempts: max_attempts })
        }
    }
}
```

**`GoalOutcome` 区分两种成功**：`Passed` 表示满足全部 criteria，`Exhausted` 表示耗尽次数但策略允许返回。调用方可据此决定后续行为。

### 3.4 Context 策略

```rust
async fn apply_context_strategy(&mut self) -> Result<(), AgentError> {
    match &self.strategy.context {
        ContextStrategy::Accumulate => { /* 保留全部历史 */ }
        ContextStrategy::Compact { keep_last_n } => {
            if self.entries.len() > *keep_last_n {
                let split_at = self.entries.len() - *keep_last_n;
                let old = self.entries.drain(..split_at).collect::<Vec<_>>();
                let summary = self.compaction_actor.summarize(&old).await?;
                self.prompt_builder.upsert_fragment(PromptFragment {
                    id: "compaction-summary".into(),
                    kind: FragmentKind::RuntimeInjection,
                    source: FragmentSource::System,
                    content: format!("## Prior context (compacted)\n\n{summary}"),
                    priority: 150,
                });
            }
        }
        ContextStrategy::Clear => {
            self.entries.clear();
            self.prompt_builder = PromptBuilder::from_base(self.base_persona.clone());
            crate::skills::inject_skills_into_builder(&mut self.prompt_builder, &self.skills);
        }
    }
    Ok(())
}
```

`Compact::keep_last_n` 的单位是 **SessionEntry 数量**（含 Message 和 Compaction 两种 Entry 类型）。

### 3.5 Goal 验证 prompt

**首轮 prompt**（含结构化输出要求）：

```
## Task
{task}

## Acceptance Criteria
You must satisfy ALL of the following before responding:

1. [tests-pass] All tests pass — `cargo test` must exit 0
2. [no-unwrap] No unwrap() calls remain in production code

After completing the task, end your response with a criteria checklist:

[CRITERION_RESULT: tests-pass: PASS|FAIL]
[CRITERION_RESULT: no-unwrap: PASS|FAIL]
```

**重试 prompt**（第二次及以后）：

```
## Acceptance Criteria Check (attempt 2/5)

The previous response did not meet all criteria:

✗ tests-pass — `cargo test` returned exit code 1
✓ no-unwrap

Please fix the failing criteria and respond with the corrected implementation.
End with [CRITERION_RESULT: ...] as before.
```

通过 `PromptBuilder` 以 `FragmentKind::RuntimeInjection` (priority 200) 注入。

**结构化输出**：agent 必须在响应末尾输出 `[CRITERION_RESULT: id: PASS|FAIL]` 标记，`SelfAssessment` 验证器解析这些标记而非自然语言来判断通过与否。

### 3.6 Goal 验证执行

```rust
async fn goal_satisfied(&self, result: &[AgentMessage], criteria: &[GoalCriterion])
    -> bool
{
    for c in criteria {
        let passed = match &c.verification {
            GoalVerification::SelfAssessment => {
                self.parse_criterion_result(result, &c.id).unwrap_or(false)
            }
            GoalVerification::Command { command } => {
                self.run_verification_command(command).await
            }
            GoalVerification::OutputContains { text } => {
                self.output_contains(result, text)
            }
        };
        if !passed { return false; }
    }
    true
}
```

---

## 4. 与现有 Hook 系统的关系

策略层在 hook 系统**之上**——控制"跑几次、多久跑一次、记多少"。每轮 run 内部仍然走完整 hook 链路：

```
SessionStrategy (新增)
  │
  └→ run_with_messages() (现有)
       │
       ├── on_before_agent_start
       ├── on_context
       ├── on_before_provider_request
       ├── on_after_provider_response
       ├── on_tool_call / on_tool_result
       └── on_turn_end / on_agent_end
```

Hook 不知道 strategy 的存在——每轮 run 对 hook 来说就是一个普通的 agent 执行。

---

## 5. API

```json
// POST /api/v1/sessions
{
  "title": "Deploy monitor",
  "system_prompt": "You monitor deployment status...",
  "strategy": {
    "termination": {
      "type": "goal",
      "criteria": [
        { "id": "deploy-success", "description": "Deployment status is 'healthy'",
          "verification": { "type": "command", "command": "curl -s deploy/status | grep healthy" } }
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

| Phase | 内容 | 预计 |
|-------|------|------|
| 1 | 定义 `SessionStrategy` + 三个子 enum + `GoalOutcome` | ~120 行 |
| 2 | `SessionActor` 加 `strategy` 字段 + 分派逻辑 + `apply_context_strategy` | ~150 行 |
| 3 | `run_goal_sync` + goal prompt 构建 + `goal_satisfied` | ~100 行 |
| 4 | `spawn_background_loop` + `AgentEvent::LoopIterationComplete/Error` | ~80 行 |
| 5 | API gateway types + `interval_ms` 转换 | ~50 行 |
| 6 | 测试 | ~100 行 |

---

## 7. 不做

| 不做 | 原因 |
|------|------|
| 跨 session 的 goal/loop 编排 | Tavern 职责 |
| 自定义 termination 条件（用户回调） | Phase 1 仅 built-in 的 Goal + max_iterations |
| `Compact` 策略使用外部记忆服务 | 当前 `CompactionActor` 已足够；未来可接入 `MemoryStore` |
