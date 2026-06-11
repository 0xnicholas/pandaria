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

### 2.1 TerminationStrategy

```rust
#[derive(Debug, Clone)]
pub enum TerminationStrategy {
    /// 默认：agent 返回 stop 就结束。
    Once,

    /// 目标驱动：agent 自我验证是否满足验收标准。
    /// 每轮 run 结束后注入验证 prompt，直到通过或超限。
    Goal {
        /// 验收标准列表。
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
    /// Agent 自我评估是否满足。
    SelfAssessment,
    /// 运行命令，exit 0 = 通过。
    Command { command: String },
    /// 上一个 assistant message 包含指定文本。
    OutputContains { text: String },
}

#[derive(Debug, Clone)]
pub enum GoalExhaustedAction {
    /// 返回错误。
    Abort,
    /// 返回最后一次结果（即使未通过）。
    ReturnLast,
}
```

### 2.2 RhythmStrategy

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
    /// 会话保持活跃直到手动 abort 或终止条件满足。
    ///
    /// 当 `PANDARIA_DISABLE_CRON=1` 时，Loop 请求直接返回错误。
    Loop {
        /// 两次迭代之间的等待时间。省略时默认 10 分钟。
        interval: Option<std::time::Duration>,
        /// 最大迭代次数。None = 无限（直到终止条件触发或手动 abort）。
        max_iterations: Option<u32>,
    },
}
```

### 2.3 ContextStrategy

```rust
#[derive(Debug, Clone)]
pub enum ContextStrategy {
    /// 默认：保留全部会话历史。
    Accumulate,

    /// 每次 run 后自动压缩，保留最近 N 轮上下文。
    Compact {
        /// 保留最近多少条消息。
        keep_last_n: usize,
    },

    /// 每次 run 后清空历史，下一次 run 从空白上下文开始。
    Clear,
}
```

---

## 3. SessionActor 执行流

### 3.1 后台 Loop 模式

Loop 首次 prompt 调用立即返回（不阻塞调用方），后续迭代在后台 `tokio::spawn` 中运行：

```rust
async fn prompt(&mut self, task: String) -> Result<Vec<AgentMessage>> {
    match &self.strategy.rhythm {
        RhythmStrategy::Once => {
            // 同步执行（现有行为）
            self.run_once(task).await
        }
        RhythmStrategy::Loop { interval, max_iterations } => {
            // 检查全局开关
            if std::env::var("PANDARIA_DISABLE_CRON").as_deref() == Ok("1") {
                return Err(AgentError::LoopDisabled);
            }

            let delay = interval.unwrap_or(DEFAULT_LOOP_INTERVAL);

            // 先跑第一轮（同步，让调用方拿到首轮结果）
            let first_result = self.run_with_fresh_context(task.clone()).await?;

            // 后续迭代在后台运行
            let abort = self.abort_token.clone();
            let strategy = self.strategy.clone();
            let store = self.store.clone();
            // ... clone what the background task needs

            tokio::spawn(async move {
                self.run_loop_background(task, delay, max_iterations, abort).await;
            });

            Ok(first_result)
        }
    }
}
```

### 3.2 独立上下文

Loop 的每次迭代创建全新的消息上下文，不累积历史：

```rust
async fn run_with_fresh_context(&mut self, task: String) -> Result<Vec<AgentMessage>> {
    // 保存旧上下文（如果需要 compact summary）
    let summary = if matches!(self.strategy.context, ContextStrategy::Compact { .. }) {
        Some(self.compact_to_summary().await?)
    } else {
        None
    };

    // 清空消息历史
    self.entries.clear();

    // 重建 PromptBuilder
    let mut builder = PromptBuilder::from_base(self.base_persona.clone());
    if let Some(summary) = summary {
        builder.upsert_fragment(PromptFragment {
            id: "previous-iteration-summary".into(),
            kind: FragmentKind::RuntimeInjection,
            source: FragmentSource::System,
            content: format!("## Summary of previous iteration\n\n{summary}"),
            priority: 100,
        });
    }
    crate::skills::inject_skills_into_builder(&mut builder, &self.skills);
    self.prompt_builder = builder;

    // 跑一轮
    self.run_with_messages(Some(task)).await
}
```

### 3.3 Goal 验证流

Goal 模式仍然是同步的——每轮执行完立刻评估，不满足就继续：

```rust
async fn run_with_strategy(&mut self, task: Option<String>) -> Result<Vec<AgentMessage>> {
    let mut iteration: u32 = 0;

    loop {
        // ── context preparation ──
        match self.strategy.context {
            ContextStrategy::Accumulate => { /* 什么都不做 */ }
            ContextStrategy::Compact { keep_last_n } => {
                self.compact_to_last_n(keep_last_n).await?;
            }
            ContextStrategy::Clear => {
                self.entries.clear();
                self.prompt_builder = PromptBuilder::from_base(self.base_persona.clone());
                crate::skills::inject_skills_into_builder(&mut self.prompt_builder, &self.skills);
            }
        }

        // ── run ──
        let task_prompt = if iteration > 0
            && matches!(self.strategy.termination, TerminationStrategy::Goal { .. })
        {
            self.build_verification_prompt(task.as_deref())
        } else {
            task.clone()
        };

        let result = self.run_with_messages(task_prompt).await?;
        iteration += 1;

        // ── termination check ──
        match &self.strategy.termination {
            TerminationStrategy::Once => return Ok(result),
            TerminationStrategy::Goal { criteria, max_attempts, on_exhausted } => {
                if self.goal_satisfied(&result, criteria).await {
                    return Ok(result);
                }
                if iteration >= *max_attempts {
                    return match on_exhausted {
                        GoalExhaustedAction::Abort => Err(AgentError::GoalNotMet { .. }),
                        GoalExhaustedAction::ReturnLast => Ok(result),
                    };
                }
                // 继续下一轮
            }
        }

        // ── rhythm ──
        match &self.strategy.rhythm {
            RhythmStrategy::Once => {
                // termination 没停但 rhythm 是 Once → 不应该发生
                // (loop 中 termination=Once 时首轮就返回了)
                return Ok(result);
            }
            RhythmStrategy::Loop { interval, max_iterations } => {
                if let Some(max) = max_iterations {
                    if iteration >= *max {
                        return Ok(result);
                    }
                }
                let delay = interval.unwrap_or(DEFAULT_LOOP_INTERVAL);
                if !self.abort_token.is_cancelled() {
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = self.abort_token.cancelled() => {
                            return Err(AgentError::Cancelled);
                        }
                    }
                }
            }
        }
    }
}
```

### 3.4 Goal 验证 prompt 注入

第二轮及以后，在原始 task 之前注入结构化反馈：

```
## Acceptance Criteria Check (attempt 2/5)

The previous response did not meet all criteria:

✗ criterion-1: All tests pass — `cargo test` returned exit code 1
✓ criterion-2: No unwrap() in production code
✗ criterion-3: Documentation added for new public API

Please address the failing criteria and respond with the corrected implementation.
```

通过 `PromptBuilder` 以 `FragmentKind::RuntimeInjection` (priority 200) 注入。

### 3.5 Goal 验证执行

```rust
async fn goal_satisfied(&self, result: &[AgentMessage], criteria: &[GoalCriterion])
    -> bool
{
    for c in criteria {
        match &c.verification {
            GoalVerification::SelfAssessment => {
                // 最后一轮 already included self-assessment in prompt
                // Check if assistant response explicitly confirms passing
                if !self.assistant_confirms_pass(result, &c.id) {
                    return false;
                }
            }
            GoalVerification::Command { command } => {
                if !self.run_verification_command(command).await {
                    return false;
                }
            }
            GoalVerification::OutputContains { text } => {
                if !self.output_contains(result, text) {
                    return false;
                }
            }
        }
    }
    true
}
```

---

## 4. 与现有 Hook 系统的关系

所有三个维度都在 hook 系统**之外**——它们控制的是 session 级别的宏观执行策略，不拦截单个 tool call 或 turn：

```
SessionStrategy (新增)
  │
  │  控制"跑几次、多久跑一次、记多少"
  │
  └→ run_with_messages() (现有)
       │
       │  内部仍然走完整 hook 链路：
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
| 1 | 定义 `SessionStrategy` + 三个子 enum | ~100 行 |
| 2 | `SessionActor` 加 `strategy` 字段，拆出 `run_once` / `run_with_strategy`；ContextStrategy 的 Compact/Clear 逻辑 | ~120 行 |
| 3 | Goal 验证 prompt 构建 + `goal_satisfied` 实现 | ~80 行 |
| 4 | API gateway types + session route | ~40 行 |
| 5 | 测试 | ~80 行 |

---

## 7. 不做

| 不做 | 原因 |
|------|------|
| ContextStrategy 的跨 session 记忆共享 | 那是 `MemoryStore` trait 的职责，不在此 scope |
| Loop 的自定义 termination 条件（用户回调） | Phase 1 仅支持 built-in 的 Goal + max_iterations |
| Tavern 层的多 agent workflow（DAG / parallel / fan-out） | Tavern 职责 |
