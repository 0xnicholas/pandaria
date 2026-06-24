use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Configuration for an external HTTP tool.
/// Defined independently in api-gateway to avoid leaking agent-core types
/// into the API contract layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Tool name, used as the tool_call identifier.
    pub name: String,
    /// Human-readable description injected into the LLM system prompt.
    pub description: String,
    /// JSON Schema describing the tool parameters.
    pub parameters: serde_json::Value,
    /// HTTP endpoint for tool execution.
    pub endpoint: String,
    /// Request timeout in milliseconds (default: 30000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Optional authentication headers.
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

/// SSE 事件类型，与 TUI 客户端 `client/model.rs` 保持字段级兼容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ServerEvent {
    #[serde(rename = "turn_start")]
    TurnStart { turn_index: u64 },

    #[serde(rename = "message_start")]
    MessageStart { message_index: u64 },

    #[serde(rename = "text_delta")]
    TextDelta { delta: String },

    /// 协议预留：当前 agent-core 未生成对应事件，MVP 不会触发。
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { content_index: usize, delta: String },

    /// 协议预留：当前 agent-core 未生成对应事件，MVP 不会触发。
    #[serde(rename = "tool_call_started")]
    ToolCallStarted { call_id: String, name: String },

    /// 协议预留：当前 agent-core 未生成对应事件，MVP 不会触发。
    #[serde(rename = "tool_call_delta")]
    ToolCallDelta { call_id: String, delta: String },

    #[serde(rename = "tool_call_done")]
    ToolCallDone {
        call_id: String,
        result: Option<String>,
        #[serde(default)]
        is_error: bool,
    },

    #[serde(rename = "turn_end")]
    TurnEnd {
        stop_reason: String,
        usage: Option<UsageInfo>,
    },

    #[serde(rename = "error")]
    Error { code: String, message: String },

    #[serde(rename = "state_changed")]
    StateChanged { state: String },

    #[serde(rename = "auto_retry_start")]
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
    },

    #[serde(rename = "auto_retry_end")]
    AutoRetryEnd {
        success: bool,
        error: Option<String>,
    },

    /// Background loop iteration completed successfully.
    #[serde(rename = "loop_iteration_complete")]
    LoopIterationComplete {
        iteration: u32,
        messages: Vec<serde_json::Value>,
    },

    /// Background loop iteration failed (loop continues).
    #[serde(rename = "loop_iteration_error")]
    LoopIterationError { iteration: u32, error: String },

    // ── Squad lifecycle events ──
    #[serde(rename = "squad_started")]
    SquadStarted { squad_id: String, team_id: String },

    #[serde(rename = "squad_mission_scheduled")]
    SquadMissionScheduled {
        squad_id: String,
        mission_id: String,
        attempt: u64,
    },

    #[serde(rename = "squad_mission_started")]
    SquadMissionStarted {
        squad_id: String,
        mission_id: String,
    },

    #[serde(rename = "squad_mission_completed")]
    SquadMissionCompleted {
        squad_id: String,
        mission_id: String,
        output: serde_json::Value,
    },

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
    SquadMissionWaitingSignal {
        squad_id: String,
        mission_id: String,
        signal_name: String,
    },

    #[serde(rename = "squad_completed")]
    SquadCompleted {
        squad_id: String,
        outputs: serde_json::Value,
    },

    #[serde(rename = "squad_failed")]
    SquadFailed { squad_id: String, reason: String },
}

/// Token 使用量统计。api-gateway 独立定义，不依赖 ai-provider 的 Usage 类型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageInfo {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

/// Session 元数据（gateway 视角）。
/// 与 tenant crate 的 `SessionInfo` 结构对齐，但 id 序列化为 String 以匹配 TUI。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: Option<String>,
    pub model: String,
    pub context_window: Option<u64>,
    pub created_at: Option<String>,
    pub turn_count: u64,
}

impl From<tenant::SessionInfo> for SessionInfo {
    fn from(info: tenant::SessionInfo) -> Self {
        Self {
            id: info.id.to_string(),
            title: info.title,
            model: info.model,
            context_window: None, // 由 gateway handler 从 ServerConfig 填充
            created_at: Some(info.created_at),
            turn_count: info.turn_count,
        }
    }
}

/// Webhook configuration for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub url: String,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub secret: Option<String>,
}

/// 创建 session 请求体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub title: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Optional external tools to register for this session.
    #[serde(default)]
    pub tools: Vec<ToolConfig>,
    /// Optional webhook configuration for event delivery.
    #[serde(default)]
    pub webhook: Option<WebhookConfig>,
    /// Built-in Pawbun tools configuration.
    #[serde(default)]
    pub builtin_tools: BuiltinToolsConfig,
    /// Execution strategy for this session.
    #[serde(default)]
    pub strategy: SessionStrategyRequest,
}

/// Configuration for built-in Pawbun tools auto-registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinToolsConfig {
    /// Enable built-in tools (default true).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Tool names to exclude from registration.
    #[serde(default)]
    pub disabled: Vec<String>,
}

impl Default for BuiltinToolsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            disabled: Vec::new(),
        }
    }
}

/// Execution strategy for a session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SessionStrategyRequest {
    #[serde(default)]
    pub termination: TerminationStrategyRequest,
    #[serde(default)]
    pub rhythm: RhythmStrategyRequest,
    #[serde(default)]
    pub context: ContextStrategyRequest,
}

impl From<SessionStrategyRequest> for agent_core::SessionStrategy {
    fn from(req: SessionStrategyRequest) -> Self {
        Self {
            termination: req.termination.into(),
            rhythm: req.rhythm.into(),
            context: req.context.into(),
        }
    }
}

/// Termination strategy.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminationStrategyRequest {
    /// Stop after a single agent run.
    #[default]
    Once,
    /// Verify acceptance criteria after each run.
    Goal {
        criteria: Vec<GoalCriterionRequest>,
        max_attempts: u32,
        on_exhausted: GoalExhaustedActionRequest,
    },
}

impl From<TerminationStrategyRequest> for agent_core::TerminationStrategy {
    fn from(req: TerminationStrategyRequest) -> Self {
        match req {
            TerminationStrategyRequest::Once => Self::Once,
            TerminationStrategyRequest::Goal {
                criteria,
                max_attempts,
                on_exhausted,
            } => Self::Goal {
                criteria: criteria.into_iter().map(Into::into).collect(),
                max_attempts,
                on_exhausted: on_exhausted.into(),
            },
        }
    }
}

/// Acceptance criterion for a Goal strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalCriterionRequest {
    pub id: String,
    pub description: String,
    pub verification: GoalVerificationRequest,
}

impl From<GoalCriterionRequest> for agent_core::GoalCriterion {
    fn from(req: GoalCriterionRequest) -> Self {
        Self {
            id: req.id,
            description: req.description,
            verification: req.verification.into(),
        }
    }
}

/// How a criterion is verified.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GoalVerificationRequest {
    /// Agent self-assesses via `[CRITERION_RESULT: id: PASS|FAIL]`.
    SelfAssessment,
    /// Run a shell command; exit 0 means pass.
    Command { command: String },
    /// Check that the assistant response contains the given text.
    OutputContains { text: String },
}

impl From<GoalVerificationRequest> for agent_core::GoalVerification {
    fn from(req: GoalVerificationRequest) -> Self {
        match req {
            GoalVerificationRequest::SelfAssessment => Self::SelfAssessment,
            GoalVerificationRequest::Command { command } => Self::Command { command },
            GoalVerificationRequest::OutputContains { text } => Self::OutputContains { text },
        }
    }
}

/// What to do when Goal attempts are exhausted.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalExhaustedActionRequest {
    /// Return an error.
    #[default]
    Abort,
    /// Run one more time with the original task and return the result.
    ReturnLast,
}

impl From<GoalExhaustedActionRequest> for agent_core::GoalExhaustedAction {
    fn from(req: GoalExhaustedActionRequest) -> Self {
        match req {
            GoalExhaustedActionRequest::Abort => Self::Abort,
            GoalExhaustedActionRequest::ReturnLast => Self::ReturnLast,
        }
    }
}

/// Rhythm strategy.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RhythmStrategyRequest {
    /// Execute immediately, once.
    #[default]
    Once,
    /// Run in the background on a fixed interval.
    Loop {
        interval_ms: Option<u64>,
        max_iterations: Option<u32>,
    },
}

impl From<RhythmStrategyRequest> for agent_core::RhythmStrategy {
    fn from(req: RhythmStrategyRequest) -> Self {
        match req {
            RhythmStrategyRequest::Once => Self::Once,
            RhythmStrategyRequest::Loop {
                interval_ms,
                max_iterations,
            } => Self::Loop {
                interval: interval_ms.map(std::time::Duration::from_millis),
                max_iterations,
            },
        }
    }
}

/// Context strategy.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextStrategyRequest {
    /// Retain all session history.
    #[default]
    Accumulate,
    /// Compact before each run, keeping the most recent N entries.
    Compact { keep_last_n: usize },
    /// Clear all history before each run.
    Clear,
}

impl From<ContextStrategyRequest> for agent_core::ContextStrategy {
    fn from(req: ContextStrategyRequest) -> Self {
        match req {
            ContextStrategyRequest::Accumulate => Self::Accumulate,
            ContextStrategyRequest::Compact { keep_last_n } => Self::Compact { keep_last_n },
            ContextStrategyRequest::Clear => Self::Clear,
        }
    }
}

/// 消息内容片段，支持多模态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContentPart {
    Text { text: String },
    Image { data: String, mime_type: String },
    Video { data: String, mime_type: String },
    Audio { data: String, mime_type: String },
}

/// 发送消息请求体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: Vec<MessageContentPart>,
}

/// Session 状态查询响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStateResponse {
    pub state: String,
    pub error_reason: Option<String>,
}

/// 配额查询响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaInfoResponse {
    pub tenant_id: String,
    pub max_concurrent_sessions: usize,
    pub active_sessions: usize,
    pub max_tokens_per_day: u64,
    pub tokens_used_today: u64,
    pub max_tool_calls_per_minute: u64,
    pub tool_calls_in_last_minute: u64,
    pub default_model: String,
    pub available_models: Vec<String>,
}

/// 批量创建 session 请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCreateRequest {
    pub count: usize,
    pub template: CreateSessionRequest,
}

/// 批量创建 session 响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCreateResult {
    pub created: Vec<SessionInfo>,
    pub failed: Vec<BatchFailure>,
}

/// 单个批量失败项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchFailure {
    pub reason: String,
}

/// Session 重置响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetSessionResponse {
    pub state: String,
}

/// 更新 session 请求体（所有字段可选）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateSessionRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// API 统一错误响应体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}

/// 发送消息成功响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub turn_index: u64,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_server_event_serde_roundtrip() {
        let events = vec![
            ServerEvent::MessageStart { message_index: 0 },
            ServerEvent::TextDelta {
                delta: "hello".into(),
            },
            ServerEvent::TurnEnd {
                stop_reason: "stop".into(),
                usage: Some(UsageInfo {
                    input_tokens: 10,
                    output_tokens: 5,
                }),
            },
            ServerEvent::Error {
                code: "rate_limited".into(),
                message: "too fast".into(),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let back: ServerEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, back);
        }
    }

    #[test]
    fn test_create_session_request_missing_system_prompt() {
        let json = r#"{"title": null}"#;
        let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
        assert!(req.title.is_none());
        assert!(req.system_prompt.is_none());
    }

    #[test]
    fn test_create_session_request_with_system_prompt() {
        let json = r#"{"title": "test", "system_prompt": "you are a helpful assistant"}"#;
        let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.title, Some("test".into()));
        assert_eq!(
            req.system_prompt,
            Some("you are a helpful assistant".into())
        );
    }

    #[test]
    fn test_session_info_from_tenant() {
        let tenant_info = tenant::SessionInfo {
            id: Uuid::new_v4(),
            tenant_id: "t1".into(),
            created_at: "1234567890".into(),
            turn_count: 0,
            system_prompt: None,
            title: Some("test".into()),
            model: "claude".into(),
        };
        let info: SessionInfo = tenant_info.into();
        assert_eq!(info.title, Some("test".into()));
        assert_eq!(info.model, "claude");
        assert_eq!(info.context_window, None);
    }

    #[test]
    fn test_session_strategy_request_default() {
        let req: CreateSessionRequest = serde_json::from_str("{}").unwrap();
        let strategy: agent_core::SessionStrategy = req.strategy.into();
        assert!(matches!(
            strategy.termination,
            agent_core::TerminationStrategy::Once
        ));
        assert!(matches!(strategy.rhythm, agent_core::RhythmStrategy::Once));
        assert!(matches!(
            strategy.context,
            agent_core::ContextStrategy::Accumulate
        ));
    }

    #[test]
    fn test_session_strategy_request_goal_loop_clear() {
        let json = r#"{
            "strategy": {
                "termination": {
                    "type": "goal",
                    "criteria": [
                        {
                            "id": "deploy-ok",
                            "description": "Deployment healthy",
                            "verification": {
                                "type": "command",
                                "command": "curl -s deploy/status | grep healthy"
                            }
                        }
                    ],
                    "max_attempts": 10,
                    "on_exhausted": "abort"
                },
                "rhythm": {
                    "type": "loop",
                    "interval_ms": 30000,
                    "max_iterations": null
                },
                "context": {
                    "type": "clear"
                }
            }
        }"#;
        let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
        let strategy: agent_core::SessionStrategy = req.strategy.into();

        let agent_core::TerminationStrategy::Goal {
            criteria,
            max_attempts,
            on_exhausted,
        } = strategy.termination
        else {
            panic!("expected Goal termination");
        };
        assert_eq!(criteria.len(), 1);
        assert_eq!(criteria[0].id, "deploy-ok");
        assert!(matches!(
            criteria[0].verification,
            agent_core::GoalVerification::Command { .. }
        ));
        assert_eq!(max_attempts, 10);
        assert!(matches!(
            on_exhausted,
            agent_core::GoalExhaustedAction::Abort
        ));

        let agent_core::RhythmStrategy::Loop {
            interval,
            max_iterations,
        } = strategy.rhythm
        else {
            panic!("expected Loop rhythm");
        };
        assert_eq!(interval, Some(std::time::Duration::from_millis(30000)));
        assert_eq!(max_iterations, None);

        assert!(matches!(
            strategy.context,
            agent_core::ContextStrategy::Clear
        ));
    }
}
