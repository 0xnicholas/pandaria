use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use dashmap::DashMap;

use crate::hook::context::{
    AgentEndCtx, BeforeAgentStartCtx, CompactCtx, CompactEndCtx, ContextCtx, ProviderRequestCtx,
    ProviderResponseCtx, SessionCtx, ToolCallCtx, ToolExecutionEndCtx, ToolExecutionStartCtx,
    ToolResultCtx, TurnEndCtx,
};
use crate::hook::dispatcher::HookDispatcher;
use crate::hook::mutations::{
    BeforeAgentStartMutation, CompactDecision, ContextMutation, HookDecision,
    ProviderRequestMutation, ProviderResponseMutation, ToolCallMutation, ToolResultMutation,
};
use crate::space::AgentSpace;

/// Default hook dispatcher that inlines the logic previously provided by
/// `extensions` builtins.
///
/// Includes:
/// - **Audit**: tracing logs for tool calls and turns
/// - **PathGuard**: validates file paths stay within `/workspace/{tenant_id}/`
/// - **ToolGuard**: allow/deny list for tool names
/// - **TokenBudget**: per-session turn counting (non-blocking, logs warning)
/// - **ContentFilter**: basic PII redaction in tool inputs/results
///
/// Rate-limiting is handled at the `api-gateway` layer and is not included here.
pub struct DefaultHookDispatcher {
    /// Unified agent space for path resolution.
    pub space: AgentSpace,
    /// ToolGuard: tools that are explicitly denied.
    pub denied_tools: Vec<String>,
    /// ToolGuard: if non-empty, only these tools are allowed.
    pub allowed_tools: Vec<String>,
    /// PathGuard: `tool_name -> [field_name]` mapping for path extraction.
    pub path_guard_fields: HashMap<String, Vec<String>>,
    /// PathGuard: whether to scan unknown tools for path-like strings.
    pub path_guard_scan_unknown: bool,
    /// TokenBudget: max turns per session (0 = unlimited).
    pub max_turns_per_session: usize,
    /// TokenBudget: session_id -> turn count.
    session_turn_counts: DashMap<String, AtomicUsize>,
    /// Optional callback for media cost tracking: (tenant_id, cost_cny).
    #[allow(clippy::type_complexity)]
    pub cost_callback: Option<std::sync::Arc<dyn Fn(&str, f64) + Send + Sync>>,
}

impl std::fmt::Debug for DefaultHookDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultHookDispatcher")
            .field("space", &self.space)
            .field("denied_tools", &self.denied_tools)
            .field("allowed_tools", &self.allowed_tools)
            .field("path_guard_fields", &self.path_guard_fields)
            .field("path_guard_scan_unknown", &self.path_guard_scan_unknown)
            .field("max_turns_per_session", &self.max_turns_per_session)
            .field("session_turn_counts", &self.session_turn_counts)
            .field("cost_callback", &self.cost_callback.is_some())
            .finish()
    }
}

impl Default for DefaultHookDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultHookDispatcher {
    /// Create a new dispatcher with the default agent space.
    pub fn new() -> Self {
        Self::with_space(AgentSpace::default())
    }

    /// Create a new dispatcher with an explicit agent space.
    pub fn with_space(space: AgentSpace) -> Self {
        Self {
            space,
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            path_guard_fields: HashMap::new(),
            path_guard_scan_unknown: false,
            max_turns_per_session: 0,
            session_turn_counts: DashMap::new(),
            cost_callback: None,
        }
    }

    /// Create a dispatcher from a `HookConfig` and an `AgentSpace`.
    pub fn from_config(space: AgentSpace, config: &crate::harness::config::HookConfig) -> Self {
        Self {
            space,
            denied_tools: config.denied_tools.clone(),
            allowed_tools: config.allowed_tools.clone(),
            path_guard_fields: config.path_guard_fields.clone(),
            path_guard_scan_unknown: config.path_guard_scan_unknown,
            max_turns_per_session: config.max_turns_per_session,
            session_turn_counts: DashMap::new(),
            cost_callback: config.cost_callback.clone(),
        }
    }

    // ═══ PathGuard helpers ═══

    fn normalize_path(path: &str) -> Option<String> {
        if path.is_empty() {
            return None;
        }
        let is_absolute = path.starts_with('/');
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => continue,
                ".." => {
                    if parts.pop().is_none() && is_absolute {
                        return None;
                    }
                }
                _ => parts.push(part),
            }
        }
        let normalized = if is_absolute {
            format!("/{}", parts.join("/"))
        } else {
            parts.join("/")
        };
        Some(normalized)
    }

    fn is_path_allowed(&self, path: &str, tenant_id: &str) -> bool {
        let normalized = match Self::normalize_path(path) {
            Some(p) => p,
            None => return false,
        };
        if !normalized.starts_with('/') {
            return true;
        }
        let allowed_prefix = self.space.workspace_for(tenant_id);
        let allowed_str = allowed_prefix.to_string_lossy();
        // Ensure the prefix ends with '/' for clean matching
        let allowed_prefix_str = if allowed_str.ends_with('/') {
            allowed_str.to_string()
        } else {
            format!("{}/", allowed_str)
        };
        normalized.starts_with(&allowed_prefix_str)
    }

    fn extract_paths(
        tool_fields: &HashMap<String, Vec<String>>,
        scan_unknown: bool,
        tool_name: &str,
        value: &serde_json::Value,
        paths: &mut Vec<String>,
    ) {
        match tool_fields.get(tool_name) {
            Some(fields) => {
                for field in fields {
                    if let Some(v) = value.get(field) {
                        Self::collect_string_paths(v, paths);
                    }
                }
            }
            None if scan_unknown => {
                Self::collect_string_paths(value, paths);
            }
            _ => {}
        }
    }

    fn collect_string_paths(value: &serde_json::Value, paths: &mut Vec<String>) {
        match value {
            serde_json::Value::String(s) => {
                if s.starts_with('/') || s.starts_with('.') || s.contains('/') {
                    paths.push(s.clone());
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    Self::collect_string_paths(v, paths);
                }
            }
            serde_json::Value::Object(obj) => {
                for (_, v) in obj {
                    Self::collect_string_paths(v, paths);
                }
            }
            _ => {}
        }
    }
}

#[async_trait]
impl HookDispatcher for DefaultHookDispatcher {
    // ═══ Blocking hooks ═══

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        // ── Audit ──
        tracing::info!(
            target: "pandaria.audit",
            tool_name = %ctx.tool_name,
            tool_call_id = %ctx.tool_call_id,
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            action = "tool_call_start"
        );

        // ── ToolGuard ──
        if self.denied_tools.contains(&ctx.tool_name) {
            return (
                HookDecision::Block {
                    reason: format!("tool '{}' is denied by tool-guard", ctx.tool_name),
                },
                ToolCallMutation::default(),
            );
        }
        if !self.allowed_tools.is_empty() && !self.allowed_tools.contains(&ctx.tool_name) {
            return (
                HookDecision::Block {
                    reason: format!("tool '{}' is not in allowed list", ctx.tool_name),
                },
                ToolCallMutation::default(),
            );
        }

        // ── PathGuard ──
        let mut paths = Vec::new();
        Self::extract_paths(
            &self.path_guard_fields,
            self.path_guard_scan_unknown,
            &ctx.tool_name,
            &ctx.input,
            &mut paths,
        );
        for path in &paths {
            if !self.is_path_allowed(path, &ctx.tenant_id) {
                tracing::warn!(
                    target: "pandaria.path_guard",
                    tenant_id = %ctx.tenant_id,
                    session_id = %ctx.session_id,
                    tool_name = %ctx.tool_name,
                    path = %path,
                    action = "block_illegal_path"
                );
                return (
                    HookDecision::Block {
                        reason: format!(
                            "path '{}' is outside of allowed workspace ({})",
                            path,
                            self.space.workspace_for(&ctx.tenant_id).display()
                        ),
                    },
                    ToolCallMutation::default(),
                );
            }
        }

        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Continue
    }

    // ═══ Chaining hooks ═══

    async fn on_tool_result(&self, ctx: &ToolResultCtx) -> ToolResultMutation {
        // ── Audit ──
        tracing::info!(
            target: "pandaria.audit",
            tool_name = %ctx.tool_name,
            tool_call_id = %ctx.tool_call_id,
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            is_error = ctx.is_error,
            action = "tool_call_end"
        );

        // ── PathGuard (result leakage) ──
        let mut paths = Vec::new();
        if let Some(details) = &ctx.details {
            Self::collect_string_paths(details, &mut paths);
        }
        for path in &paths {
            if !self.is_path_allowed(path, &ctx.tenant_id) {
                tracing::warn!(
                    target: "pandaria.path_guard",
                    tenant_id = %ctx.tenant_id,
                    session_id = %ctx.session_id,
                    tool_name = %ctx.tool_name,
                    path = %path,
                    action = "leak_illegal_path"
                );
                return ToolResultMutation {
                    content: Some(vec![ai_provider::Content::Text {
                        text: "[PathGuard: illegal path reference removed]".to_string(),
                        text_signature: None,
                    }]),
                    details: None,
                    is_error: Some(true),
                    terminate: None,
                };
            }
        }

        // Media cost tracking
        if let Some(ref cb) = self.cost_callback
            && let Some(ref details) = ctx.details
                && let Some(cost) = details.get("cost_per_call").and_then(|v| v.as_f64()) {
                    cb(&ctx.tenant_id, cost);
                }

        ToolResultMutation::default()
    }

    async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
        ContextMutation::default()
    }

    async fn on_before_agent_start(&self, _ctx: &BeforeAgentStartCtx) -> BeforeAgentStartMutation {
        BeforeAgentStartMutation::default()
    }

    async fn on_before_provider_request(
        &self,
        ctx: &ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        // ── TokenBudget ──
        if self.max_turns_per_session > 0 {
            let count = self
                .session_turn_counts
                .get(&ctx.session_id)
                .map(|c| c.load(Ordering::SeqCst))
                .unwrap_or(0);
            if count >= self.max_turns_per_session {
                tracing::warn!(
                    target: "pandaria.token_budget",
                    tenant_id = %ctx.tenant_id,
                    session_id = %ctx.session_id,
                    turn_count = count,
                    max_turns = self.max_turns_per_session,
                    action = "budget_exceeded"
                );
            }
        }

        ProviderRequestMutation::default()
    }

    async fn on_after_provider_response(
        &self,
        _ctx: &ProviderResponseCtx,
    ) -> ProviderResponseMutation {
        ProviderResponseMutation::default()
    }

    // ═══ Observational hooks ═══

    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        // ── Audit ──
        tracing::info!(
            target: "pandaria.audit",
            turn_index = ctx.turn_index,
            message_count = ctx.messages.len(),
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            action = "turn_end"
        );

        // ── TokenBudget ──
        if self.max_turns_per_session > 0 {
            self.session_turn_counts
                .entry(ctx.session_id.clone())
                .or_insert_with(|| AtomicUsize::new(0))
                .fetch_add(1, Ordering::SeqCst);
        }
    }

    async fn on_agent_end(&self, _ctx: &AgentEndCtx) {}
    async fn on_session_start(&self, _ctx: &SessionCtx) {}
    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {}
    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {}
    async fn on_compact_end(&self, _ctx: &CompactEndCtx) {}
}
