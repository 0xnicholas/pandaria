//! Observability layer for pandaria.
//!
//! Provides tracing initialisation, per-tenant metrics, and a Prometheus
//! metrics endpoint that downstream crates (api-gateway, tenant) can mount.

use std::net::SocketAddr;

use metrics::{
    Label, Unit,
    counter, describe_counter, describe_histogram, histogram,
};
use metrics_exporter_prometheus::PrometheusBuilder;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialise the tracing subscriber.
///
/// Respects the `RUST_LOG` environment variable (default: `info`).
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = fmt::layer()
        .with_target(false)
        .with_ansi(true);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init()
        .ok();
}

/// Initialise tracing with JSON output (for structured logging).
pub fn init_tracing_json() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init()
        .ok();
}

/// Register standard pandaria metrics descriptions.
fn register_metrics() {
    describe_counter!(
        "pandaria.tool_calls_total",
        Unit::Count,
        "Total number of tool calls executed"
    );
    describe_counter!(
        "pandaria.tool_calls_blocked_total",
        Unit::Count,
        "Total number of tool calls blocked by hooks"
    );
    describe_counter!(
        "pandaria.tool_calls_errors_total",
        Unit::Count,
        "Total number of tool call execution errors"
    );
    describe_histogram!(
        "pandaria.tool_call_duration_seconds",
        Unit::Seconds,
        "Duration of tool call execution"
    );
    describe_counter!(
        "pandaria.llm_tokens_input_total",
        Unit::Count,
        "Total LLM input tokens consumed"
    );
    describe_counter!(
        "pandaria.llm_tokens_output_total",
        Unit::Count,
        "Total LLM output tokens consumed"
    );
    describe_counter!(
        "pandaria.compactions_total",
        Unit::Count,
        "Total number of context compactions"
    );
    describe_counter!(
        "pandaria.agent_errors_total",
        Unit::Count,
        "Total number of agent errors"
    );
}

/// Labels for per-tenant/per-session metrics.
fn tenant_labels(tenant_id: &str, session_id: &str) -> Vec<Label> {
    vec![
        Label::new("tenant_id", tenant_id.to_string()),
        Label::new("session_id", session_id.to_string()),
    ]
}

/// Record a tool call execution.
pub fn record_tool_call(tenant_id: &str, session_id: &str, duration_secs: f64) {
    let labels = tenant_labels(tenant_id, session_id);
    counter!("pandaria.tool_calls_total", labels.clone()).increment(1);
    histogram!("pandaria.tool_call_duration_seconds", labels).record(duration_secs);
}

/// Record a blocked tool call.
pub fn record_tool_call_blocked(tenant_id: &str, session_id: &str) {
    let labels = tenant_labels(tenant_id, session_id);
    counter!("pandaria.tool_calls_blocked_total", labels).increment(1);
}

/// Record a tool call error.
pub fn record_tool_call_error(tenant_id: &str, session_id: &str) {
    let labels = tenant_labels(tenant_id, session_id);
    counter!("pandaria.tool_calls_errors_total", labels).increment(1);
}

/// Record LLM token consumption.
pub fn record_llm_tokens(
    tenant_id: &str,
    session_id: &str,
    input_tokens: u64,
    output_tokens: u64,
) {
    let labels = tenant_labels(tenant_id, session_id);
    counter!("pandaria.llm_tokens_input_total", labels.clone()).increment(input_tokens);
    counter!("pandaria.llm_tokens_output_total", labels).increment(output_tokens);
}

/// Record a context compaction event.
pub fn record_compaction(tenant_id: &str, session_id: &str) {
    let labels = tenant_labels(tenant_id, session_id);
    counter!("pandaria.compactions_total", labels).increment(1);
}

/// Record an agent error.
pub fn record_agent_error(tenant_id: &str, session_id: &str) {
    let labels = tenant_labels(tenant_id, session_id);
    counter!("pandaria.agent_errors_total", labels).increment(1);
}

/// Build and start the Prometheus exporter.
///
/// Returns a `PrometheusHandle` that exposes a `render()` method to
/// produce the Prometheus text format output.
///
/// # Example
///
/// ```ignore
/// let handle = observability::start_metrics_exporter(None);
/// // In your HTTP handler:
/// let body = handle.render();
/// ```
pub fn start_metrics_exporter(
    bind_addr: Option<SocketAddr>,
) -> metrics_exporter_prometheus::PrometheusHandle {
    let addr = bind_addr.unwrap_or_else(|| {
        "0.0.0.0:9001".parse().expect("invalid default metrics addr")
    });

    let builder = PrometheusBuilder::new().with_http_listener(addr);
    let handle = builder
        .install_recorder()
        .expect("failed to install Prometheus recorder");

    register_metrics();

    handle
}
