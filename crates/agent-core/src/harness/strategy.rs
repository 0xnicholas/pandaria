use std::time::Duration;

use crate::types::AgentMessage;

// ═══ Default constants ═══

/// Default Loop interval: 10 minutes.
pub const DEFAULT_LOOP_INTERVAL: Duration = Duration::from_secs(600);

// ═══ Strategy struct ═══

/// Three-dimensional execution strategy for an agent session.
///
/// Each dimension is independently configurable. The default (`Once` + `Once`
/// + `Accumulate`) preserves the existing single-shot behavior.
#[derive(Debug, Clone)]
pub struct SessionStrategy {
    pub termination: TerminationStrategy,
    pub rhythm: RhythmStrategy,
    pub context: ContextStrategy,
}

impl Default for SessionStrategy {
    fn default() -> Self {
        Self {
            termination: TerminationStrategy::Once,
            rhythm: RhythmStrategy::Once,
            context: ContextStrategy::Accumulate,
        }
    }
}

// ═══ Termination ═══

#[derive(Debug, Clone)]
pub enum TerminationStrategy {
    /// Stop after a single agent run.
    Once,

    /// Verify acceptance criteria after each run. Keep retrying until
    /// all criteria pass or `max_attempts` is exhausted.
    Goal {
        criteria: Vec<GoalCriterion>,
        /// Maximum attempts including the first run.
        max_attempts: u32,
        /// What to do when attempts are exhausted.
        on_exhausted: GoalExhaustedAction,
    },
}

#[derive(Debug, Clone)]
pub struct GoalCriterion {
    pub id: String,
    pub description: String,
    pub verification: GoalVerification,
}

#[derive(Debug, Clone)]
pub enum GoalVerification {
    /// Agent self-assesses and outputs `[CRITERION_RESULT: id: PASS|FAIL]`.
    SelfAssessment,
    /// Run a command; exit 0 means pass.
    Command { command: String },
    /// Check that the assistant response contains the given text.
    OutputContains { text: String },
}

#[derive(Debug, Clone)]
pub enum GoalExhaustedAction {
    /// Return an error.
    Abort,
    /// Run one more time with the original task (no verification prompt),
    /// then return the result.
    ReturnLast,
}

/// Outcome of a Goal-driven execution.
#[derive(Debug, Clone)]
pub enum GoalOutcome {
    Passed {
        messages: Vec<AgentMessage>,
        attempts: u32,
    },
    Exhausted {
        messages: Vec<AgentMessage>,
        attempts: u32,
    },
}

impl GoalOutcome {
    /// Extract messages regardless of variant.
    pub fn into_messages(self) -> Vec<AgentMessage> {
        match self {
            GoalOutcome::Passed { messages, .. } | GoalOutcome::Exhausted { messages, .. } => {
                messages
            }
        }
    }
}

/// Result of evaluating all criteria against one agent response.
#[derive(Debug, Clone)]
pub struct CriteriaEvaluation {
    pub results: Vec<(String, bool)>,
}

impl CriteriaEvaluation {
    pub fn all_passed(&self) -> bool {
        self.results.iter().all(|(_, p)| *p)
    }

    pub fn failures(&self) -> Vec<&str> {
        self.results
            .iter()
            .filter(|(_, p)| !*p)
            .map(|(id, _)| id.as_str())
            .collect()
    }
}

// ═══ Rhythm ═══

#[derive(Debug, Clone)]
pub enum RhythmStrategy {
    /// Execute immediately, once.
    Once,

    /// Run in the background on a fixed interval.
    ///
    /// The first iteration runs synchronously and returns immediately.
    /// Subsequent iterations are spawned on `tokio::spawn`; their results
    /// are pushed through the SSE event stream.
    ///
    /// When `PANDARIA_DISABLE_CRON=1`, Loop requests return
    /// `AgentError::LoopDisabled`.
    Loop {
        /// Time between iterations. Defaults to `DEFAULT_LOOP_INTERVAL` (10 min).
        interval: Option<Duration>,
        /// Maximum iterations. `None` means unlimited (until termination
        /// condition triggers or session aborts).
        max_iterations: Option<u32>,
    },
}

// ═══ Context ═══

#[derive(Debug, Clone)]
pub enum ContextStrategy {
    /// Retain all session history (default).
    Accumulate,

    /// Compact before each run, keeping the most recent N `SessionEntry`s.
    /// Actual compaction is delegated to the existing `CompactionActor`.
    Compact {
        /// Number of most recent `SessionEntry`s to retain.
        keep_last_n: usize,
    },

    /// Clear all history and rebuild the `PromptBuilder` before each run.
    Clear,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_strategy_is_once() {
        let s = SessionStrategy::default();
        assert!(matches!(s.termination, TerminationStrategy::Once));
        assert!(matches!(s.rhythm, RhythmStrategy::Once));
        assert!(matches!(s.context, ContextStrategy::Accumulate));
    }

    #[test]
    fn test_criteria_evaluation_all_passed() {
        let eval = CriteriaEvaluation {
            results: vec![
                ("a".into(), true),
                ("b".into(), true),
            ],
        };
        assert!(eval.all_passed());
        assert!(eval.failures().is_empty());
    }

    #[test]
    fn test_criteria_evaluation_some_failed() {
        let eval = CriteriaEvaluation {
            results: vec![
                ("a".into(), true),
                ("b".into(), false),
                ("c".into(), false),
            ],
        };
        assert!(!eval.all_passed());
        assert_eq!(eval.failures(), vec!["b", "c"]);
    }

    #[test]
    fn test_goal_outcome_into_messages() {
        let msgs = vec![];
        let outcome = GoalOutcome::Passed {
            messages: msgs.clone(),
            attempts: 1,
        };
        assert_eq!(outcome.into_messages().len(), 0);
    }

    #[test]
    fn test_default_loop_interval() {
        assert_eq!(DEFAULT_LOOP_INTERVAL, Duration::from_secs(600));
    }

    #[test]
    fn test_exhausted_into_messages() {
        let msgs = vec![];
        let outcome = GoalOutcome::Exhausted {
            messages: msgs.clone(),
            attempts: 5,
        };
        assert_eq!(outcome.into_messages().len(), 0);
    }
}
