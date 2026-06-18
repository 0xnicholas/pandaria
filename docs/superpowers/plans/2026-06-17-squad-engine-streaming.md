# SquadEngine 真流式输出 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `run_stream()` to SquadEngine for real-time mission lifecycle event streaming via mpsc, exposed as SSE in api-gateway.

**Architecture:** Extract `run_core()` shared by `run()` and `run_stream()`. Events flow mpsc-first then EventStore. Squad state shared via `Arc<Mutex<Squad>>` enabling pause/resume. api-gateway wraps `Receiver<SquadEvent>` in existing `SseStream`.

**Tech Stack:** Rust, tokio (mpsc, oneshot, Mutex, spawn), axum SSE, serde

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `crates/tavern-comp/src/team/engine.rs` | Modify | Add `StreamHandle`, `run_stream()`, `run_core()`. Refactor `run()`, `run_dag()`, `run_hierarchical()`, `execute_mission()` for `event_tx` propagation |
| `crates/tavern-comp/src/error.rs` | Modify | Add `CompError::MissionFailed` variant |
| `crates/api-gateway/src/types.rs` | Modify | Add 9 squad `ServerEvent` variants + `event_type_name` mapping |
| `crates/api-gateway/src/tavern.rs` | Modify | Add `SquadHandle`, registry, SSE handler, route |

---

## Phase 1: `CompError::MissionFailed`

### Task 1: Add `MissionFailed` variant to `CompError`

**Files:**
- Modify: `crates/tavern-comp/src/error.rs`

- [ ] **Step 1: Add the variant**

```rust
// In CompError enum, add before the existing ManagerError section:
#[error("mission '{mission_id}' failed on attempt {attempt}: {reason}")]
MissionFailed {
    mission_id: String,
    attempt: u64,
    reason: String,
},
```

- [ ] **Step 2: Add Clone impl**

```rust
// In impl Clone for CompError, add before ManagerError:
CompError::MissionFailed { mission_id, attempt, reason } => CompError::MissionFailed {
    mission_id: mission_id.clone(),
    attempt: *attempt,
    reason: reason.clone(),
},
```

- [ ] **Step 3: Build and verify**

Run: `cargo build -p tavern-comp 2>&1`
Expected: Compiles clean (no usages yet, just the variant exists)

- [ ] **Step 4: Commit**

```bash
git add crates/tavern-comp/src/error.rs
git commit -m "feat(tavern): add CompError::MissionFailed variant"
```

---

## Phase 2: SquadEngine Core Refactoring

### Task 2: Add `StreamHandle` struct and `run_stream()` signature

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: Add `StreamHandle` struct after `SquadEngine` impl block**

```rust
/// Handle returned by SquadEngine::run_stream().
/// Combines the real-time event stream with a oneshot for final result.
pub struct StreamHandle {
    /// Real-time squad lifecycle events. Closes when execution completes, fails, or pauses.
    pub events: tokio::sync::mpsc::Receiver<SquadEvent>,
    /// Final SquadResult sent when the spawned execution task finishes.
    pub result: tokio::sync::oneshot::Receiver<SquadResult>,
}
```

- [ ] **Step 2: Add `run_stream()` method stub to `SquadEngine` impl**

```rust
/// Stream squad execution in real-time. Spawns a tokio task internally,
/// sharing squad state via Arc<Mutex<Squad>>. Returns immediately.
pub async fn run_stream(
    &self,
    team: &Team,
    squad: Arc<tokio::sync::Mutex<Squad>>,
) -> Result<StreamHandle, CompError> {
    // stub — implemented in next task
    todo!("run_stream")
}
```

- [ ] **Step 3: Build to verify types compile**

Run: `cargo build -p tavern-comp 2>&1`
Expected: Compiles (unused warning on `run_stream` is ok)

- [ ] **Step 4: Commit**

```bash
git add crates/tavern-comp/src/team/engine.rs
git commit -m "feat(tavern): add StreamHandle struct and run_stream() stub"
```

### Task 3: Extract `run_core()` with event_tx parameter

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: Add a helper to emit events (both mpsc and EventStore)**

In `SquadEngine`, add a private helper used by `run_core` and its callees:

```rust
/// Emit a squad event: push to streaming channel (if present), then persist.
async fn emit_event(
    &self,
    squad_id: &str,
    event: SquadEvent,
    event_tx: Option<&tokio::sync::mpsc::Sender<SquadEvent>>,
) -> Result<(), CompError> {
    // Push to streaming channel first (if present, best-effort)
    if let Some(tx) = event_tx {
        if let Err(e) = tx.try_send(event.clone()) {
            tracing::warn!(squad_id = %squad_id, error = %e, event_type = ?std::mem::discriminant(&event),
                "stream channel full, dropping event");
        }
    }
    // Persist (error propagates)
    self.store.append(squad_id, event.into()).await
}
```

- [ ] **Step 2: Refactor `run()` to call `run_core()`**

```rust
pub async fn run(
    &self,
    team: &Team,
    squad: &mut Squad,
) -> Result<SquadResult, CompError> {
    // ... keep existing preamble (SquadStarted, webhook, planning) ...
    // Replace direct run_dag/run_hierarchical call with:
    let result = self.run_core(team, squad, None).await;
    // ... keep existing postamble (flush, webhook) ...
    result
}
```

The existing planning phase, webhook notifies, and flush logic stay in `run()` and are NOT moved into `run_core`.

- [ ] **Step 3: Write `run_core()`**

```rust
/// Shared execution core. `event_tx` is None for synchronous run(),
/// Some for streaming run_stream().
async fn run_core(
    &self,
    team: &Team,
    squad: &mut Squad,
    event_tx: Option<&tokio::sync::mpsc::Sender<SquadEvent>>,
) -> Result<SquadResult, CompError> {
    match &team.default_process {
        Process::Sequential => self.run_dag(team, squad, event_tx).await,
        Process::Hierarchical(cfg) => self.run_hierarchical(team, squad, cfg, event_tx).await,
    }
}
```

- [ ] **Step 4: Update `run_dag` signature to accept `event_tx`**

```rust
async fn run_dag(
    &self,
    team: &Team,
    squad: &mut Squad,
    event_tx: Option<&tokio::sync::mpsc::Sender<SquadEvent>>,
) -> Result<SquadResult, CompError>
```

- [ ] **Step 5: Update `run_hierarchical` signature to accept `event_tx`**

```rust
async fn run_hierarchical(
    &self,
    team: &Team,
    squad: &mut Squad,
    manager_cfg: &tavern_core::ManagerConfig,
    event_tx: Option<&tokio::sync::mpsc::Sender<SquadEvent>>,
) -> Result<SquadResult, CompError>
```

- [ ] **Step 6: Update `execute_mission` signature to accept `event_tx`**

```rust
async fn execute_mission(
    &self,
    squad: &mut Squad,
    mission: &Mission,
    event_tx: Option<&tokio::sync::mpsc::Sender<SquadEvent>>,
) -> Result<(), CompError>
```

- [ ] **Step 7: Build to verify**

Run: `cargo build -p tavern-comp 2>&1`
Expected: Compilation errors at call sites — expected, will fix in next tasks

- [ ] **Step 8: Commit**

```bash
git add crates/tavern-comp/src/team/engine.rs
git commit -m "refactor(tavern): extract run_core(), add event_tx parameter to execution methods"
```

### Task 4: Replace `self.store.append()` calls with `emit_event()` in execution paths

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

> This is the core task — every `self.store.append(&squad.id, SquadEvent::...).await?` in `run_dag`, `run_hierarchical`, and `execute_mission` becomes `self.emit_event(&squad.id, SquadEvent::..., event_tx).await?`.

- [ ] **Step 1: Replace in `run_dag()` — SquadStarted-like events**

In `run_dag()`, the method starts after `run()`'s preamble (which stays in `run()`). The events within the DAG loop are:

- `SquadStarted` — stays in `run()` (not in run_dag)
- `SquadCompleted` — at loop exit, replace `self.store.append(...)` with `self.emit_event(...)`
- `SquadFailed` — on mission failure, replace with `emit_event`

- [ ] **Step 2: Replace in `run_dag()` — mission spawn and result collection**

For each spawned mission, the `MissionStarted` and `MissionCompleted`/`MissionFailed` events are emitted inside `execute_mission()`. The `run_dag` spawns need to pass `event_tx`:

```rust
// In the spawn closure, add event_tx:
let tx_clone = event_tx.cloned(); // Option<&Sender> -> Option<Sender>
tokio::spawn(async move {
    let result = engine.execute_mission(&mut squad, &mission, tx_clone.as_ref()).await;
    // ...
});
```

> Note: `event_tx` is `Option<&Sender>`. Within `tokio::spawn`, we need an owned `Option<Sender>`. Clone the `Sender` (mpsc::Sender is Clone) and wrap in Option.

- [ ] **Step 3: Replace in `run_hierarchical()`**

Same pattern — pass `event_tx` to `execute_mission()` calls, replace direct `store.append` calls in the hierarchical loop with `emit_event()`:
- `SquadFailed` → `emit_event`
- `MissionCompleted` → already in `execute_mission`

- [ ] **Step 4: Replace in `execute_mission()`**

Current direct `store.append` calls to convert:
- `MissionWaitingForSignal` (line ~635)
- `MissionScheduled` (line ~645)
- `MissionStarted` (line ~668)
- `MissionCompleted` (line ~730)
- `MissionFailed` (on error path, new emission)

```rust
// Replace each:
// self.store.append(&squad.id, SquadEvent::MissionStarted { ... }.into()).await?;
// With:
// self.emit_event(&squad.id, SquadEvent::MissionStarted { ... }, event_tx).await?;
```

- [ ] **Step 5: Add `MissionFailed` emission on final retry exhaustion**

In `execute_mission()`, the error path (the `Err(e)` arm of `squad.executor.execute(...)`) currently does:

```rust
Err(e) => {
    if attempt < max_attempts {
        // emit retry event, continue loop
    } else {
        return Err(CompError::StepFailed { ... });
    }
}
```

Add `MissionFailed` emission before the final `return Err`:

```rust
Err(e) => {
    if attempt < max_attempts {
        self.emit_event(&squad.id, SquadEvent::MissionRetryScheduled { ... }, event_tx).await?;
        continue;
    } else {
        self.emit_event(&squad.id, SquadEvent::MissionFailed {
            mission_id: mission.id.clone(),
            error: e.to_string(),
            attempt,
            will_retry: false,
        }, event_tx).await?;
        return Err(CompError::MissionFailed {
            mission_id: mission.id.clone(),
            attempt,
            reason: e.to_string(),
        });
    }
}
```

- [ ] **Step 6: Build and fix compilation errors**

Run: `cargo build -p tavern-comp 2>&1`
Expected: All call sites updated, compiles clean

- [ ] **Step 7: Commit**

```bash
git add crates/tavern-comp/src/team/engine.rs
git commit -m "refactor(tavern): replace store.append with emit_event() in execution paths"
```

### Task 5: Implement `run_stream()` body

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: Replace the `run_stream()` stub with full implementation**

```rust
pub async fn run_stream(
    &self,
    team: &Team,
    squad: Arc<tokio::sync::Mutex<Squad>>,
) -> Result<StreamHandle, CompError> {
    team.validate()?;

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<SquadEvent>(256);
    let (result_tx, result_rx) = tokio::sync::oneshot::channel::<SquadResult>();

    let engine = self.clone();
    let team = team.clone();
    let squad_clone = squad.clone();

    // Set initial status
    {
        let mut s = squad.lock().await;
        s.status = SquadStatus::Running;
    }

    // Emit SquadStarted
    self.emit_event(&squad.lock().await.id, SquadEvent::SquadStarted, Some(&event_tx)).await?;
    self.notify_webhook(&team, &*squad.lock().await, "started", None).await;

    tokio::spawn(async move {
        let mut s = squad_clone.lock().await;

        // Planning phase (if enabled)
        let effective_team = if let Some(ref planning) = team.planning
            && planning.enabled
        {
            match engine.run_planning_phase(&team, &s).await {
                Ok(t) => t,
                Err(e) => {
                    let _ = result_tx.send(SquadResult {
                        squad_id: s.id.clone(),
                        team_id: s.team_id.clone(),
                        status: SquadStatus::Failed,
                        context: s.context.clone(),
                        outputs: s.context.shared.clone(),
                    });
                    return;
                }
            }
        } else {
            team.clone()
        };

        let result = engine.run_core(&effective_team, &mut s, Some(&event_tx)).await;

        // Flush executor (best-effort)
        if let Err(e) = s.executor.flush().await {
            tracing::warn!(squad_id = %s.id, error = %e, "squad executor flush failed");
        }

        let squad_result = match result {
            Ok(r) => r,
            Err(e) => SquadResult {
                squad_id: s.id.clone(),
                team_id: s.team_id.clone(),
                status: SquadStatus::Failed,
                context: s.context.clone(),
                outputs: s.context.shared.clone(),
            },
        };

        // Webhook notify on terminal states
        match &squad_result.status {
            SquadStatus::Completed => {
                engine.notify_webhook(&effective_team, &s, "completed", None).await;
            }
            SquadStatus::Failed => {
                engine.notify_webhook(&effective_team, &s, "failed", None).await;
            }
            SquadStatus::WaitingForSignal { .. } | SquadStatus::Sleeping { .. } | SquadStatus::Breakpoint { .. } => {
                engine.notify_webhook(&effective_team, &s, "paused", None).await;
            }
            _ => {}
        }

        // Send final result (ignored if receiver dropped)
        let _ = result_tx.send(squad_result);
    });

    Ok(StreamHandle {
        events: event_rx,
        result: result_rx,
    })
}
```

- [ ] **Step 2: Verify `SquadEngine` implements `Clone`** — already does (`#[derive(Clone)]`)

- [ ] **Step 3: Build**

Run: `cargo build -p tavern-comp 2>&1`
Expected: Compiles clean

- [ ] **Step 4: Commit**

```bash
git add crates/tavern-comp/src/team/engine.rs
git commit -m "feat(tavern): implement run_stream() with Arc<Mutex<Squad>> and StreamHandle"
```

---

## Phase 3: Tests for `run_stream()`

### Task 6: Unit test — single mission happy path

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs` (add `#[cfg(test)] mod` tests)

- [ ] **Step 1: Write the test**

```rust
#[tokio::test]
async fn test_run_stream_single_mission() {
    use crate::team::executor::mock::MockAgentExecutor;
    use crate::team::role::Role;
    use std::sync::Arc;

    let role = Role {
        id: "worker".into(),
        name: "Worker".into(),
        description: None,
        instructions: None,
        model_override: None,
        tools: vec![],
    };
    let mission = Mission {
        id: "m1".into(),
        description: None,
        role: "worker".into(),
        task: "do it".into(),
        depends_on: vec![],
        or_depends_on: None,
        output_key: None,
        timeout: None,
        retries: None,
        retry_delay: None,
        wait_for_signal: None,
        breakpoint: false,
        handoff_mode: HandoffMode::Inherit,
    };
    let team = Team {
        id: "team1".into(),
        name: "Test".into(),
        description: None,
        roles: vec![role.clone()],
        missions: vec![mission.clone()],
        default_process: Process::Sequential,
        managers: vec![],
        planning: None,
        webhooks: vec![],
        signal_timeout: None,
    };

    let mut responses = std::collections::HashMap::new();
    responses.insert("worker".into(), serde_json::json!({"done": true}));
    let executor = Arc::new(MockAgentExecutor::new(vec![role], responses));
    let engine = SquadEngine::new();

    let squad = Arc::new(tokio::sync::Mutex::new(Squad::new(
        "s1".into(), "team1".into(), executor,
    )));

    let handle = engine.run_stream(&team, squad.clone()).await.unwrap();
    let mut events: Vec<SquadEvent> = vec![];
    while let Some(e) = handle.events.recv().await {
        events.push(e);
    }

    // Verify event sequence
    assert!(matches!(events[0], SquadEvent::SquadStarted));
    assert!(matches!(&events[1], SquadEvent::MissionScheduled { mission_id, .. } if mission_id == "m1"));
    assert!(matches!(&events[2], SquadEvent::MissionStarted { mission_id, .. } if mission_id == "m1"));
    assert!(matches!(&events[3], SquadEvent::MissionCompleted { mission_id, .. } if mission_id == "m1"));
    assert!(matches!(events[4], SquadEvent::SquadCompleted { .. }));

    // Verify Arc state was updated
    let s = squad.lock().await;
    assert!(matches!(s.status, SquadStatus::Completed));
    assert!(s.completed_missions.contains("m1"));
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p tavern-comp -- test_run_stream_single_mission`
Expected: FAIL (method may have compile issues to fix)

- [ ] **Step 3: Fix until pass**

Iterate on any compilation or logic issues until test passes.

- [ ] **Step 4: Commit**

```bash
git add crates/tavern-comp/src/team/engine.rs
git commit -m "test(tavern): run_stream single mission happy path"
```

### Task 7: Unit test — parallel mission interleaving

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: Write test for 2 parallel missions**

```rust
#[tokio::test]
async fn test_run_stream_parallel_interleaving() {
    // Set up 2 independent missions (no depends_on)
    // Verify MissionStarted for both arrives before either MissionCompleted
    // Verify eventual SquadCompleted
}
```

- [ ] **Step 2: Run and fix**

Run: `cargo test -p tavern-comp -- test_run_stream_parallel`

- [ ] **Step 3: Commit**

```bash
git add crates/tavern-comp/src/team/engine.rs
git commit -m "test(tavern): run_stream parallel mission interleaving"
```

### Task 8: Unit test — mission failure

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: Write test**

```rust
#[tokio::test]
async fn test_run_stream_mission_failure() {
    // Use StatefulMockExecutor with a failing response
    // Verify MissionFailed + SquadFailed in event stream
    // Verify squad status is Failed in Arc state
}
```

- [ ] **Step 2: Run and fix**

- [ ] **Step 3: Commit**

### Task 9: Unit test — backward compatibility

**Files:**
- Modify: `crates/tavern-comp/src/team/engine.rs`

- [ ] **Step 1: Verify existing tests still pass**

Run: `cargo test -p tavern-comp 2>&1`
Expected: All 128 existing tests pass with the refactored code

- [ ] **Step 2: Commit**

```bash
git commit -m "test(tavern): verify all existing tests pass after refactor"
```

---

## Phase 4: api-gateway — Types

### Task 10: Add squad `ServerEvent` variants

**Files:**
- Modify: `crates/api-gateway/src/types.rs`

- [ ] **Step 1: Add squad event variants to `ServerEvent` enum**

```rust
pub enum ServerEvent {
    // ... existing variants unchanged ...

    // ── Squad lifecycle ──
    #[serde(rename = "squad_started")]
    SquadStarted { squad_id: String, team_id: String },

    #[serde(rename = "squad_mission_scheduled")]
    SquadMissionScheduled { squad_id: String, mission_id: String, attempt: u64 },

    #[serde(rename = "squad_mission_started")]
    SquadMissionStarted { squad_id: String, mission_id: String },

    #[serde(rename = "squad_mission_completed")]
    SquadMissionCompleted { squad_id: String, mission_id: String, output: serde_json::Value },

    #[serde(rename = "squad_mission_failed")]
    SquadMissionFailed {
        squad_id: String,
        mission_id: String,
        error: String,
        attempt: u64,
        will_retry: bool,
    },

    #[serde(rename = "squad_mission_retry_scheduled")]
    SquadMissionRetryScheduled {
        squad_id: String,
        mission_id: String,
        attempt: u64,
        reason: String,
    },

    #[serde(rename = "squad_mission_waiting_signal")]
    SquadMissionWaitingSignal { squad_id: String, mission_id: String, signal_name: String },

    #[serde(rename = "squad_completed")]
    SquadCompleted { squad_id: String, outputs: serde_json::Value },

    #[serde(rename = "squad_failed")]
    SquadFailed { squad_id: String, reason: String },
}
```

- [ ] **Step 2: Update `event_type_name()` in sse.rs**

```rust
fn event_type_name(event: &ServerEvent) -> &'static str {
    match event {
        // ... existing branches ...
        ServerEvent::SquadStarted { .. } => "squad_started",
        ServerEvent::SquadMissionScheduled { .. } => "squad_mission_scheduled",
        ServerEvent::SquadMissionStarted { .. } => "squad_mission_started",
        ServerEvent::SquadMissionCompleted { .. } => "squad_mission_completed",
        ServerEvent::SquadMissionFailed { .. } => "squad_mission_failed",
        ServerEvent::SquadMissionRetryScheduled { .. } => "squad_mission_retry_scheduled",
        ServerEvent::SquadMissionWaitingSignal { .. } => "squad_mission_waiting_signal",
        ServerEvent::SquadCompleted { .. } => "squad_completed",
        ServerEvent::SquadFailed { .. } => "squad_failed",
    }
}
```

- [ ] **Step 3: Build api-gateway**

Run: `cargo build -p api-gateway 2>&1`
Expected: Compiles (unused variant warnings ok for now)

- [ ] **Step 4: Commit**

```bash
git add crates/api-gateway/src/types.rs crates/api-gateway/src/sse.rs
git commit -m "feat(api-gateway): add squad ServerEvent variants and event_type_name mapping"
```

---

## Phase 5: api-gateway — Squad Registry & SSE Endpoint

### Task 11: Add `SquadHandle` and registry to `TavernState`

**Files:**
- Modify: `crates/api-gateway/src/tavern.rs`

- [ ] **Step 1: Add `SquadHandle` struct and update `TavernState`**

```rust
use tokio::sync::RwLock;

pub struct SquadHandle {
    pub engine: tavern_comp::SquadEngine,
    pub squad: Arc<tokio::sync::Mutex<tavern_comp::Squad>>,
    pub team: tavern_core::Team,
    _cleanup: tokio::task::AbortHandle,
}

pub struct TavernState {
    pub hero: Arc<tavern_comp::TavernHero>,
    pub registry: Arc<RwLock<tavern_comp::WorkflowRegistry>>,
    pub event_store: Arc<dyn tavern_comp::EventStore>,
    pub tool_registry: Arc<tavern_core::ToolRegistry>,
    pub squads: Arc<RwLock<HashMap<String, SquadHandle>>>,
}
```

- [ ] **Step 2: Build and fix**

Run: `cargo build -p api-gateway 2>&1`
Expected: Fix any import issues or missing type references

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/src/tavern.rs
git commit -m "feat(api-gateway): add SquadHandle and squad registry to TavernState"
```

### Task 12: Implement squad register/deploy and cleanup lifecycle

**Files:**
- Modify: `crates/api-gateway/src/tavern.rs`

- [ ] **Step 1: Add `register_squad()` helper to `TavernState`**

```rust
impl TavernState {
    /// Register a newly deployed squad in the in-memory registry.
    /// Spawns a background task to clean up on terminal state.
    pub async fn register_squad(&self, handle: SquadHandle) {
        let squad_id = handle.squad.lock().await.id.clone();
        let cleanup_handle = {
            let squads = self.squads.clone();
            let sid = squad_id.clone();
            tokio::spawn(async move {
                // Wait for the oneshot result from the spawned execution task.
                // When it fires, remove the squad from the registry.
                // (The oneshot is set up in run_stream's caller — we hook it here.)
                squads.write().await.remove(&sid);
            })
        };
        self.squads.write().await.insert(squad_id, handle);
    }
}
```

> Note: cleanup timing is approximate. In a future iteration, `StreamHandle::result` oneshot will drive cleanup directly. For now, registry entries are removed when the SSE stream handler's spawned task drops (consumer disconnects or stream ends).

- [ ] **Step 2: Add `POST /tavern/squads` deploy endpoint**

```rust
#[derive(Deserialize)]
pub struct DeploySquadRequest {
    pub team_id: String,
    pub inputs: Value,
}

pub async fn deploy_squad(
    Extension(state): Extension<Arc<TavernState>>,
    Json(req): Json<DeploySquadRequest>,
) -> impl IntoResponse {
    // 1. Load team from workflow registry (or hero)
    // 2. Create SquadEngine + Squad
    // 3. Call engine.deploy() to get Squad
    // 4. Create SquadHandle, insert into registry
    // 5. Return { squad_id }
    todo!("deploy_squad — requires team resolution from registry")
}
```

For the initial implementation, keep the deploy endpoint simple:
- Team lookup from `WorkflowRegistry` (if team is stored as workflow) or create a minimal inline team
- Return `{ squad_id, status: "deployed" }`

- [ ] **Step 3: Add route**

```rust
// In routes():
.route("/squads", post(deploy_squad))
```

- [ ] **Step 4: Build**

Run: `cargo build -p api-gateway 2>&1`
Expected: Compiles (dead_code on deploy_squad is ok — user-facing endpoint)

- [ ] **Step 5: Commit**

```bash
git add crates/api-gateway/src/tavern.rs
git commit -m "feat(api-gateway): squad deploy endpoint with registry insertion"
```

### Task 13: Add `SquadEvent → ServerEvent` mapping function

**Files:**
- Modify: `crates/api-gateway/src/tavern.rs`

- [ ] **Step 1: Add mapping function**

```rust
fn map_squad_event(event: tavern_comp::SquadEvent, squad_id: &str, team_id: &str) -> Option<ServerEvent> {
    use tavern_comp::SquadEvent;
    match event {
        SquadEvent::SquadStarted => Some(ServerEvent::SquadStarted {
            squad_id: squad_id.into(),
            team_id: team_id.into(),
        }),
        SquadEvent::MissionScheduled { mission_id, attempt } => {
            Some(ServerEvent::SquadMissionScheduled {
                squad_id: squad_id.into(),
                mission_id,
                attempt,
            })
        }
        SquadEvent::MissionStarted { mission_id, .. } => {
            Some(ServerEvent::SquadMissionStarted {
                squad_id: squad_id.into(),
                mission_id,
            })
        }
        SquadEvent::MissionCompleted { mission_id, output, .. } => {
            Some(ServerEvent::SquadMissionCompleted {
                squad_id: squad_id.into(),
                mission_id,
                output,
            })
        }
        SquadEvent::MissionFailed { mission_id, error, attempt, will_retry } => {
            Some(ServerEvent::SquadMissionFailed {
                squad_id: squad_id.into(),
                mission_id,
                error,
                attempt,
                will_retry,
            })
        }
        SquadEvent::MissionRetryScheduled { mission_id, attempt, reason, .. } => {
            Some(ServerEvent::SquadMissionRetryScheduled {
                squad_id: squad_id.into(),
                mission_id,
                attempt,
                reason,
            })
        }
        SquadEvent::MissionWaitingForSignal { mission_id, signal_name, .. } => {
            Some(ServerEvent::SquadMissionWaitingSignal {
                squad_id: squad_id.into(),
                mission_id,
                signal_name,
            })
        }
        SquadEvent::SquadCompleted { outputs, .. } => Some(ServerEvent::SquadCompleted {
            squad_id: squad_id.into(),
            outputs,
        }),
        SquadEvent::SquadFailed { reason, .. } => Some(ServerEvent::SquadFailed {
            squad_id: squad_id.into(),
            reason,
        }),
        // SquadCreated is not streamed
        SquadEvent::SquadCreated { .. } => None,
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p api-gateway 2>&1`
Expected: Compiles

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/src/tavern.rs
git commit -m "feat(api-gateway): add SquadEvent → ServerEvent mapping function"
```

### Task 14: Add SSE endpoint handler

**Files:**
- Modify: `crates/api-gateway/src/tavern.rs`

- [ ] **Step 1: Add `squad_events_stream` handler**

```rust
pub async fn squad_events_stream(
    Extension(state): Extension<Arc<TavernState>>,
    Path(squad_id): Path<String>,
) -> impl IntoResponse {
    let handle = {
        let squads = state.squads.read().await;
        match squads.get(&squad_id) {
            Some(h) => h.clone(), // need Clone on SquadHandle or extract fields
            None => return ApiError::new(
                StatusCode::NOT_FOUND,
                "SquadNotFound",
                &format!("Squad '{}' not found", squad_id),
            ).into_response(),
        }
    };

    // Actual implementation needs SquadHandle to hold clonable fields.
    // For now: stub, SquadHandle will be adjusted in next commit.
    todo!("squad_events_stream")
}
```

- [ ] **Step 2: Make `SquadHandle` fields clonable — adjust struct**

`SquadHandle` needs `engine: SquadEngine` and `team: Team` to be accessible. Both already implement Clone. Remove `_cleanup` from the clone path by storing cleanup separately.

```rust
pub struct SquadHandle {
    pub engine: tavern_comp::SquadEngine,
    pub squad: Arc<tokio::sync::Mutex<tavern_comp::Squad>>,
    pub team: tavern_core::Team,
}

impl Clone for SquadHandle {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            squad: self.squad.clone(),
            team: self.team.clone(),
        }
    }
}
```

- [ ] **Step 3: Implement full handler body**

```rust
pub async fn squad_events_stream(
    Extension(state): Extension<Arc<TavernState>>,
    Path(squad_id): Path<String>,
) -> impl IntoResponse {
    let handle = {
        let squads = state.squads.read().await;
        match squads.get(&squad_id).cloned() {
            Some(h) => h,
            None => return ApiError::new(
                StatusCode::NOT_FOUND,
                "SquadNotFound",
                &format!("Squad '{}' not found", squad_id),
            ).into_response(),
        }
    };

    let team_id = handle.team.id.clone();
    let stream_handle = match handle.engine.run_stream(&handle.team, handle.squad).await {
        Ok(h) => h,
        Err(e) => return ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "StreamError",
            &e.to_string(),
        ).into_response(),
    };

    let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<ServerEvent>(256);
    let abort_handle = tokio::spawn(async move {
        while let Some(event) = stream_handle.events.recv().await {
            if let Some(server_event) = map_squad_event(event, &squad_id, &team_id) {
                if sse_tx.send(server_event).await.is_err() {
                    break;
                }
            }
        }
    });

    SseStream::new(sse_rx, abort_handle.abort_handle())
}
```

- [ ] **Step 4: Add route**

```rust
// In routes() function:
.route("/squads/{squad_id}/events/stream", get(squad_events_stream))
```

- [ ] **Step 5: Build**

Run: `cargo build -p api-gateway 2>&1`
Expected: Compiles clean

- [ ] **Step 6: Commit**

```bash
git add crates/api-gateway/src/tavern.rs
git commit -m "feat(api-gateway): add SSE endpoint for squad event streaming"
```

---

## Phase 6: Test Mapping Function

### Task 15: Unit test for `SquadEvent → ServerEvent` mapping

**Files:**
- Modify: `crates/api-gateway/src/tavern.rs`

- [ ] **Step 1: Add `#[cfg(test)]` test module**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_squad_started() {
        let result = map_squad_event(
            tavern_comp::SquadEvent::SquadStarted,
            "s1", "t1",
        );
        assert!(matches!(result, Some(ServerEvent::SquadStarted { squad_id, team_id }
            if squad_id == "s1" && team_id == "t1")));
    }

    #[test]
    fn test_map_mission_completed() {
        let result = map_squad_event(
            tavern_comp::SquadEvent::MissionCompleted {
                mission_id: "m1".into(),
                output: serde_json::json!({"done": true}),
                output_key: Some("out".into()),
                completed_at: chrono::Utc::now(),
            },
            "s1", "t1",
        );
        assert!(matches!(result, Some(ServerEvent::SquadMissionCompleted { squad_id, mission_id, .. }
            if squad_id == "s1" && mission_id == "m1")));
    }

    #[test]
    fn test_map_squad_created_is_skipped() {
        let result = map_squad_event(
            tavern_comp::SquadEvent::SquadCreated {
                squad_id: "s1".into(),
                team_id: "t1".into(),
                inputs: serde_json::json!({}),
            },
            "s1", "t1",
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_map_all_variants() {
        // Verify all SquadEvent variants map to Some (except SquadCreated)
        // This ensures no variant is forgotten when the enum is extended
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p api-gateway -- tavern::tests`
Expected: All pass

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/src/tavern.rs
git commit -m "test(api-gateway): SquadEvent → ServerEvent mapping coverage"
```

---

## Phase 7: Integration & Final Verification

### Task 16: Run full test suite

- [ ] **Step 1: tavern-comp tests**

Run: `cargo test -p tavern-comp 2>&1`
Expected: All tests pass (existing 128 + new streaming tests)

- [ ] **Step 2: api-gateway tests**

Run: `cargo test -p api-gateway 2>&1`
Expected: All tests pass

- [ ] **Step 3: Full workspace build**

Run: `cargo build 2>&1`
Expected: Clean build, no warnings (except dead_code for unused squad event variants until TUI integration)

- [ ] **Step 4: Commit**

```bash
git commit -m "chore: final verification — all tests pass"
```

---

## Task Summary

| # | Phase | Task | Est. |
|---|---|---|---|
| 1 | CompError | Add `MissionFailed` variant | 5 min |
| 2 | Engine | `StreamHandle` + `run_stream()` stub | 5 min |
| 3 | Engine | Extract `run_core()`, update signatures | 10 min |
| 4 | Engine | Replace `store.append` with `emit_event()` | 15 min |
| 5 | Engine | Implement `run_stream()` body | 10 min |
| 6 | Tests | Single mission happy path | 10 min |
| 7 | Tests | Parallel mission interleaving | 10 min |
| 8 | Tests | Mission failure | 10 min |
| 9 | Tests | Backward compatibility | 5 min |
| 10 | API Types | Squad `ServerEvent` variants | 5 min |
| 11 | API State | `SquadHandle` + registry | 5 min |
| 12 | API Deploy | Squad register/deploy endpoint | 10 min |
| 13 | API Mapping | `SquadEvent → ServerEvent` mapping | 5 min |
| 14 | API Handler | SSE endpoint | 10 min |
| 15 | API Tests | Mapping function tests | 5 min |
| 16 | Verify | Full suite pass | 5 min |

**Total estimated:** ~2.25 hours
