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
    reg.increment_counter("pandaria_tokens_consumed_total", &[("tenant_id", "test"), ("direction", "input")], 1500);
    reg.increment_counter("pandaria_tokens_consumed_total", &[("tenant_id", "test"), ("direction", "output")], 500);
    reg.increment_counter("pandaria_tokens_consumed_total", &[("tenant_id", "test"), ("direction", "input")], 800);
    let output = reg.export();
    assert!(output.contains("pandaria_tokens_consumed_total{direction=\"input\",tenant_id=\"test\"} 2300"));
    assert!(output.contains("pandaria_tokens_consumed_total{direction=\"output\",tenant_id=\"test\"} 500"));
}

#[test]
fn test_metrics_tool_call_all_statuses() {
    let reg = Arc::new(MetricsRegistry::new());
    reg.increment_counter("pandaria_tool_calls_total", &[("tenant_id", "t1"), ("tool", "read_file"), ("status", "success")], 3);
    reg.increment_counter("pandaria_tool_calls_total", &[("tenant_id", "t1"), ("tool", "read_file"), ("status", "blocked")], 1);
    reg.increment_counter("pandaria_tool_calls_total", &[("tenant_id", "t1"), ("tool", "read_file"), ("status", "error")], 1);
    let output = reg.export();
    assert!(output.contains("pandaria_tool_calls_total{status=\"success\",tenant_id=\"t1\",tool=\"read_file\"} 3"));
    assert!(output.contains("pandaria_tool_calls_total{status=\"blocked\",tenant_id=\"t1\",tool=\"read_file\"} 1"));
    assert!(output.contains("pandaria_tool_calls_total{status=\"error\",tenant_id=\"t1\",tool=\"read_file\"} 1"));
}

#[test]
fn test_metrics_session_lifecycle() {
    let reg = Arc::new(MetricsRegistry::new());
    reg.increment_counter("pandaria_sessions_total", &[("tenant_id", "acme"), ("status", "created")], 5);
    reg.increment_counter("pandaria_sessions_total", &[("tenant_id", "acme"), ("status", "completed")], 4);
    reg.increment_counter("pandaria_sessions_total", &[("tenant_id", "acme"), ("status", "failed")], 1);
    let output = reg.export();
    assert!(output.contains("pandaria_sessions_total{status=\"created\",tenant_id=\"acme\"} 5"));
    assert!(output.contains("pandaria_sessions_total{status=\"completed\",tenant_id=\"acme\"} 4"));
    assert!(output.contains("pandaria_sessions_total{status=\"failed\",tenant_id=\"acme\"} 1"));
}

#[test]
fn test_metrics_disabled_no_panic() {
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
    assert!(output.contains("# HELP "));
    assert!(output.contains("# TYPE "));
    for line in output.lines() {
        if !line.is_empty() && !line.starts_with('#') {
            assert!(line.contains(' '), "line missing space: {}", line);
        }
    }
}
