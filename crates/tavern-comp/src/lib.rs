pub mod agent;
pub mod context;
pub mod engine;
pub mod error;
pub mod event;
pub mod executor;
pub mod flow_executor;
pub mod handle;
pub mod hero;
pub mod instance;
pub mod registry;
pub mod replay;
pub mod store;
pub mod team;
pub mod timer;
pub mod validator;
pub mod workflow;

pub use context::render_template;
pub use engine::{ExecutionInfo, WorkflowEngine, send_webhook};
pub use error::CompError;
pub use event::{SignalAction, SquadEvent, WorkflowEvent};
pub use executor::StepExecutor;
pub use flow_executor::FlowStepExecutor;
pub use handle::ExecutionHandle;
pub use instance::{InstanceState, InstanceStatus};
pub use registry::{WorkflowRegistry, WorkflowSummary};
pub use replay::{
    ExecutionReplay, ExecutionReplayer, ReplayOptions, ReplaySummary, StateDiff, TimelineEntry,
};
#[cfg(feature = "postgres")]
pub use store::PostgreSQLEventStore;
#[cfg(feature = "sqlite")]
pub use store::SqliteEventStore;
pub use store::{EventStore, MemoryEventStore};
pub use team::{
    AgentExecutor, AgentExecutorError, AgentInput, AgentOutput, AgentOutputChunk, AgentResolver,
    AttachmentRef, AttachmentScope, Handoff, HandoffMode, Message, MessageKind, Mission,
    PandariaAgentExecutor, Role, SkillRef, Squad, SquadEngine, SquadResult, SquadStatus, Team, TeamContext,
    TeamRegistry, Visibility, VisibilityRules,
};
pub use timer::TimerRegistry;
pub use validator::validate_dag;
pub use workflow::{
    FLOW_AGENT_ID, InputDef, OutputDef, Process, RouterConfig, SignalTimeoutAction, Step,
    StepResult, StepStatus, WebhookConfig, Workflow, WorkflowResult,
};
pub use hero::TavernHero;
pub use hero::TavernError;
pub use hero::AgentRegistry;

// Re-export agent runtime types (from tavern-agent merge)
pub use agent::{AgentError, AgentRuntime, NativeTool, NativeToolHandler, ToolDef};

// Re-export proc-macro DSL (from tavern-flow-macros)
pub use tavern_flow_macros::{Flow, flow_impl, listen, router, start};

/// Flow 方法错误类型。
#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("flow error: {0}")]
    Other(String),
}
