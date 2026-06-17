use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tavern_core::{AgentConfig, ModelConfig};

use super::context::TeamContext;
use super::role::Role;
pub use crate::error::AgentExecutorError;

pub struct AgentInput {
    pub task: String,
    pub context: TeamContext,
    pub model_override: Option<ModelConfig>,
    pub timeout: Option<Duration>,
    /// Squad identifier for tracing and persistence.
    pub squad_id: Option<String>,
    /// Mission identifier for tracing.
    pub mission_id: Option<String>,
}

/// Resolves an `AgentConfig` from an agent identifier.
/// Implemented by `TavernHero` in production, mockable in tests.
#[async_trait]
pub trait AgentResolver: Send + Sync {
    async fn resolve(&self, agent_id: &str) -> Option<AgentConfig>;
}

pub struct AgentOutput {
    pub content: Value,
    pub usage: Option<Value>,
    pub latency: Duration,
    pub metadata: HashMap<String, Value>,
}

pub struct AgentOutputChunk {
    pub content: Value,
    pub usage: Option<Value>,
}

#[async_trait]
pub trait AgentExecutor: Send + Sync {
    async fn resolve_role(&self, role_id: &str) -> Result<Role, AgentExecutorError>;

    async fn execute(
        &self,
        role_id: &str,
        input: AgentInput,
    ) -> Result<AgentOutput, AgentExecutorError>;

    async fn execute_stream(
        &self,
        role_id: &str,
        input: AgentInput,
    ) -> Result<BoxStream<'static, AgentOutputChunk>, AgentExecutorError>;

    /// Flush all cached session state to persistent storage.
    /// Default: no-op. Production implementations override.
    async fn flush(&self) -> Result<(), AgentExecutorError> {
        Ok(())
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use crate::team::role::Role;
    use futures_util::stream;
    use std::collections::HashMap;

    pub struct MockAgentExecutor {
        roles: HashMap<String, Role>,
        responses: HashMap<String, Value>,
    }

    impl MockAgentExecutor {
        pub fn new(roles: Vec<Role>, responses: HashMap<String, Value>) -> Self {
            Self {
                roles: roles.into_iter().map(|r| (r.id.clone(), r)).collect(),
                responses,
            }
        }
    }

    #[async_trait]
    impl AgentExecutor for MockAgentExecutor {
        async fn resolve_role(
            &self,
            role_id: &str,
        ) -> Result<Role, AgentExecutorError> {
            self.roles
                .get(role_id)
                .cloned()
                .ok_or_else(|| AgentExecutorError::RoleNotFound { id: role_id.into() })
        }

        async fn execute(
            &self,
            role_id: &str,
            input: AgentInput,
        ) -> Result<AgentOutput, AgentExecutorError> {
            let content = self.responses.get(role_id).cloned().unwrap_or_else(|| {
                serde_json::json!({
                    "received": input.task,
                    "shared": input.context.shared,
                })
            });
            Ok(AgentOutput {
                content,
                usage: None,
                latency: Duration::from_millis(10),
                metadata: HashMap::new(),
            })
        }

        async fn execute_stream(
            &self,
            _role_id: &str,
            _input: AgentInput,
        ) -> Result<BoxStream<'static, AgentOutputChunk>, AgentExecutorError> {
            Ok(Box::pin(stream::empty()))
        }
    }
}

#[cfg(test)]
pub mod stateful_mock {
    use super::*;
    use crate::team::role::Role;
    use futures_util::stream;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Mock executor that returns a sequence of responses for a given role.
    pub struct StatefulMockExecutor {
        roles: HashMap<String, Role>,
        sequences: Mutex<HashMap<String, Vec<Value>>>,
        default: Value,
    }

    impl StatefulMockExecutor {
        pub fn new(
            roles: Vec<Role>,
            sequences: HashMap<String, Vec<Value>>,
            default: Value,
        ) -> Self {
            Self {
                roles: roles.into_iter().map(|r| (r.id.clone(), r)).collect(),
                sequences: Mutex::new(sequences),
                default,
            }
        }
    }

    #[async_trait]
    impl AgentExecutor for StatefulMockExecutor {
        async fn resolve_role(
            &self,
            role_id: &str,
        ) -> Result<Role, AgentExecutorError> {
            self.roles
                .get(role_id)
                .cloned()
                .ok_or_else(|| AgentExecutorError::RoleNotFound { id: role_id.into() })
        }

        async fn execute(
            &self,
            role_id: &str,
            input: AgentInput,
        ) -> Result<AgentOutput, AgentExecutorError> {
            let content = {
                let mut seq = self.sequences.lock().unwrap();
                if let Some(items) = seq.get_mut(role_id) {
                    if !items.is_empty() {
                        items.remove(0)
                    } else {
                        self.default.clone()
                    }
                } else {
                    self.default.clone()
                }
            };

            let content = if content == Value::Null {
                serde_json::json!({
                    "received": input.task,
                    "shared": input.context.shared,
                })
            } else {
                content
            };

            Ok(AgentOutput {
                content,
                usage: None,
                latency: Duration::from_millis(10),
                metadata: HashMap::new(),
            })
        }

        async fn execute_stream(
            &self,
            _role_id: &str,
            _input: AgentInput,
        ) -> Result<BoxStream<'static, AgentOutputChunk>, AgentExecutorError> {
            Ok(Box::pin(stream::empty()))
        }
    }
}
