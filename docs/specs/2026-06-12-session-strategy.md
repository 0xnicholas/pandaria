# SessionStrategy — 三维执行策略

> Status: Draft  
> Date: 2026-06-12  
> Target: agent-core v0.3+

---

## 1. 设计

一个 session 的执行行为由三个正交维度决定：

```
                ┌─ Once         → 跑一次就停（默认）
termination ────┤
  (何时停)       └─ Goal         → 验证验收标准，不满足则继续

                ┌─ Once         → 立刻执行（默认）
rhythm ─────────┤
  (怎么跑)       └─ Loop         → 按间隔反复执行

                ┌─ Accumulate   → 保留全部历史（默认）
context ────────┤
  (记什么)       ├─ Compact      → 自动压缩旧上下文
                └─ Clear        → 每次 run 后清空历史
```

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
#[derive(Debug, Clone)]
pub enum RhythmStrategy {
    /// 默认：立即执行一次。
    Once,

    /// 按固定间隔反复执行，直到终止条件满足或达到上限。
    Loop {
        /// 两次 run 之间的等待时间。
        interval: std::time::Duration,
        /// 最大迭代次数。
        /// None = 无限（直到 termination 条件触发或手动 abort）。
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
                if !self.abort_token.is_cancelled() {
                    tokio::select! {
                        _ = tokio::time::sleep(*interval) => {}
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

### 3.1 Goal 验证 prompt 注入

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

### 3.2 Goal 验证执行

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
      "max_iterations": 20
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
