use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};

use agent_core::context::{
    AgentEndCtx, BeforeAgentStartCtx, CompactCtx, CompactEndCtx, ContextCtx,
    ProviderRequestCtx, ProviderResponseCtx, SessionCtx, ToolCallCtx, ToolExecutionEndCtx,
    ToolExecutionStartCtx, ToolResultCtx, TurnEndCtx,
};
use agent_core::error::AgentError;
use agent_core::mutations::{
    BeforeAgentStartMutation, CompactDecision, ContextMutation, HookDecision,
    ProviderRequestMutation, ProviderResponseMutation, ToolCallMutation, ToolResultMutation,
};
use agent_core::types::AgentToolResult;

use super::event_bus::EventBus;
use super::extension::Extension;

const INTERCEPT_TIMEOUT: Duration = Duration::from_millis(500);
const TOOL_EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);

// ============================================================================
// ExtensionCommand — messages sent to an ExtensionActor via its mpsc mailbox
// ============================================================================

pub(crate) enum ExtensionCommand {
    // Blocking hooks (first-block-wins)
    OnToolCall {
        ctx: ToolCallCtx,
        reply: oneshot::Sender<(HookDecision, ToolCallMutation)>,
    },
    OnBeforeCompact {
        ctx: CompactCtx,
        reply: oneshot::Sender<CompactDecision>,
    },

    // Chaining hooks (chain merge)
    OnToolResult {
        ctx: ToolResultCtx,
        reply: oneshot::Sender<ToolResultMutation>,
    },
    OnContext {
        ctx: ContextCtx,
        reply: oneshot::Sender<ContextMutation>,
    },
    OnBeforeAgentStart {
        ctx: BeforeAgentStartCtx,
        reply: oneshot::Sender<BeforeAgentStartMutation>,
    },
    OnBeforeProviderRequest {
        ctx: ProviderRequestCtx,
        reply: oneshot::Sender<ProviderRequestMutation>,
    },
    OnAfterProviderResponse {
        ctx: ProviderResponseCtx,
        reply: oneshot::Sender<ProviderResponseMutation>,
    },

    // Tool execution — spawned with 30s timeout
    OnExecuteTool {
        tool_call_id: String,
        params: serde_json::Value,
        reply: oneshot::Sender<Result<AgentToolResult, AgentError>>,
    },

    /// Graceful shutdown — actor exits its loop.
    Shutdown,
}

// ============================================================================
// AskError — errors from ExtensionHandle::ask
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum AskError {
    #[error("extension handler timed out")]
    Timeout,
    #[error("extension actor is gone")]
    ActorGone,
}

// ============================================================================
// ExtensionHandle — external handle to an ExtensionActor's mailbox
// ============================================================================

#[derive(Clone)]
pub struct ExtensionHandle {
    pub(crate) name: String,
    sender: mpsc::Sender<ExtensionCommand>,
}

impl ExtensionHandle {
    /// Generic ask pattern: send a command and await the oneshot reply with timeout.
    #[allow(private_bounds)]
    pub async fn ask<T: Default + Send + 'static>(
        &self,
        command_builder: impl FnOnce(oneshot::Sender<T>) -> ExtensionCommand,
        timeout: Duration,
    ) -> Result<T, AskError> {
        let (tx, rx) = oneshot::channel();
        let cmd = command_builder(tx);
        self.sender.send(cmd).await.map_err(|_| AskError::ActorGone)?;
        tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| AskError::Timeout)?
            .map_err(|_| AskError::ActorGone)
    }

    /// Send a tool_call message and await the (decision, mutation) tuple
    pub async fn on_tool_call(&self,
        ctx: ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        self.ask(
            |reply| ExtensionCommand::OnToolCall { ctx, reply },
            INTERCEPT_TIMEOUT,
        )
        .await
        .unwrap_or_else(|_| (HookDecision::Continue, ToolCallMutation::default()))
    }

    /// Send a before_compact message and await the decision
    pub async fn on_before_compact(&self,
        ctx: CompactCtx,
    ) -> CompactDecision {
        self.ask(
            |reply| ExtensionCommand::OnBeforeCompact { ctx, reply },
            INTERCEPT_TIMEOUT,
        )
        .await
        .unwrap_or(CompactDecision::Continue)
    }

    /// Send a tool_result message and await the mutation
    pub async fn on_tool_result(&self,
        ctx: ToolResultCtx,
    ) -> ToolResultMutation {
        self.ask(
            |reply| ExtensionCommand::OnToolResult { ctx, reply },
            INTERCEPT_TIMEOUT,
        )
        .await
        .unwrap_or_else(|_| ToolResultMutation::default())
    }

    /// Send a context message and await the mutation
    pub async fn on_context(&self,
        ctx: ContextCtx,
    ) -> ContextMutation {
        self.ask(
            |reply| ExtensionCommand::OnContext { ctx, reply },
            INTERCEPT_TIMEOUT,
        )
        .await
        .unwrap_or_else(|_| ContextMutation::default())
    }

    /// Send a before_agent_start message and await the mutation
    pub async fn on_before_agent_start(
        &self,
        ctx: BeforeAgentStartCtx,
    ) -> BeforeAgentStartMutation {
        self.ask(
            |reply| ExtensionCommand::OnBeforeAgentStart { ctx, reply },
            INTERCEPT_TIMEOUT,
        )
        .await
        .unwrap_or_else(|_| BeforeAgentStartMutation::default())
    }

    /// Send a before_provider_request message and await the mutation
    pub async fn on_before_provider_request(
        &self,
        ctx: ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        self.ask(
            |reply| ExtensionCommand::OnBeforeProviderRequest { ctx, reply },
            INTERCEPT_TIMEOUT,
        )
        .await
        .unwrap_or_else(|_| ProviderRequestMutation::default())
    }

    /// Send an after_provider_response message and await the mutation
    pub async fn on_after_provider_response(
        &self,
        ctx: ProviderResponseCtx,
    ) -> ProviderResponseMutation {
        self.ask(
            |reply| ExtensionCommand::OnAfterProviderResponse { ctx, reply },
            INTERCEPT_TIMEOUT,
        )
        .await
        .unwrap_or_else(|_| ProviderResponseMutation::default())
    }

    /// Execute a tool via this extension's actor, with a timeout.
    pub async fn execute_tool(
        &self,
        tool_call_id: String,
        params: serde_json::Value,
    ) -> Result<AgentToolResult, AgentError> {
        let (tx, rx) = oneshot::channel();
        let cmd = ExtensionCommand::OnExecuteTool {
            tool_call_id,
            params,
            reply: tx,
        };
        self.sender
            .send(cmd)
            .await
            .map_err(|_| AgentError::ToolExecutionFailed("extension actor terminated".into()))?;
        tokio::time::timeout(TOOL_EXECUTION_TIMEOUT, rx)
            .await
            .map_err(|_| AgentError::ToolExecutionFailed(
                format!("tool execution timed out after {:?}", TOOL_EXECUTION_TIMEOUT)
            ))?
            .map_err(|_| AgentError::ToolExecutionFailed("extension actor terminated during execution".into()))?
    }

    /// Send a Shutdown command to the actor.
    pub async fn shutdown(&self) {
        let _ = self.sender.send(ExtensionCommand::Shutdown).await;
    }
}

// ============================================================================
// ObsEvent — observational events broadcast via EventBus
// ============================================================================

#[derive(Debug, Clone)]
pub enum ObsEvent {
    TurnEnd(TurnEndCtx),
    AgentEnd(AgentEndCtx),
    SessionStart(SessionCtx),
    ToolExecutionStart(ToolExecutionStartCtx),
    ToolExecutionEnd(ToolExecutionEndCtx),
    CompactEnd(CompactEndCtx),
}

// ============================================================================
// ExtensionActor — an actor running a single Extension
// ============================================================================

/// An actor running a single Extension.
///
/// # Architecture
/// The actor uses `tokio::select!` to concurrently listen on:
/// - **mpsc mailbox**: blocking / chaining hooks (with 500ms timeout)
/// - **broadcast EventBus**: observational hooks (with 100ms timeout)
///
/// # Panic isolation
/// Each hook call is wrapped in `tokio::spawn` so that an extension panic
/// does not kill the actor. The default fallback is returned on panic.
///
/// # Warning: `std::sync::Mutex` poisoning
/// If an extension holds a `std::sync::Mutex` guard across an await point
/// and panics, the mutex will be poisoned. Extension authors should use
/// `tokio::sync::Mutex` or handle poisoning explicitly.
pub struct ExtensionActor;

impl ExtensionActor {
    /// Spawn the actor and return its handle + JoinHandle.
    pub fn spawn(
        extension: Arc<dyn Extension>,
        obs_bus: Arc<EventBus<ObsEvent>>,
        buffer: usize,
    ) -> (ExtensionHandle, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<ExtensionCommand>(buffer);
        let name = extension.name().to_string();
        let handle = ExtensionHandle { name, sender: tx };

        let join_handle = tokio::spawn(async move {
            run_actor(extension, rx, obs_bus).await;
        });

        (handle, join_handle)
    }
}

// ============================================================================
// Actor run loop
// ============================================================================

async fn run_actor(
    extension: Arc<dyn Extension>,
    mut mailbox: mpsc::Receiver<ExtensionCommand>,
    obs_bus: Arc<EventBus<ObsEvent>>,
) {
    let mut event_bus_rx = obs_bus.subscribe();

    loop {
        tokio::select! {
            // Mailbox commands (blocking / chaining hooks)
            cmd = mailbox.recv() => {
                match cmd {
                    Some(ExtensionCommand::OnToolCall { ctx, reply }) => {
                        handle_hook_with_panic_isolation(
                            extension.clone(),
                            "on_tool_call",
                            move |ext| async move { ext.on_tool_call(&ctx).await },
                            reply,
                            || (HookDecision::Continue, ToolCallMutation::default()),
                        ).await;
                    }
                    Some(ExtensionCommand::OnBeforeCompact { ctx, reply }) => {
                        handle_hook_with_panic_isolation(
                            extension.clone(),
                            "on_before_compact",
                            move |ext| async move { ext.on_before_compact(&ctx).await },
                            reply,
                            || CompactDecision::Continue,
                        ).await;
                    }
                    Some(ExtensionCommand::OnToolResult { ctx, reply }) => {
                        handle_hook_with_panic_isolation(
                            extension.clone(),
                            "on_tool_result",
                            move |ext| async move { ext.on_tool_result(&ctx).await },
                            reply,
                            ToolResultMutation::default,
                        ).await;
                    }
                    Some(ExtensionCommand::OnContext { ctx, reply }) => {
                        handle_hook_with_panic_isolation(
                            extension.clone(),
                            "on_context",
                            move |ext| async move { ext.on_context(&ctx).await },
                            reply,
                            ContextMutation::default,
                        ).await;
                    }
                    Some(ExtensionCommand::OnBeforeAgentStart { ctx, reply }) => {
                        handle_hook_with_panic_isolation(
                            extension.clone(),
                            "on_before_agent_start",
                            move |ext| async move { ext.on_before_agent_start(&ctx).await },
                            reply,
                            BeforeAgentStartMutation::default,
                        ).await;
                    }
                    Some(ExtensionCommand::OnBeforeProviderRequest { ctx, reply }) => {
                        handle_hook_with_panic_isolation(
                            extension.clone(),
                            "on_before_provider_request",
                            move |ext| async move { ext.on_before_provider_request(&ctx).await },
                            reply,
                            ProviderRequestMutation::default,
                        ).await;
                    }
                    Some(ExtensionCommand::OnAfterProviderResponse { ctx, reply }) => {
                        handle_hook_with_panic_isolation(
                            extension.clone(),
                            "on_after_provider_response",
                            move |ext| async move { ext.on_after_provider_response(&ctx).await },
                            reply,
                            ProviderResponseMutation::default,
                        ).await;
                    }
                    Some(ExtensionCommand::OnExecuteTool { tool_call_id, params, reply }) => {
                        // Tool execution: spawn for panic isolation.
                        // Timeout is enforced by ExtensionHandle::execute_tool.
                        let ext = extension.clone();
                        tokio::spawn(async move {
                            let result = ext.execute_tool(&tool_call_id, params).await;
                            let _ = reply.send(result);
                        });
                    }
                    Some(ExtensionCommand::Shutdown) | None => break,
                }
            }

            // Observation events (fire-and-forget with 100ms timeout)
            result = event_bus_rx.recv() => {
                match result {
                    Ok(event) => {
                        let ext = extension.clone();
                        tokio::spawn(async move {
                            let timeout_result = tokio::time::timeout(
                                Duration::from_millis(100),
                                async {
                                    match event {
                                        ObsEvent::TurnEnd(ctx) => ext.on_turn_end(&ctx).await,
                                        ObsEvent::AgentEnd(ctx) => ext.on_agent_end(&ctx).await,
                                        ObsEvent::SessionStart(ctx) => ext.on_session_start(&ctx).await,
                                        ObsEvent::ToolExecutionStart(ctx) => ext.on_tool_execution_start(&ctx).await,
                                        ObsEvent::ToolExecutionEnd(ctx) => ext.on_tool_execution_end(&ctx).await,
                                        ObsEvent::CompactEnd(ctx) => ext.on_compact_end(&ctx).await,
                                    }
                                },
                            ).await;

                            if timeout_result.is_err() {
                                tracing::warn!("observation hook timed out after 100ms, silently dropped");
                            }
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("EventBus listener lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

/// Helper: execute a hook inside `tokio::spawn` for panic isolation,
/// send the result via oneshot, or the default on panic.
async fn handle_hook_with_panic_isolation<T, F, Fut>(
    extension: Arc<dyn Extension>,
    hook_name: &str,
    f: F,
    reply: oneshot::Sender<T>,
    default: fn() -> T,
) where
    T: Send + 'static,
    F: FnOnce(Arc<dyn Extension>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
{
    let spawn_result = tokio::spawn(async move {
        f(extension).await
    }).await;

    match spawn_result {
        Ok(result) => { let _ = reply.send(result); }
        Err(e) => {
            tracing::error!(error = %e, hook = hook_name, "Extension panicked in hook, returning default");
            let _ = reply.send(default());
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct TestExtension {
        name: String,
    }

    #[async_trait]
    impl Extension for TestExtension {
        fn name(&self) -> &str { &self.name }

        async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
            if ctx.tool_name == "blocked_tool" {
                (HookDecision::Block { reason: "blocked".to_string() }, ToolCallMutation::default())
            } else {
                (HookDecision::Continue, ToolCallMutation::default())
            }
        }
    }

    #[tokio::test]
    async fn test_actor_blocking_hook() {
        let ext = Arc::new(TestExtension { name: "test".to_string() });
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

        let mut ctx = ToolCallCtx::new("t1", "s1", "blocked_tool", "c1");
        ctx.input = serde_json::json!({});

        let (decision, _mutation) = handle.on_tool_call(ctx).await;
        match decision {
            HookDecision::Block { reason } => assert_eq!(reason, "blocked"),
            _ => panic!("expected Block"),
        }
    }

    #[tokio::test]
    async fn test_actor_continue_hook() {
        let ext = Arc::new(TestExtension { name: "test".to_string() });
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

        let mut ctx = ToolCallCtx::new("t1", "s1", "allowed_tool", "c2");
        ctx.input = serde_json::json!({});

        let (decision, _mutation) = handle.on_tool_call(ctx).await;
        assert!(matches!(decision, HookDecision::Continue));
    }

    #[tokio::test]
    async fn test_actor_timeout_returns_default() {
        struct SlowExtension;
        #[async_trait]
        impl Extension for SlowExtension {
            fn name(&self) -> &str { "slow" }
            async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
                tokio::time::sleep(Duration::from_secs(10)).await;
                ToolResultMutation::default()
            }
        }

        let ext = Arc::new(SlowExtension);
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

        let mut ctx = ToolResultCtx::new("t1", "s1", "t", "c1");
        ctx.input = serde_json::json!({});

        let result = tokio::time::timeout(Duration::from_secs(2), handle.on_tool_result(ctx)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_actor_panic_isolation() {
        struct PanicExtension;
        #[async_trait]
        impl Extension for PanicExtension {
            fn name(&self) -> &str { "panicky" }
            async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
                panic!("intentional panic for test");
            }
        }

        let ext = Arc::new(PanicExtension);
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

        let mut ctx = ToolCallCtx::new("t1", "s1", "t", "c1");
        ctx.input = serde_json::json!({});

        let (decision, _mutation) = handle.on_tool_call(ctx).await;
        assert!(matches!(decision, HookDecision::Continue));

        let mut ctx2 = ToolCallCtx::new("t1", "s1", "t", "c2");
        ctx2.input = serde_json::json!({});
        let (decision2, _mutation2) = handle.on_tool_call(ctx2).await;
        assert!(matches!(decision2, HookDecision::Continue));
    }

    #[tokio::test]
    async fn test_actor_panic_isolation_on_tool_result() {
        struct PanicOnResult;
        #[async_trait]
        impl Extension for PanicOnResult {
            fn name(&self) -> &str { "panic_result" }
            async fn on_tool_result(&self, _ctx: &ToolResultCtx) -> ToolResultMutation {
                panic!("intentional panic in on_tool_result");
            }
        }

        let ext = Arc::new(PanicOnResult);
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

        let mut ctx = ToolResultCtx::new("t1", "s1", "t", "c1");
        ctx.input = serde_json::json!({});

        let mutation = handle.on_tool_result(ctx).await;
        assert!(mutation.content.is_none());
        assert!(mutation.details.is_none());
        assert!(mutation.is_error.is_none());
        assert!(mutation.terminate.is_none());
    }

    #[tokio::test]
    async fn test_actor_panic_isolation_on_context() {
        struct PanicOnContext;
        #[async_trait]
        impl Extension for PanicOnContext {
            fn name(&self) -> &str { "panic_context" }
            async fn on_context(&self, _ctx: &ContextCtx) -> ContextMutation {
                panic!("intentional panic in on_context");
            }
        }

        let ext = Arc::new(PanicOnContext);
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

        let ctx = ContextCtx::new("t1", "s1");

        let mutation = handle.on_context(ctx).await;
        assert!(mutation.messages.is_none());
    }

    #[tokio::test]
    async fn test_actor_shutdown() {
        let ext = Arc::new(TestExtension { name: "test".to_string() });
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let (handle, join_handle) = ExtensionActor::spawn(ext, bus, 8);

        // Send shutdown
        handle.shutdown().await;

        // Actor should exit
        let result = tokio::time::timeout(Duration::from_secs(2), join_handle).await;
        assert!(result.is_ok(), "actor should exit after shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn test_execute_tool_timeout() {
        struct SlowToolExtension;
        #[async_trait]
        impl Extension for SlowToolExtension {
            fn name(&self) -> &str { "slow_tool" }
            async fn execute_tool(
                &self,
                _tool_call_id: &str,
                _params: serde_json::Value,
            ) -> Result<AgentToolResult, AgentError> {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(AgentToolResult {
                    content: vec![],
                    details: None,
                    is_error: false,
                    terminate: false,
                })
            }
        }

        let ext = Arc::new(SlowToolExtension);
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

        let start = std::time::Instant::now();
        let result = handle.execute_tool(
            "call_1".to_string(),
            serde_json::json!({}),
        ).await;

        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "expected timeout within a few seconds, but took {:?}",
            elapsed
        );
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("timed out"),
            "expected timeout error, got: {}",
            err_msg
        );
    }
}