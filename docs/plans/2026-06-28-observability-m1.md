# Observability M1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `observability` crate with `MetricsRegistry` (counter/gauge/histogram + Prometheus export), integrate into agent-core, tenant, and api-gateway for per-tenant metrics.

**Architecture:** New zero-dependency `observability` crate provides `MetricsRegistry` (dashmap-backed, `&self` methods). `Arc<MetricsRegistry>` injected as `Option` through existing config structs (SessionConfig → AgentLoopConfig, TenantManagerImpl). `/metrics` endpoint rewired to call `active_session_counts()` + `set_gauge()` + `export()`.

**Tech Stack:** Rust + tokio, dashmap 6 (already in workspace), axum (existing api-gateway).

**Spec:** `docs/specs/2026-06-28-observability-m1.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/observability/Cargo.toml` | Create | Crate manifest |
| `crates/observability/README.md` | Create | Crate doc |
| `crates/observability/src/lib.rs` | Create | Re-export `MetricsRegistry` |
| `crates/observability/src/registry.rs` | Create | `MetricsRegistry` impl (counter, gauge, histogram, export) |
| `crates/observability/src/layer.rs` | Create | Empty skeleton for future tracing Layer |
| `crates/agent-core/Cargo.toml` | Modify | Add `observability` path dep |
| `crates/agent-core/src/harness/agent_loop.rs` | Modify | Add `metrics` field to `AgentLoopConfig`; call `record_turn_metrics()` |
| `crates/agent-core/src/harness/tool.rs` | Modify | Add `metrics` param to `ToolExecutor::new()`; blocked/success/error/duration counters |
| `crates/agent-core/tests/metrics_integration.rs` | Create | Integration tests for agent-core metrics |
| `crates/tenant/Cargo.toml` | Modify | Add `observability` path dep |
| `crates/tenant/src/manager.rs` | Modify | Add `active_session_counts()` to trait + `TenantManagerImpl` metrics field + session counters |
| `crates/api-gateway/Cargo.toml` | Modify | Add `observability` path dep |
| `crates/api-gateway/src/server.rs` | Modify | Add `metrics_registry` to `AppState` |
| `crates/api-gateway/src/routes/metrics.rs` | Modify | Rewrite: gauge population + export + fallback |
| `crates/api-gateway/src/main.rs` | Modify | Create `MetricsRegistry`, inject into `TenantManagerImpl` and `AppState` |
| `crates/api-gateway/tests/e2e/e2e_metrics.rs` | Create | E2E tests for /metrics endpoint |
| `AGENTS.md` | Modify | Update observability crate status |

---

### Task 1: Create observability crate skeleton

**Files:**
- Create: `crates/observability/Cargo.toml`
- Create: `crates/observability/README.md`
- Create: `crates/observability/src/lib.rs`
- Create: `crates/observability/src/layer.rs`

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "observability"
version = "0.1.0"
edition = "2021"
description = "Lightweight embedded metrics registry with Prometheus export for Pandaria"

[dependencies]
dashmap = "6"
```

- [ ] **Step 2: Create README.md**

```bash
cat > crates/observability/README.md << 'EOF'
# observability

Lightweight embedded metrics registry for Pandaria.

## Public API

- `MetricsRegistry` — thread-safe counter, gauge, histogram registry
- `export()` — Prometheus exposition format

## Dependencies

Only `dashmap`. No external metrics libraries.

## Usage

```rust
let registry = Arc::new(MetricsRegistry::new());
registry.increment_counter("my_counter", &[("label", "val")], 1);
registry.set_gauge("my_gauge", &[("label", "val")], 42);
let prometheus_text = registry.export();
```
EOF
```

- [ ] **Step 3: Create lib.rs**

```rust
pub mod registry;
pub mod layer;

pub use registry::MetricsRegistry;
```

- [ ] **Step 4: Create layer.rs (empty skeleton)**

```rust
//! tracing-subscriber Layer for automatic span metrics.
//!
//! Reserved for M2. Currently an empty skeleton to avoid file structure
//! changes when Layer-based collection is added.
```

- [ ] **Step 5: Verify crate compiles**

```bash
cargo check -p observability
```

Expected: compiles with no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/observability/
git commit -m "feat(observability): create crate skeleton"
```

---

### Task 2: Implement MetricsRegistry with TDD

**Files:**
- Create: `crates/observability/src/registry.rs`

- [ ] **Step 1: Write failing unit tests (test module first)**

Create `registry.rs` with `#[cfg(test)] mod tests`:

```rust
//! Thread-safe metrics registry backed by dashmap.
//!
//! Supports counters (u64), gauges (i64), and histograms (f64 values
//! with pre-defined buckets).

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Pre-defined histogram buckets for tool call duration (seconds).
const DEFAULT_BUCKETS: &[f64] = &[0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0];

/// Internal metric value variants.
enum MetricValue {
    Counter(AtomicU64),
    Gauge(Mutex<i64>),
    Histogram(HistogramInner),
}

struct HistogramInner {
    buckets: Vec<f64>,       // upper bounds
    counts: Vec<AtomicU64>,  // per-bucket count
    sum: AtomicU64,          // sum encoded as f64 bits
}

/// Composite key for metric lookup: (name, label_string).
///
/// Labels are normalized as `key1=val1,key2=val2` for deterministic ordering.
#[derive(Hash, PartialEq, Eq)]
struct MetricKey {
    name: String,
    labels: String,  // sorted, comma-separated "key=val" pairs
}

impl MetricKey {
    fn new(name: &str, labels: &[(&str, &str)]) -> Self {
        let mut pairs: Vec<String> = labels
            .iter()
            .map(|(k, v)| format!("{}=\"{}\"", k, v))
            .collect();
        pairs.sort(); // deterministic ordering
        Self {
            name: name.to_string(),
            labels: pairs.join(","),
        }
    }
}

pub struct MetricsRegistry {
    metrics: DashMap<MetricKey, MetricValue>,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            metrics: DashMap::new(),
        }
    }

    pub fn increment_counter(&self, name: &str, labels: &[(&str, &str)], delta: u64) {
        let key = MetricKey::new(name, labels);
        if let Some(entry) = self.metrics.get(&key) {
            match entry.value() {
                MetricValue::Counter(c) => {
                    c.fetch_add(delta, Ordering::Relaxed);
                }
                _ => {} // type mismatch: silently no-op
            }
        } else {
            self.metrics
                .insert(key, MetricValue::Counter(AtomicU64::new(delta)));
        }
    }

    pub fn set_gauge(&self, name: &str, labels: &[(&str, &str)], value: i64) {
        let key = MetricKey::new(name, labels);
        if let Some(entry) = self.metrics.get(&key) {
            match entry.value() {
                MetricValue::Gauge(g) => {
                    *g.lock().expect("gauge lock poisoned") = value;
                }
                _ => {} // type mismatch: silently no-op
            }
        } else {
            self.metrics
                .insert(key, MetricValue::Gauge(Mutex::new(value)));
        }
    }

    pub fn observe_duration(&self, name: &str, labels: &[(&str, &str)], seconds: f64) {
        let key = MetricKey::new(name, labels);
        use dashmap::mapref::entry::Entry;
        match self.metrics.entry(key) {
            Entry::Occupied(entry) => {
                if let MetricValue::Histogram(h) = entry.get() {
                    h.record(seconds);
                }
            }
            Entry::Vacant(entry) => {
                let h = HistogramInner::new(DEFAULT_BUCKETS);
                h.record(seconds);
                entry.insert(MetricValue::Histogram(h));
            }
        }
    }

    pub fn export(&self) -> String {
        // Group metrics by name for Prometheus HELP/TYPE lines
        let mut names: Vec<String> = self
            .metrics
            .iter()
            .map(|entry| entry.key().name.clone())
            .collect();
        names.sort();
        names.dedup();

        let mut output = String::new();

        for name in &names {
            // Collect all entries for this metric name
            let entries: Vec<_> = self
                .metrics
                .iter()
                .filter(|e| e.key().name == *name)
                .collect();

            // Determine type from first entry
            let (help_line, type_line) = match entries.first().map(|e| e.value()) {
                Some(MetricValue::Counter(_)) => (
                    format!("# HELP {} Auto-generated counter\n", name),
                    format!("# TYPE {} counter\n", name),
                ),
                Some(MetricValue::Gauge(_)) => (
                    format!("# HELP {} Auto-generated gauge\n", name),
                    format!("# TYPE {} gauge\n", name),
                ),
                Some(MetricValue::Histogram(_)) => (
                    format!("# HELP {} Auto-generated histogram\n", name),
                    format!("# TYPE {} histogram\n", name),
                ),
                None => continue,
            };

            output.push_str(&help_line);
            output.push_str(&type_line);

            for entry in &entries {
                let labels_str = if entry.key().labels.is_empty() {
                    String::new()
                } else {
                    format!("{{{}}}", entry.key().labels)
                };

                match entry.value() {
                    MetricValue::Counter(c) => {
                        let val = c.load(Ordering::Relaxed);
                        output.push_str(&format!(
                            "{}{} {}\n",
                            name, labels_str, val
                        ));
                    }
                    MetricValue::Gauge(g) => {
                        let val = *g.lock().expect("gauge lock poisoned");
                        output.push_str(&format!(
                            "{}{} {}\n",
                            name, labels_str, val
                        ));
                    }
                    MetricValue::Histogram(h) => {
                        let sum = f64::from_bits(h.sum.load(Ordering::Relaxed));
                        let mut cumulative = 0u64;
                        for (i, bucket) in h.buckets.iter().enumerate() {
                            cumulative += h.counts[i].load(Ordering::Relaxed);
                            output.push_str(&format!(
                                "{}_bucket{} {{le=\"{}\"}} {}\n",
                                name, labels_str, bucket, cumulative
                            ));
                        }
                        // +Inf bucket
                        let total: u64 = h.counts.iter().map(|c| c.load(Ordering::Relaxed)).sum();
                        output.push_str(&format!(
                            "{}_bucket{} {{le=\"+Inf\"}} {}\n",
                            name, labels_str, total
                        ));
                        output.push_str(&format!(
                            "{}_count{} {}\n",
                            name, labels_str, total
                        ));
                        output.push_str(&format!(
                            "{}_sum{} {}\n",
                            name, labels_str, sum
                        ));
                    }
                }
            }
        }

        output
    }
}

impl HistogramInner {
    fn new(buckets: &[f64]) -> Self {
        Self {
            buckets: buckets.to_vec(),
            counts: buckets.iter().map(|_| AtomicU64::new(0)).collect(),
            sum: AtomicU64::new(0),
        }
    }

    fn record(&self, value: f64) {
        // Update sum (atomic f64 via to_bits/from_bits with fetch_update)
        let bits = value.to_bits();
        let _ = self.sum.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| Some(f64::from_bits(current).to_bits() + value.to_bits()),
        );
        // Actually, this is wrong for f64 atomics. Use a simple CAS loop.
        // For M1 simplicity, we use a Mutex-protected sum:
        // Let's just use the first bucket approach — the actual impl will
        // be refined during Task 2 implementation.

        // Find the right bucket
        for (i, upper) in self.buckets.iter().enumerate() {
            if value <= *upper {
                self.counts[i].fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // Falls in +Inf bucket — handled during export (total - sum of bucket counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_counter_increment() {
        let reg = MetricsRegistry::new();
        reg.increment_counter("test_total", &[("tenant", "acme")], 1);
        reg.increment_counter("test_total", &[("tenant", "acme")], 2);
        let output = reg.export();
        assert!(output.contains("test_total{tenant=\"acme\"} 3"));
    }

    #[test]
    fn test_counter_multi_label() {
        let reg = MetricsRegistry::new();
        reg.increment_counter("test_total", &[("tenant", "a"), ("status", "ok")], 5);
        reg.increment_counter("test_total", &[("tenant", "b"), ("status", "err")], 3);
        let output = reg.export();
        assert!(output.contains("test_total{status=\"ok\",tenant=\"a\"} 5"));
        assert!(output.contains("test_total{status=\"err\",tenant=\"b\"} 3"));
    }

    #[test]
    fn test_gauge_set() {
        let reg = MetricsRegistry::new();
        reg.set_gauge("test_gauge", &[("tenant", "acme")], 42);
        reg.set_gauge("test_gauge", &[("tenant", "acme")], 99);
        let output = reg.export();
        assert!(output.contains("test_gauge{tenant=\"acme\"} 99"));
        // Should not show 42 (overwritten)
        assert!(!output.contains("42"));
    }

    #[test]
    fn test_histogram_observe() {
        let reg = MetricsRegistry::new();
        reg.observe_duration("test_seconds", &[("tenant", "acme")], 0.3);
        reg.observe_duration("test_seconds", &[("tenant", "acme")], 2.0);
        let output = reg.export();
        assert!(output.contains("test_seconds_bucket{tenant=\"acme\",le=\"0.5\"} 1"));
        assert!(output.contains("test_seconds_bucket{tenant=\"acme\",le=\"5\"} 2"));
        assert!(output.contains("test_seconds_bucket{tenant=\"acme\",le=\"+Inf\"} 2"));
        assert!(output.contains("test_seconds_count{tenant=\"acme\"} 2"));
    }

    #[test]
    fn test_export_empty() {
        let reg = MetricsRegistry::new();
        let output = reg.export();
        assert!(output.is_empty());
    }

    #[test]
    fn test_export_format_has_type_lines() {
        let reg = MetricsRegistry::new();
        reg.increment_counter("my_counter", &[], 1);
        reg.set_gauge("my_gauge", &[], 5);
        let output = reg.export();
        assert!(output.contains("# HELP my_counter"));
        assert!(output.contains("# TYPE my_counter counter"));
        assert!(output.contains("# HELP my_gauge"));
        assert!(output.contains("# TYPE my_gauge gauge"));
    }

    #[test]
    fn test_concurrent_access() {
        let reg = Arc::new(MetricsRegistry::new());
        let mut handles = vec![];
        for _ in 0..10 {
            let reg = reg.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    reg.increment_counter("concurrent_total", &[], 1);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let output = reg.export();
        assert!(output.contains("concurrent_total 1000"));
    }

    #[test]
    fn test_counter_gauge_type_isolation() {
        // Calling set_gauge on a counter-backed metric should not affect the counter
        let reg = MetricsRegistry::new();
        reg.increment_counter("shared_name", &[], 10);
        reg.set_gauge("shared_name", &[], 99);
        let output = reg.export();
        // The first write determines the type; gauge call is silently no-op
        assert!(output.contains("shared_name 10"));
        assert!(output.contains("# TYPE shared_name counter"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail (metrics not implemented yet)**

```bash
cargo test -p observability 2>&1 | tail -20
```

Expected: compilation errors (no `MetricsRegistry` yet) — actually we wrote the tests and impl together. Let's verify they pass:

```bash
cargo test -p observability
```

Expected: all 8 tests pass.

- [ ] **Step 3: Review HistogramInner::record sum tracking**

The initial pseudocode uses a broken `fetch_update` for f64. Fix: track sum in a `Mutex<f64>` instead.

Replace `HistogramInner`:

```rust
struct HistogramInner {
    buckets: Vec<f64>,
    counts: Vec<AtomicU64>,
    sum: Mutex<f64>,
}

impl HistogramInner {
    fn new(buckets: &[f64]) -> Self {
        Self {
            buckets: buckets.to_vec(),
            counts: buckets.iter().map(|_| AtomicU64::new(0)).collect(),
            sum: Mutex::new(0.0),
        }
    }

    fn record(&self, value: f64) {
        // Update sum
        if let Ok(mut s) = self.sum.lock() {
            *s += value;
        }
        // Find bucket
        for (i, upper) in self.buckets.iter().enumerate() {
            if value <= *upper {
                self.counts[i].fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }
}
```

And update `export()` to read sum:

```rust
MetricValue::Histogram(h) => {
    let sum = *h.sum.lock().expect("histogram sum lock poisoned");
    // ... rest unchanged
}
```

- [ ] **Step 4: Run tests again after fix**

```bash
cargo test -p observability
```

Expected: all 8 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/observability/src/registry.rs
git commit -m "feat(observability): implement MetricsRegistry with counter, gauge, histogram, Prometheus export"
```

---

### Task 3: Add metrics to AgentLoopConfig and record turn metrics

**Files:**
- Modify: `crates/agent-core/Cargo.toml`
- Modify: `crates/agent-core/src/harness/agent_loop.rs`

- [ ] **Step 1: Add observability dependency to agent-core**

Open `crates/agent-core/Cargo.toml`. Under `[dependencies]`, add:

```toml
observability = { path = "../observability" }
```

- [ ] **Step 2: Verify dependency resolves**

```bash
cargo check -p agent-core 2>&1 | head -5
```

Expected: compiles (no new code uses `observability` yet).

- [ ] **Step 3: Add metrics field to AgentLoopConfig**

Open `crates/agent-core/src/harness/agent_loop.rs`. Add to the struct:

```rust
use std::sync::Arc;

pub struct AgentLoopConfig {
    // ... existing fields ...

    /// Optional metrics registry for per-tenant observability.
    /// When None, all metric recording is skipped (zero overhead).
    #[doc(hidden)]
    pub metrics: Option<Arc<observability::MetricsRegistry>>,
}
```

Add to `AgentLoopConfig::new()`:

```rust
impl AgentLoopConfig {
    pub fn new(/* ... */) -> Self {
        Self {
            // ... existing fields ...
            metrics: None,  // default: metrics disabled
        }
    }
}
```

- [ ] **Step 4: Add record_turn_metrics method**

Add a private method to `AgentLoop`:

```rust
impl AgentLoop {
    /// Record per-turn token consumption metrics.
    fn record_turn_metrics(&self, usage: &ai_provider::Usage) {
        if let Some(ref m) = self.config.metrics {
            let tid = &self.config.tenant_id;
            m.increment_counter(
                "pandaria_tokens_consumed_total",
                &[("tenant_id", tid), ("direction", "input")],
                usage.input_tokens,
            );
            m.increment_counter(
                "pandaria_tokens_consumed_total",
                &[("tenant_id", tid), ("direction", "output")],
                usage.output_tokens,
            );
        }
    }
}
```

- [ ] **Step 5: Call record_turn_metrics at turn end**

Locate the two places where `on_turn_end` hook is called in `AgentLoop::run()` (around lines 523 and 574 in the current agent_loop.rs). After the hook call (and after `Usage` is available), add:

```rust
// After on_turn_end hook, record metrics
if let Some(ref usage) = assistant_msg.usage {
    self.record_turn_metrics(usage);
}
```

- [ ] **Step 6: Verify compilation**

```bash
cargo check -p agent-core 2>&1 | tail -5
```

Expected: compiles without errors.

- [ ] **Step 7: Commit**

```bash
git add crates/agent-core/Cargo.toml crates/agent-core/src/harness/agent_loop.rs
git commit -m "feat(agent-core): add metrics field to AgentLoopConfig, record token consumption"
```

---

### Task 4: Add metrics to ToolExecutor

**Files:**
- Modify: `crates/agent-core/src/harness/tool.rs`

- [ ] **Step 1: Add metrics parameter to ToolExecutor::new()**

```rust
use std::sync::Arc;

pub(crate) struct ToolExecutor {
    tenant_id: String,
    session_id: String,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    tool: AgentToolRef,
    metrics: Option<Arc<observability::MetricsRegistry>>,  // NEW
}

impl ToolExecutor {
    pub(crate) fn new(
        tenant_id: String,
        session_id: String,
        hook_dispatcher: Arc<dyn HookDispatcher>,
        tool: AgentToolRef,
        metrics: Option<Arc<observability::MetricsRegistry>>,  // NEW
    ) -> Self {
        Self {
            tenant_id,
            session_id,
            hook_dispatcher,
            tool,
            metrics,  // NEW
        }
    }
}
```

- [ ] **Step 2: Add blocked counter in HookDecision::Block branch**

In `execute_tool_call()`, find the `match decision { HookDecision::Block { reason } => {` arm (around line 75). Before the `warn!` and `return Err(...)`, insert:

```rust
HookDecision::Block { reason } => {
    if let Some(ref m) = self.metrics {
        m.increment_counter(
            "pandaria_tool_calls_total",
            &[
                ("tenant_id", &self.tenant_id),
                ("tool", &tool_call.name),
                ("status", "blocked"),
            ],
            1,
        );
    }
    warn!(/* existing warn! */);
    // ... existing return Err(...) ...
}
```

- [ ] **Step 3: Add success/error counter and duration at end of execute_tool_call**

At the end of `execute_tool_call()`, before the final `result` return, add timing and metrics:

```rust
// At the top of execute_tool_call or before tool execution:
let start = std::time::Instant::now();

// ... existing execution pipeline ...

// At the end, just before returning result:
let elapsed = start.elapsed();
if let Some(ref m) = self.metrics {
    let tid = &self.tenant_id;
    let tool = &tool_call.name;
    let status = if result.is_ok() { "success" } else { "error" };
    m.increment_counter(
        "pandaria_tool_calls_total",
        &[("tenant_id", tid), ("tool", tool), ("status", status)],
        1,
    );
    m.observe_duration(
        "pandaria_tool_call_duration_seconds",
        &[("tenant_id", tid), ("tool", tool)],
        elapsed.as_secs_f64(),
    );
}
result
```

- [ ] **Step 4: Update all ToolExecutor::new() call sites**

In `tool.rs`, there are ~10 test call sites (around lines 211, 248, 313, 363, 415, 471, 506, 537, 586, 620). Each looks like:

```rust
ToolExecutor::new(
    tenant_id,
    session_id,
    hook_dispatcher,
    tool,
)
```

Change each to:

```rust
ToolExecutor::new(
    tenant_id,
    session_id,
    hook_dispatcher,
    tool,
    None,  // metrics disabled in tests
)
```

Find the production call site (likely in `harness/session/mod.rs` or `agent_loop.rs`). Update to pass `self.metrics.clone()`.

- [ ] **Step 5: Verify compilation**

```bash
cargo check -p agent-core 2>&1 | tail -5
```

Expected: compiles without errors. If test call sites are missed, compiler will flag them.

- [ ] **Step 6: Run agent-core tests**

```bash
cargo test -p agent-core 2>&1 | tail -10
```

Expected: all existing tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/agent-core/src/harness/tool.rs crates/agent-core/src/harness/session/mod.rs
git commit -m "feat(agent-core): add metrics to ToolExecutor (blocked/success/error/duration)"
```

---

### Task 5: Integration tests for agent-core metrics

**Files:**
- Create: `crates/agent-core/tests/metrics_integration.rs`

- [ ] **Step 1: Create the test file**

```rust
use std::sync::Arc;
use observability::MetricsRegistry;

#[test]
fn test_metrics_registry_creation() {
    let reg = MetricsRegistry::new();
    let output = reg.export();
    assert!(output.is_empty());
}

#[test]
fn test_metrics_token_counter_accumulates() {
    let reg = Arc::new(MetricsRegistry::new());

    // Simulate two turns of token usage
    reg.increment_counter(
        "pandaria_tokens_consumed_total",
        &[("tenant_id", "test"), ("direction", "input")],
        1500,
    );
    reg.increment_counter(
        "pandaria_tokens_consumed_total",
        &[("tenant_id", "test"), ("direction", "output")],
        500,
    );
    reg.increment_counter(
        "pandaria_tokens_consumed_total",
        &[("tenant_id", "test"), ("direction", "input")],
        800,
    );

    let output = reg.export();
    assert!(output.contains("pandaria_tokens_consumed_total{tenant_id=\"test\",direction=\"input\"} 2300"));
    assert!(output.contains("pandaria_tokens_consumed_total{tenant_id=\"test\",direction=\"output\"} 500"));
}

#[test]
fn test_metrics_tool_call_all_statuses() {
    let reg = Arc::new(MetricsRegistry::new());

    reg.increment_counter(
        "pandaria_tool_calls_total",
        &[("tenant_id", "t1"), ("tool", "read_file"), ("status", "success")],
        3,
    );
    reg.increment_counter(
        "pandaria_tool_calls_total",
        &[("tenant_id", "t1"), ("tool", "read_file"), ("status", "blocked")],
        1,
    );
    reg.increment_counter(
        "pandaria_tool_calls_total",
        &[("tenant_id", "t1"), ("tool", "read_file"), ("status", "error")],
        1,
    );

    let output = reg.export();
    assert!(output.contains("pandaria_tool_calls_total{status=\"success\",tenant_id=\"t1\",tool=\"read_file\"} 3"));
    assert!(output.contains("pandaria_tool_calls_total{status=\"blocked\",tenant_id=\"t1\",tool=\"read_file\"} 1"));
    assert!(output.contains("pandaria_tool_calls_total{status=\"error\",tenant_id=\"t1\",tool=\"read_file\"} 1"));
}

#[test]
fn test_metrics_session_lifecycle() {
    let reg = Arc::new(MetricsRegistry::new());

    // Created
    reg.increment_counter(
        "pandaria_sessions_total",
        &[("tenant_id", "acme"), ("status", "created")],
        5,
    );
    // Completed
    reg.increment_counter(
        "pandaria_sessions_total",
        &[("tenant_id", "acme"), ("status", "completed")],
        4,
    );
    // Failed
    reg.increment_counter(
        "pandaria_sessions_total",
        &[("tenant_id", "acme"), ("status", "failed")],
        1,
    );

    let output = reg.export();
    assert!(output.contains("pandaria_sessions_total{status=\"created\",tenant_id=\"acme\"} 5"));
    assert!(output.contains("pandaria_sessions_total{status=\"completed\",tenant_id=\"acme\"} 4"));
    assert!(output.contains("pandaria_sessions_total{status=\"failed\",tenant_id=\"acme\"} 1"));
}

#[test]
fn test_metrics_disabled_no_panic() {
    // Verify that increment_counter on a separate registry doesn't affect others
    let reg1 = Arc::new(MetricsRegistry::new());
    let reg2 = Arc::new(MetricsRegistry::new());

    reg1.increment_counter("counter", &[], 1);
    reg2.increment_counter("counter", &[], 2);

    let out1 = reg1.export();
    let out2 = reg2.export();
    assert!(out1.contains("counter 1"));
    assert!(out2.contains("counter 2"));
}

#[test]
fn test_metrics_export_valid_prometheus() {
    let reg = Arc::new(MetricsRegistry::new());
    reg.set_gauge("sessions_active", &[("tenant_id", "xyz")], 7);
    reg.increment_counter("requests_total", &[("status", "200")], 42);

    let output = reg.export();

    // Every metric must have HELP and TYPE lines
    assert!(output.contains("# HELP "));
    assert!(output.contains("# TYPE "));

    // No trailing whitespace issues
    for line in output.lines() {
        if !line.is_empty() && !line.starts_with('#') {
            // Metric lines: name{labels} value
            assert!(line.contains(' '), "line missing space: {}", line);
        }
    }
}
```

- [ ] **Step 2: Run integration tests**

```bash
cargo test -p agent-core --test metrics_integration
```

Expected: all 6 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/agent-core/tests/metrics_integration.rs
git commit -m "test(agent-core): add metrics integration tests"
```

---

### Task 6: Add active_session_counts() to TenantManager trait

**Files:**
- Modify: `crates/tenant/Cargo.toml`
- Modify: `crates/tenant/src/manager.rs`

- [ ] **Step 1: Add observability dependency to tenant**

```toml
# crates/tenant/Cargo.toml, under [dependencies]
observability = { path = "../observability" }
```

- [ ] **Step 2: Verify dependency resolves**

```bash
cargo check -p tenant 2>&1 | head -5
```

Expected: compiles.

- [ ] **Step 3: Add active_session_counts() to TenantManager trait**

Open `crates/tenant/src/manager.rs`. Find the `TenantManager` trait definition. Add the new method:

```rust
use std::collections::HashMap;

#[async_trait]
pub trait TenantManager: Send + Sync {
    // ... existing methods ...

    /// Returns per-tenant active session counts.
    ///
    /// Default implementation returns a single entry with key `"__total__"`
    /// delegating to `active_session_count()`. Implementations with per-tenant
    /// tracking (e.g., `TenantManagerImpl`) should override to return
    /// accurate per-tenant breakdowns.
    async fn active_session_counts(&self) -> Result<HashMap<String, usize>, TenantError> {
        let mut m = HashMap::new();
        m.insert("__total__".into(), self.active_session_count());
        Ok(m)
    }
}
```

Note: `active_session_count()` already exists on the trait. Check the trait definition for its exact signature. If it's sync, the default impl above is fine.

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p tenant 2>&1 | tail -5
```

Expected: compiles. If `active_session_count()` doesn't exist on the trait (only on impl), add it to the trait first.

- [ ] **Step 5: Commit**

```bash
git add crates/tenant/Cargo.toml crates/tenant/src/manager.rs
git commit -m "feat(tenant): add active_session_counts() to TenantManager trait"
```

---

### Task 7: Add metrics to TenantManagerImpl + session lifecycle counters

**Files:**
- Modify: `crates/tenant/src/manager.rs`

- [ ] **Step 1: Add metrics field to TenantManagerImpl**

Find `TenantManagerImpl` struct. Add:

```rust
pub struct TenantManagerImpl {
    // ... existing fields ...
    metrics: Option<Arc<observability::MetricsRegistry>>,
}
```

Update the constructor to accept and store it:

```rust
impl TenantManagerImpl {
    pub fn new(
        // ... existing params ...
        metrics: Option<Arc<observability::MetricsRegistry>>,
    ) -> Self {
        Self {
            // ... existing fields ...
            metrics,
        }
    }
}
```

- [ ] **Step 2: Override active_session_counts()**

```rust
impl TenantManager for TenantManagerImpl {
    async fn active_session_counts(&self) -> Result<HashMap<String, usize>, TenantError> {
        let counts = self.registry.active_session_counts(); // per-tenant from TenantRegistry
        Ok(counts)
    }
}
```

Check if `TenantRegistry` already has a method that returns per-tenant counts. If not, add it to `TenantRegistry`:

```rust
// crates/tenant/src/registry.rs
impl TenantRegistry {
    pub fn active_session_counts(&self) -> HashMap<String, usize> {
        self.tenants
            .iter()
            .map(|entry| {
                let tid = entry.key().clone();
                let count = entry.value().supervisor.active_session_count();
                (tid, count)
            })
            .collect()
    }
}
```

- [ ] **Step 3: Add session lifecycle counters**

In `create_session()`:

```rust
pub async fn create_session(&self, tenant_id: &str, params: CreateSessionParams)
    -> Result<SessionInfo, TenantError>
{
    // ... existing validation and creation logic ...
    // After successful creation:
    if let Some(ref m) = self.metrics {
        m.increment_counter(
            "pandaria_sessions_total",
            &[("tenant_id", tenant_id), ("status", "created")],
            1,
        );
    }
    // ...
}
```

In the session completion path (where `complete_session()` is handled):

```rust
if let Some(ref m) = self.metrics {
    m.increment_counter(
        "pandaria_sessions_total",
        &[("tenant_id", tenant_id), ("status", "completed")],
        1,
    );
}
```

In `delete_session()`:

```rust
if let Some(ref m) = self.metrics {
    m.increment_counter(
        "pandaria_sessions_total",
        &[("tenant_id", tenant_id), ("status", "failed")],
        1,
    );
}
```

In `cleanup_expired_sessions()` — note: this is on `SessionStore`, not `TenantManagerImpl`. Find where expired sessions are actually deleted and add:

```rust
if let Some(ref m) = self.metrics {
    for (tenant_id, count) in expired_counts_by_tenant {
        m.increment_counter(
            "pandaria_sessions_total",
            &[("tenant_id", &tenant_id), ("status", "expired")],
            count,
        );
    }
}
```

If per-tenant expired counts are not tracked in the cleanup path, record a single increment per cleanup with a sentinel or skip the per-tenant breakdown for expired status.

- [ ] **Step 4: Update TenantManagerImpl::new() call site**

Find where `TenantManagerImpl::new()` is called (likely in `api-gateway/src/main.rs` or a test helper). Add `None` for the metrics parameter for now (will be wired in Task 10).

- [ ] **Step 5: Verify compilation and tests**

```bash
cargo check -p tenant 2>&1 | tail -5
cargo test -p tenant 2>&1 | tail -10
```

Expected: compiles, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tenant/src/manager.rs crates/tenant/src/registry.rs
git commit -m "feat(tenant): add metrics to TenantManagerImpl with session lifecycle counters"
```

---

### Task 8: Add metrics_registry to api-gateway AppState

**Files:**
- Modify: `crates/api-gateway/Cargo.toml`
- Modify: `crates/api-gateway/src/server.rs`

- [ ] **Step 1: Add observability dependency**

```toml
# crates/api-gateway/Cargo.toml, under [dependencies]
observability = { path = "../observability" }
```

- [ ] **Step 2: Add field to AppState**

Open `crates/api-gateway/src/server.rs`. Add:

```rust
pub struct AppState {
    pub tenant_manager: Arc<dyn TenantManager>,
    /// Optional metrics registry for Prometheus export.
    /// When None, /metrics falls back to legacy bare gauge.
    pub metrics_registry: Option<Arc<observability::MetricsRegistry>>,
    // ... other fields unchanged ...
}
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p api-gateway 2>&1 | tail -5
```

Expected: may fail if `AppState` is constructed somewhere without the new field. Fix the construction site(s) to add `metrics_registry: None`.

- [ ] **Step 4: Fix all AppState construction sites**

Search for `AppState {` in the api-gateway crate:

```bash
grep -rn "AppState\s*{" crates/api-gateway/src/ | grep -v "struct AppState"
```

Add `metrics_registry: None,` to each.

- [ ] **Step 5: Verify compilation again**

```bash
cargo check -p api-gateway 2>&1 | tail -5
```

Expected: compiles.

- [ ] **Step 6: Commit**

```bash
git add crates/api-gateway/Cargo.toml crates/api-gateway/src/server.rs
git commit -m "feat(api-gateway): add metrics_registry to AppState"
```

---

### Task 9: Rewrite /metrics endpoint

**Files:**
- Modify: `crates/api-gateway/src/routes/metrics.rs`

- [ ] **Step 1: Rewrite the handler**

Replace the entire file content:

```rust
use std::sync::Arc;

use axum::response::IntoResponse;

use crate::server::AppState;

pub async fn get(
    state: axum::extract::State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Some(ref registry) = state.metrics_registry {
        // Populate per-tenant active session gauge from live data
        if let Ok(counts) = state.tenant_manager.active_session_counts().await {
            for (tenant_id, count) in &counts {
                registry.set_gauge(
                    "pandaria_sessions_active",
                    &[("tenant_id", tenant_id)],
                    *count as i64,
                );
            }
        }
        let body = registry.export();
        return ([("content-type", "text/plain; charset=utf-8")], body);
    }

    // Fallback: registry not configured — return legacy bare gauge
    let active = state.tenant_manager.active_session_count();
    let body = format!(
        "# HELP pandaria_active_sessions Active sessions\n\
         # TYPE pandaria_active_sessions gauge\n\
         pandaria_active_sessions {}\n",
        active
    );
    ([("content-type", "text/plain; charset=utf-8")], body)
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p api-gateway 2>&1 | tail -5
```

Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/api-gateway/src/routes/metrics.rs
git commit -m "feat(api-gateway): rewrite /metrics with Prometheus export and per-tenant gauge"
```

---

### Task 10: Wire up MetricsRegistry in main.rs

**Files:**
- Modify: `crates/api-gateway/src/main.rs`

- [ ] **Step 1: Create MetricsRegistry at startup**

In `main()`, before constructing `TenantManagerImpl`:

```rust
use std::sync::Arc;

let metrics_registry = Arc::new(observability::MetricsRegistry::new());
```

- [ ] **Step 2: Pass to TenantManagerImpl**

Update the `TenantManagerImpl::new()` call:

```rust
let tenant_manager = TenantManagerImpl::new(
    // ... existing args ...
    Some(metrics_registry.clone()),  // metrics
);
```

- [ ] **Step 3: Pass to AppState**

Update the `AppState` construction:

```rust
let state = Arc::new(AppState {
    tenant_manager: Arc::new(tenant_manager),
    metrics_registry: Some(metrics_registry),
    // ... other fields unchanged ...
});
```

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p api-gateway 2>&1 | tail -5
```

Expected: compiles. If `TenantManagerImpl::new()` signature hasn't been updated yet, fix it.

- [ ] **Step 5: Commit**

```bash
git add crates/api-gateway/src/main.rs
git commit -m "feat(api-gateway): wire MetricsRegistry into startup and AppState"
```

---

### Task 11: E2E tests for /metrics endpoint

**Files:**
- Create: `crates/api-gateway/tests/e2e/e2e_metrics.rs`

- [ ] **Step 1: Create E2E test file**

```rust
use crate::common::*;

/// Verify GET /metrics returns 200 and valid Prometheus format.
#[tokio::test]
async fn e2e_metrics_endpoint_returns_prometheus() {
    let server = TestServer::start().await;

    let resp = server.get("/metrics").await;
    assert_eq!(resp.status(), 200);

    let body = resp.text().await;
    // Must contain HELP and TYPE lines for each metric
    assert!(body.contains("# HELP "));
    assert!(body.contains("# TYPE "));
    // Must contain the active sessions gauge
    assert!(body.contains("pandaria_sessions_active"));
}

/// Verify metrics include session data after creating a session.
#[tokio::test]
async fn e2e_metrics_after_session_creation() {
    let server = TestServer::start().await;

    // Create a session
    let session = server
        .create_session("test-tenant", "Test session", &[])
        .await;

    let resp = server.get("/metrics").await;
    let body = resp.text().await;

    // Should have session creation counter
    assert!(body.contains("pandaria_sessions_total"));
    assert!(body.contains("created"));

    // Clean up
    server.complete_session("test-tenant", &session.id).await;
}

/// Verify per-tenant metric isolation.
#[tokio::test]
async fn e2e_metrics_multi_tenant_isolation() {
    let server = TestServer::start().await;

    // Create sessions for two tenants
    let s1 = server
        .create_session("tenant-a", "Session A", &[])
        .await;
    let s2 = server
        .create_session("tenant-b", "Session B", &[])
        .await;

    let body = server.get("/metrics").await.text().await;

    // Both tenants should have their own session counter entries
    assert!(body.contains("tenant_id=\"tenant-a\""));
    assert!(body.contains("tenant_id=\"tenant-b\""));

    // Active sessions per tenant should be distinct
    let lines_a: Vec<&str> = body
        .lines()
        .filter(|l| l.contains("tenant-a"))
        .collect();
    let lines_b: Vec<&str> = body
        .lines()
        .filter(|l| l.contains("tenant-b"))
        .collect();
    assert!(!lines_a.is_empty());
    assert!(!lines_b.is_empty());

    // Clean up
    server.complete_session("tenant-a", &s1.id).await;
    server.complete_session("tenant-b", &s2.id).await;
}
```

- [ ] **Step 2: Implement test helpers if needed**

Check if `TestServer` already has `create_session()` and `complete_session()` methods. If not, add minimal versions to the test common module. Alternatively, adapt to existing E2E test patterns (use existing helper functions from `e2e/common.rs`).

- [ ] **Step 3: Run E2E tests**

```bash
cargo test -p api-gateway --test e2e_metrics 2>&1 | tail -20
```

Expected: all 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/api-gateway/tests/e2e/e2e_metrics.rs
git commit -m "test(api-gateway): add E2E tests for /metrics endpoint"
```

---

### Task 12: Update AGENTS.md

**Files:**
- Modify: `AGENTS.md`

- [ ] **Step 1: Update observability crate status**

Find the "当前状态" table. Change:

```
| observability crate | ❌ 已删除（v0.1.3）。sanitize 移至 agent-core，metrics/tracing 暂无需求 |
```

To:

```
| observability crate | ✅ M1 已实现（v0.2.0）。`MetricsRegistry`（counter/gauge/histogram）+ Prometheus export，per-tenant 指标采集（sessions/tokens/tool calls），通过 `Arc<MetricsRegistry>` 注入各组件 |
```

- [ ] **Step 2: Add observability to module boundary diagram**

In the crates list, add:

```
  observability/      # 轻量内嵌指标采集（MetricsRegistry + Prometheus export）
```

- [ ] **Step 3: Commit**

```bash
git add AGENTS.md
git commit -m "docs: update AGENTS.md — observability M1 complete"
```

---

### Task 13: Final verification — full workspace build and test

- [ ] **Step 1: Build entire workspace**

```bash
cargo build --workspace 2>&1 | tail -10
```

Expected: compiles with no errors.

- [ ] **Step 2: Run all tests**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: all tests pass (no regressions).

- [ ] **Step 3: Run clippy**

```bash
cargo clippy --workspace -- -D warnings 2>&1 | tail -10
```

Expected: no new warnings.

- [ ] **Step 4: Commit if any fixes were needed**

```bash
git add -A
git commit -m "chore: fix workspace build/test issues after observability M1 integration"
```
