use async_trait::async_trait;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::{HookDecision, ToolCallMutation};

use crate::host::extension::Extension;

/// Tool-guard extension — access control based on tool names.
///
/// Rules:
/// 1. If `ctx.tool_name` is in `denied_tools` → Block (priority over allowed)
/// 2. If `allowed_tools` is non-empty and `ctx.tool_name` is not in it → Block
/// 3. Otherwise → Continue
pub struct ToolGuardExtension {
    allowed_tools: Vec<String>,
    denied_tools: Vec<String>,
}

impl ToolGuardExtension {
    pub fn new(allowed_tools: Vec<String>, denied_tools: Vec<String>) -> Self {
        Self {
            allowed_tools,
            denied_tools,
        }
    }
}

#[async_trait]
impl Extension for ToolGuardExtension {
    fn name(&self) -> &str {
        "tool-guard"
    }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
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
                    reason: format!(
                        "tool '{}' is not in allowed list ({:?})",
                        ctx.tool_name,
                        self.allowed_tools
                    ),
                },
                ToolCallMutation::default(),
            );
        }

        (HookDecision::Continue, ToolCallMutation::default())
    }
}