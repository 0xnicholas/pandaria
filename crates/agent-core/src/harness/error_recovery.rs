use ai_provider::{AssistantMessage, StopReason};

#[derive(Debug, Clone)]
pub(crate) enum RecoveryAction {
    Continue,
    RetryAfterBackoff {
        delay_ms: u64,
    },
    #[allow(dead_code)]
    RetryAfterCompaction {
        reason: crate::hook::context::CompactReason,
    },
    Abort {
        reason: String,
    },
}

pub(crate) struct RecoveryStateMachine {
    pub overflow_attempted: bool,
    pub retry_count: u32,
    pub max_retries: u32,
}

impl RecoveryStateMachine {
    pub(crate) fn new(max_retries: u32) -> Self {
        Self {
            overflow_attempted: false,
            retry_count: 0,
            max_retries,
        }
    }

   
    #[allow(dead_code)]
    pub(crate) fn max_attempts(&self) -> u32 {
        self.max_retries
    }

    pub(crate) fn evaluate(&mut self, msg: &AssistantMessage) -> RecoveryAction {
        if is_context_overflow(msg) {
            if self.overflow_attempted {
                return RecoveryAction::Abort {
                    reason: "Context overflow recovery failed after compact-and-retry".into(),
                };
            }
            self.overflow_attempted = true;
            return RecoveryAction::RetryAfterCompaction {
                reason: crate::hook::context::CompactReason::Overflow,
            };
        }

        if is_session_retryable(msg) {
            self.retry_count += 1;
            if self.retry_count > self.max_retries {
                self.retry_count = 0;
                return RecoveryAction::Abort {
                    reason: "Max retry attempts exceeded".into(),
                };
            }
            let delay_ms = 100 * 2_u64.pow(self.retry_count - 1);
            return RecoveryAction::RetryAfterBackoff { delay_ms };
        }

        RecoveryAction::Continue
    }

    pub(crate) fn evaluate_overflow(&mut self, error_msg: &str) -> RecoveryAction {
        let lower = error_msg.to_lowercase();
        if lower.contains("context length") || lower.contains("token limit") {
            if self.overflow_attempted {
                return RecoveryAction::Abort {
                    reason: "Context overflow recovery failed after compact-and-retry".into(),
                };
            }
            self.overflow_attempted = true;
            return RecoveryAction::RetryAfterCompaction {
                reason: crate::hook::context::CompactReason::Overflow,
            };
        }
        // Non-overflow error: treat as retryable with backoff
        self.retry_count += 1;
        if self.retry_count > self.max_retries {
            self.retry_count = 0;
            return RecoveryAction::Abort {
                reason: "Max retry attempts exceeded".into(),
            };
        }
        let delay_ms = 100 * 2_u64.pow(self.retry_count - 1);
        RecoveryAction::RetryAfterBackoff { delay_ms }
    }

    pub(crate) fn mark_success(&mut self) {
        self.retry_count = 0;
    }

    #[allow(dead_code)]
    pub(crate) fn reset(&mut self) {
        self.retry_count = 0;
        self.overflow_attempted = false;
    }
}

fn is_context_overflow(msg: &AssistantMessage) -> bool {
    msg.stop_reason == StopReason::Error
        && msg.error_message.as_ref().is_some_and(|e| {
            let lower = e.to_lowercase();
            lower.contains("context length") || lower.contains("token limit")
        })
}

const RETRYABLE_PATTERNS: &[&str] = &[
    "overloaded",
    "rate limit",
    "429",
    "timeout",
    "network error",
    "service unavailable",
    "fetch failed",
    "terminated",
    "500",
    "502",
    "503",
    "504",
];

fn is_session_retryable(msg: &AssistantMessage) -> bool {
    msg.stop_reason == StopReason::Error
        && msg.error_message.as_ref().is_some_and(|e| {
            let lower = e.to_lowercase();
            RETRYABLE_PATTERNS.iter().any(|p| lower.contains(p))
        })
        && !is_context_overflow(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_provider::{Api, Usage};
    use std::time::SystemTime;

    fn make_msg(stop_reason: StopReason, error: Option<&str>) -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            provider: "test".to_string(),
            model: "test".to_string(),
            api: Api {
                provider: "test".to_string(),
                model: "test".to_string(),
            },
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason,
            response_id: None,
            error_message: error.map(|s| s.to_string()),
            timestamp: SystemTime::now(),
        }
    }

    #[test]
    fn test_overflow_first_time() {
        let mut r = RecoveryStateMachine::new(3);
        let action = r.evaluate(&make_msg(
            StopReason::Error,
            Some("context length exceeded"),
        ));
        assert!(matches!(
            action,
            RecoveryAction::RetryAfterCompaction { .. }
        ));
    }

    #[test]
    fn test_overflow_second_time_aborts() {
        let mut r = RecoveryStateMachine::new(3);
        r.evaluate(&make_msg(
            StopReason::Error,
            Some("context length exceeded"),
        ));
        let action = r.evaluate(&make_msg(
            StopReason::Error,
            Some("context length exceeded"),
        ));
        assert!(matches!(action, RecoveryAction::Abort { .. }));
    }

    #[test]
    fn test_retryable_backoff() {
        let mut r = RecoveryStateMachine::new(3);
        let action = r.evaluate(&make_msg(StopReason::Error, Some("rate limit")));
        assert!(matches!(
            action,
            RecoveryAction::RetryAfterBackoff { delay_ms: 100 }
        ));
    }

    #[test]
    fn test_retryable_exhausted() {
        let mut r = RecoveryStateMachine::new(1);
        r.evaluate(&make_msg(StopReason::Error, Some("overloaded")));
        let action = r.evaluate(&make_msg(StopReason::Error, Some("overloaded")));
        assert!(matches!(action, RecoveryAction::Abort { .. }));
    }

    #[test]
    fn test_normal_continue() {
        let mut r = RecoveryStateMachine::new(3);
        let action = r.evaluate(&make_msg(StopReason::Stop, None));
        assert!(matches!(action, RecoveryAction::Continue));
    }
}
