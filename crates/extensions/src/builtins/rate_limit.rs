use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::{HookDecision, ToolCallMutation};

use crate::host::extension::Extension;

/// Rate-limit extension — sliding-window tool call frequency limit.
///
/// Tracks calls per minute. If the limit is exceeded, returns Block.
pub struct RateLimitExtension {
    max_calls_per_minute: u64,
    call_times: Mutex<Vec<Instant>>,
}

impl RateLimitExtension {
    pub fn new(max_calls_per_minute: u64) -> Self {
        Self {
            max_calls_per_minute,
            call_times: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Extension for RateLimitExtension {
    fn name(&self) -> &str {
        "rate-limit"
    }

    async fn on_tool_call(
        &self,
        _ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        let mut times = self.call_times.lock().expect("rate-limit mutex poisoned");
        let now = Instant::now();

        // Prune entries older than 60s
        times.retain(|t| now.duration_since(*t) < Duration::from_secs(60));

        if times.len() as u64 >= self.max_calls_per_minute {
            return (
                HookDecision::Block {
                    reason: format!(
                        "rate limit exceeded: {} tool calls per minute (limit: {})",
                        times.len(),
                        self.max_calls_per_minute
                    ),
                },
                ToolCallMutation::default(),
            );
        }

        times.push(now);
        (HookDecision::Continue, ToolCallMutation::default())
    }
}