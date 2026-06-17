# Tavern Agent Team P0 类型实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `tavern-comp` 中定义 Agent Team 的核心类型：`Role`、`Team`、`Squad`、`TeamContext`、`Handoff`、`AgentExecutor`、`AgentInput`、`AgentOutput`、`Mission`。

**Architecture:** 新增 `crates/tavern-comp/src/team/` 模块收敛所有 Agent Team 类型；复用 `tavern-core` 的 `AgentConfig`、`ModelConfig` 以及 `tavern-comp::workflow` 的 `Process`、`WebhookConfig`、`SignalTimeoutAction`；保持与现有 `Workflow`/`Step` 的向后兼容，不删除旧类型。

**Tech Stack:** Rust, serde, chrono, async-trait, uuid, thiserror

---

## 文件结构

- **Create:** `crates/tavern-comp/src/team/mod.rs` — 导出所有 team 模块
- **Create:** `crates/tavern-comp/src/team/role.rs` — `Role`, `Visibility`, `SkillRef`
- **Create:** `crates/tavern-comp/src/team/definition.rs` — `Team`
- **Create:** `crates/tavern-comp/src/team/squad.rs` — `Squad`, `SquadStatus`
- **Create:** `crates/tavern-comp/src/team/context.rs` — `TeamContext`, `Message`, `MessageKind`, `VisibilityRules`
- **Create:** `crates/tavern-comp/src/team/handoff.rs` — `Handoff`, `AttachmentRef`, `AttachmentScope`, `HandoffMode`
- **Create:** `crates/tavern-comp/src/team/mission.rs` — `Mission`
- **Create:** `crates/tavern-comp/src/team/executor.rs` — `AgentExecutor` trait, `AgentInput`, `AgentOutput`, `AgentExecutorError`
- **Modify:** `crates/tavern-comp/src/lib.rs` — 导出新增模块
- **Modify:** `crates/tavern-comp/src/error.rs` — 添加 `AgentExecutorError` 类型

每个模块的单元测试放在文件内 `#[cfg(test)] mod tests` 中，与现有代码风格一致。

---

## Task 1: Role 类型

**Files:**
- Create: `crates/tavern-comp/src/team/role.rs`
- Modify: `crates/tavern-comp/src/team/mod.rs`

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_defaults() {
        let role = Role {
            id: "researcher".into(),
            name: "研究员".into(),
            description: None,
            agent_id: "base_researcher".into(),
            team_instructions: None,
            model_override: None,
            visibility: Visibility::default(),
            skills: vec![],
        };
        assert!(role.visibility.read_shared);
        assert!(role.visibility.read_private_roles.is_empty());
    }
}
```

- [ ] **Step 2: Implement `Role`, `Visibility`, and `SkillRef`**

```rust
use serde::{Deserialize, Serialize};
use tavern_core::{ModelConfig, SkillConfig};

/// Agent Team 内对 skill 的引用。P0 直接复用 `SkillConfig`。
pub type SkillRef = SkillConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub agent_id: String,
    #[serde(default)]
    pub team_instructions: Option<String>,
    #[serde(default)]
    pub model_override: Option<ModelConfig>,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default)]
    pub skills: Vec<SkillRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Visibility {
    #[serde(default = "default_true")]
    pub read_shared: bool,
    #[serde(default)]
    pub read_private_roles: Vec<String>,
}

impl Default for Visibility {
    fn default() -> Self {
        Self {
            read_shared: true,
            read_private_roles: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p tavern-comp team::role::tests
```

Expected: PASS

---

## Task 2: Team 类型

**Files:**
- Create: `crates/tavern-comp/src/team/definition.rs`
- Modify: `crates/tavern-comp/src/team/mod.rs`

- [ ] **Step 1: Implement `Team`**

```rust
use serde::{Deserialize, Serialize};
use tavern_core::PlanningConfig;
use crate::workflow::{Process, WebhookConfig};
use super::role::Role;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub roles: Vec<Role>,
    #[serde(default)]
    pub default_process: Process,
    #[serde(default)]
    pub planning: Option<PlanningConfig>,
    #[serde(default)]
    pub webhook: Option<WebhookConfig>,
}
```

- [ ] **Step 2: Add unit test for YAML deserialize**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_yaml_deserialize() {
        let yaml = r#"
id: content_team
name: 内容生产小组
roles:
  - id: researcher
    name: 研究员
    agent_id: base_researcher
"#;
        let team: Team = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(team.id, "content_team");
        assert_eq!(team.roles.len(), 1);
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p tavern-comp team::team::tests
```

Expected: PASS

---

## Task 3: Squad 类型

**Files:**
- Create: `crates/tavern-comp/src/team/squad.rs`
- Modify: `crates/tavern-comp/src/team/mod.rs`

- [ ] **Step 1: Implement `SquadStatus` and `Squad`**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::context::TeamContext;
use super::executor::AgentExecutor;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SquadStatus {
    Pending,
    Running,
    WaitingForSignal { signal: String },
    Sleeping { wake_at: DateTime<Utc> },
    Completed,
    Failed,
}

#[derive(Clone)]
pub struct Squad {
    pub id: String,
    pub team_id: String,
    pub status: SquadStatus,
    pub context: TeamContext,
    pub executor: Arc<dyn AgentExecutor>,
}
```

Note: `Squad` intentionally derives only `Clone`, not `Serialize`, because `Arc<dyn AgentExecutor>` is not serializable. State recovery uses `TeamContext` + `SquadStatus`.

---

## Task 4: TeamContext 类型

**Files:**
- Create: `crates/tavern-comp/src/team/context.rs`
- Modify: `crates/tavern-comp/src/team/mod.rs`

- [ ] **Step 1: Implement `MessageKind`, `Message`, `VisibilityRules`, `TeamContext`**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageKind {
    Invocation,
    Output,
    Handoff,
    Observation,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub turn: u32,
    pub kind: MessageKind,
    pub content: Value,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VisibilityRules {
    /// role id -> list of other role private spaces it can read
    #[serde(default)]
    pub role_can_read: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TeamContext {
    #[serde(default)]
    pub shared: Value,
    #[serde(default)]
    pub private: HashMap<String, Value>,
    #[serde(default)]
    pub thread: Vec<Message>,
    #[serde(default)]
    pub visibility: VisibilityRules,
}
```

- [ ] **Step 2: Add helper methods**

```rust
impl TeamContext {
    pub fn can_read_private(&self, reader_role: &str, owner_role: &str) -> bool {
        reader_role == owner_role
            || self
                .visibility
                .role_can_read
                .get(reader_role)
                .map(|v| v.contains(&owner_role.to_string()))
                .unwrap_or(false)
    }

    /// P0: minimal resolution. Full priority (shared -> own private -> authorized private ->
    /// _last_message) will be implemented in template rendering integration.
    pub fn resolve(&self, role: &str, key: &str) -> Option<&Value> {
        self.shared.get(key).or_else(|| {
            if self.can_read_private(role, role) {
                self.private.get(role).and_then(|v| v.get(key))
            } else {
                None
            }
        })
    }
}
```

- [ ] **Step 3: Add tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_private_visibility() {
        let mut ctx = TeamContext::default();
        ctx.private.insert("a".into(), serde_json::json!({"x": 1}));
        assert!(ctx.can_read_private("a", "a"));
        assert!(!ctx.can_read_private("b", "a"));
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p tavern-comp team::context::tests
```

Expected: PASS

---

## Task 5: Handoff 类型

**Files:**
- Create: `crates/tavern-comp/src/team/handoff.rs`
- Modify: `crates/tavern-comp/src/team/mod.rs`

- [ ] **Step 1: Implement types**

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HandoffMode {
    Inherit,
    Required,
    Auto,
}

impl Default for HandoffMode {
    fn default() -> Self {
        HandoffMode::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    pub summary: String,
    #[serde(default)]
    pub next_role: Option<String>,
    #[serde(default)]
    pub candidates: Vec<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub request_human: bool,
    #[serde(default)]
    pub terminate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub scope: AttachmentScope,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AttachmentScope {
    Shared,
    Private { role: String },
}
```

- [ ] **Step 2: Add `detect` helper**

```rust
impl Handoff {
    /// Heuristic: a Value is a Handoff if it is an object with a "summary" string field.
    pub fn detect(value: &Value) -> Option<Result<Handoff, serde_json::Error>> {
        if let Some(obj) = value.as_object() {
            if obj.get("summary").and_then(|v| v.as_str()).is_some() {
                return Some(serde_json::from_value(value.clone()));
            }
        }
        None
    }
}
```

- [ ] **Step 3: Add tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_detection() {
        let normal = serde_json::json!("hello");
        assert!(Handoff::detect(&normal).is_none());

        let handoff = serde_json::json!({
            "summary": "done",
            "terminate": true
        });
        assert!(Handoff::detect(&handoff).is_some());
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p tavern-comp team::handoff::tests
```

Expected: PASS

---

## Task 6: Mission 类型

**Files:**
- Create: `crates/tavern-comp/src/team/mission.rs`
- Modify: `crates/tavern-comp/src/team/mod.rs`

- [ ] **Step 1: Implement `Mission`**

```rust
use serde::{Deserialize, Serialize};
use crate::workflow::SignalTimeoutAction;
use super::handoff::HandoffMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mission {
    pub id: String,
    pub role: String,
    pub task: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub or_depends_on: Vec<String>,
    #[serde(default)]
    pub output_key: Option<String>,
    #[serde(default)]
    pub handoff_mode: HandoffMode,

    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub retries: Option<u64>,
    #[serde(default)]
    pub retry_delay: Option<u64>,
    #[serde(default)]
    pub wait_for_signal: Option<String>,
    #[serde(default)]
    pub signal_timeout: Option<u64>,
    #[serde(default)]
    pub signal_timeout_action: Option<SignalTimeoutAction>,
    #[serde(default)]
    pub breakpoint: bool,
}
```

---

## Task 7: AgentExecutor trait

**Files:**
- Create: `crates/tavern-comp/src/team/executor.rs`
- Modify: `crates/tavern-comp/src/team/mod.rs`
- Modify: `crates/tavern-comp/src/error.rs`

- [ ] **Step 1: Define `AgentExecutorError`**

Add to `crates/tavern-comp/src/error.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentExecutorError {
    #[error("role not found: {id}")]
    RoleNotFound { id: String },

    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    #[error("timeout")]
    Timeout,
}
```

- [ ] **Step 2: Implement trait and input/output types**

```rust
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tavern_core::ModelConfig;

use super::context::TeamContext;
use super::role::Role;
use crate::error::AgentExecutorError;

pub struct AgentInput {
    pub task: String,
    pub context: TeamContext,
    pub model_override: Option<ModelConfig>,
    pub timeout: Option<Duration>,
}

pub struct AgentOutput {
    pub content: Value,
    pub usage: Option<Value>,
    pub latency: Duration,
    pub metadata: HashMap<String, Value>,
}

pub struct AgentOutputChunk {
    pub content: Value,
    pub usage: Option<Value>,
}

#[async_trait]
pub trait AgentExecutor: Send + Sync {
    async fn resolve_role(&self, role_id: &str) -> Result<Role, AgentExecutorError>;

    async fn execute(
        &self,
        role_id: &str,
        input: AgentInput,
    ) -> Result<AgentOutput, AgentExecutorError>;

    async fn execute_stream(
        &self,
        role_id: &str,
        input: AgentInput,
    ) -> Result<BoxStream<'static, AgentOutputChunk>, AgentExecutorError>;
}
```

Note: `usage` uses `Value` for now to avoid coupling to ai-provider Usage type. Can be refined later.

---

## Task 8: Module exports

**Files:**
- Modify: `crates/tavern-comp/src/team/mod.rs`
- Modify: `crates/tavern-comp/src/lib.rs`

- [ ] **Step 1: `team/mod.rs`**

```rust
pub mod context;
pub mod executor;
pub mod handoff;
pub mod mission;
pub mod role;
pub mod squad;
pub mod definition;
pub mod executor;
pub mod handoff;
pub mod mission;
pub mod role;
pub mod squad;

pub use context::{Message, MessageKind, TeamContext, VisibilityRules};
pub use definition::Team;
pub use executor::{AgentExecutor, AgentExecutorError, AgentInput, AgentOutput, AgentOutputChunk};
pub use handoff::{AttachmentRef, AttachmentScope, Handoff, HandoffMode};
pub use mission::Mission;
pub use role::{Role, SkillRef, Visibility};
pub use squad::{Squad, SquadStatus};
```

- [ ] **Step 2: `lib.rs`**

Add:

```rust
pub mod team;
```

And re-export key types:

```rust
pub use team::{
    AgentExecutor, AgentExecutorError, AgentInput, AgentOutput, AgentOutputChunk,
    AttachmentRef, AttachmentScope, Handoff, HandoffMode, Message, MessageKind, Mission,
    Role, SkillRef, Squad, SquadStatus, Team, TeamContext, Visibility, VisibilityRules,
};
```

---

## Task 9: Final verification

- [ ] **Step 1: Run all tavern-comp tests**

```bash
cargo test -p tavern-comp --lib
```

Expected: all tests pass, including new ones.

- [ ] **Step 2: Check clippy**

```bash
cargo clippy -p tavern-comp --all-targets
```

Expected: no warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/tavern-comp/src/team crates/tavern-comp/src/lib.rs crates/tavern-comp/src/error.rs
git commit -m "feat(tavern): add Agent Team core types (Team, Squad, Role, Context, Handoff, AgentExecutor)"
```

---

## Notes for implementer

- Keep types serializable where possible. `Squad` itself is not serializable due to `Arc<dyn AgentExecutor>`; persistence works on `TeamContext` + `SquadStatus`.
- Do not delete or rename existing `Workflow`/`Step`/`Instance` types in this P0.
- `SkillConfig` is already re-exported from `tavern-core`.
- `futures_util` is already in `tavern-comp/Cargo.toml`; use `futures_util::stream::BoxStream`.
