# observability

Tracing, metrics, and structured logging for pandaria.

## Overview

Provides:
- **Tracing initialisation** — `init_tracing()` / `init_tracing_json()` for console/JSON logging
- **Per-tenant metrics** — counters and histograms tagged with `tenant_id` and `session_id`
- **Prometheus endpoint** — `start_metrics_exporter()` exposes `:9001/metrics`

## Usage

```rust
// Initialise logging
observability::init_tracing();

// Start metrics server
let metrics_handle = observability::start_metrics_exporter(None);

// Record events
observability::record_tool_call("t1", "s1", 0.42);
observability::record_llm_tokens("t1", "s1", 150, 80);
observability::record_compaction("t1", "s1");
```

## Metrics

| Name | Type | Description |
|---|---|---|
| `pandaria.tool_calls_total` | Counter | Tool calls executed |
| `pandaria.tool_calls_blocked_total` | Counter | Tool calls blocked by hooks |
| `pandaria.tool_calls_errors_total` | Counter | Tool call execution errors |
| `pandaria.tool_call_duration_seconds` | Histogram | Tool call execution time |
| `pandaria.llm_tokens_input_total` | Counter | Input token consumption |
| `pandaria.llm_tokens_output_total` | Counter | Output token consumption |
| `pandaria.compactions_total` | Counter | Context compaction events |
| `pandaria.agent_errors_total` | Counter | Agent-level errors |

All metrics are labeled with `tenant_id` and `session_id`.
