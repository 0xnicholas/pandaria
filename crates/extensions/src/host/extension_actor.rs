use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use agent_core::context::{
    AgentEndCtx, ContextCtx, SessionCtx, ToolCallCtx, ToolResultCtx, TurnEndCtx,
};
use agent_core::mutations::{ContextMutation, HookDecision, ToolResultMutation};

use super::event_bus::{self, EventBus};
use super::extension::Extension;

const INTERCEPT_TIMEOUT: Duration = Duration::from_millis(500);

/// Messages sent to an ExtensionActor via its mpsc channel
enum ActorMessage {
    ToolCall {
        ctx: ToolCallCtx,
        reply: oneshot::Sender<HookDecision>,
    },
    ToolResult {
        ctx: ToolResultCtx,
        reply: oneshot::Sender<ToolResultMutation>,
    },
    Context {
        ctx: ContextCtx,
        reply: oneshot::Sender<ContextMutation>,
    },
}

/// Handle for communicating with an ExtensionActor
#[derive(Clone)]
pub struct ExtensionHandle {
    sender: mpsc::Sender<ActorMessage>,
}

impl ExtensionHandle {
    /// Send a tool_call message and await the decision
    pub async fn on_tool_call(&self, ctx: ToolCallCtx) -> HookDecision {
        let (reply, rx) = oneshot::channel();
        let msg = ActorMessage::ToolCall { ctx, reply };
        let _ = self.sender.send(msg).await;
        match tokio::time::timeout(INTERCEPT_TIMEOUT, rx).await {
            Ok(Ok(decision)) => decision,
            _ => HookDecision::Continue,
        }
    }

    /// Send a tool_result message and await the mutation
    pub async fn on_tool_result(&self, ctx: ToolResultCtx) -> ToolResultMutation {
        let (reply, rx) = oneshot::channel();
        let msg = ActorMessage::ToolResult { ctx, reply };
        let _ = self.sender.send(msg).await;
        match tokio::time::timeout(INTERCEPT_TIMEOUT, rx).await {
            Ok(Ok(mutation)) => mutation,
            _ => ToolResultMutation::default(),
        }
    }

    /// Send a context message and await the mutation
    pub async fn on_context(&self, ctx: ContextCtx) -> ContextMutation {
        let (reply, rx) = oneshot::channel();
        let msg = ActorMessage::Context { ctx, reply };
        let _ = self.sender.send(msg).await;
        match tokio::time::timeout(INTERCEPT_TIMEOUT, rx).await {
            Ok(Ok(mutation)) => mutation,
            _ => ContextMutation::default(),
        }
    }
}

/// Observational events broadcast via EventBus
#[derive(Debug, Clone)]
pub enum ObsEvent {
    TurnEnd(TurnEndCtx),
    AgentEnd(AgentEndCtx),
    SessionStart(SessionCtx),
}

/// An actor running a single Extension.
pub struct ExtensionActor {
    _extension: Arc<dyn Extension>,
}

impl ExtensionActor {
    /// Spawn the actor and return its handle + JoinHandle.
    /// The actor listens on mpsc for intercepting hooks and EventBus for observational hooks.
    pub fn spawn(
        extension: Arc<dyn Extension>,
        obs_bus: Arc<EventBus<ObsEvent>>,
        buffer: usize,
    ) -> (ExtensionHandle, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<ActorMessage>(buffer);
        let handle = ExtensionHandle { sender: tx };
        let ext = extension.clone();

        let join_handle = tokio::spawn(async move {
            run_actor(ext, rx, obs_bus).await;
        });

        (handle, join_handle)
    }
}

async fn run_actor(
    extension: Arc<dyn Extension>,
    mut mpsc_rx: mpsc::Receiver<ActorMessage>,
    obs_bus: Arc<EventBus<ObsEvent>>,
) {
    // Subscribe to observational events
    let obs_rx = obs_bus.subscribe();
    let ext_for_obs = extension.clone();
    tokio::spawn(async move {
        event_bus::spawn_listener(obs_rx, move |event: ObsEvent| {
            let ext = ext_for_obs.clone();
            async move {
                match event {
                    ObsEvent::TurnEnd(ctx) => { let _ = ext.on_turn_end(&ctx).await; }
                    ObsEvent::AgentEnd(ctx) => { let _ = ext.on_agent_end(&ctx).await; }
                    ObsEvent::SessionStart(ctx) => { let _ = ext.on_session_start(&ctx).await; }
                }
            }
        });
    });

    // Process intercepting hooks from mpsc
    while let Some(msg) = mpsc_rx.recv().await {
        match msg {
            ActorMessage::ToolCall { ctx, reply } => {
                let result = extension.on_tool_call(&ctx).await;
                let _ = reply.send(result);
            }
            ActorMessage::ToolResult { ctx, reply } => {
                let result = extension.on_tool_result(&ctx).await;
                let _ = reply.send(result);
            }
            ActorMessage::Context { ctx, reply } => {
                let result = extension.on_context(&ctx).await;
                let _ = reply.send(result);
            }
        }
    }
}

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

        async fn on_tool_call(&self, ctx: &ToolCallCtx) -> HookDecision {
            if ctx.tool_name == "blocked_tool" {
                HookDecision::Block { reason: "blocked".to_string() }
            } else {
                HookDecision::Continue
            }
        }
    }

    #[tokio::test]
    async fn test_actor_blocking_hook() {
        let ext = Arc::new(TestExtension { name: "test".to_string() });
        let bus = Arc::new(EventBus::<ObsEvent>::new(16));
        let (handle, _jh) = ExtensionActor::spawn(ext, bus, 8);

        let ctx = ToolCallCtx {
            tool_name: "blocked_tool".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
        };

        let decision = handle.on_tool_call(ctx).await;
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

        let ctx = ToolCallCtx {
            tool_name: "allowed_tool".to_string(),
            tool_call_id: "c2".to_string(),
            input: serde_json::json!({}),
        };

        let decision = handle.on_tool_call(ctx).await;
        assert!(matches!(decision, HookDecision::Continue));
    }

    #[tokio::test]
    async fn test_actor_timeout_returns_default() {
        // Create an extension that takes forever
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

        let ctx = ToolResultCtx {
            tool_name: "t".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
            content: vec![],
            details: None,
            is_error: false,
        };

        // Should timeout and return default (not hang)
        let result = tokio::time::timeout(Duration::from_secs(2), handle.on_tool_result(ctx)).await;
        assert!(result.is_ok());
    }
}
