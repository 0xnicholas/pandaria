use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use async_trait::async_trait;

use agent_core::context::{ToolExecutionStartCtx, ToolExecutionEndCtx, CompactEndCtx};
use agent_core::HookDispatcher;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

struct ExecutionCounterExt {
    start_count: AtomicUsize,
    end_count: AtomicUsize,
}

#[async_trait]
impl Extension for ExecutionCounterExt {
    fn name(&self) -> &str { "execution_counter" }

    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {
        self.start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_tool_execution_end(&self, _ctx: &ToolExecutionEndCtx) {
        self.end_count.fetch_add(1, Ordering::SeqCst);
    }
}

struct CompactCounterExt {
    compact_end_count: AtomicUsize,
}

#[async_trait]
impl Extension for CompactCounterExt {
    fn name(&self) -> &str { "compact_counter" }

    async fn on_compact_end(&self, _ctx: &CompactEndCtx) {
        self.compact_end_count.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn test_tool_execution_events_broadcast() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let counter = Arc::new(ExecutionCounterExt {
        start_count: AtomicUsize::new(0),
        end_count: AtomicUsize::new(0),
    });

    let (handle, _) = ExtensionActor::spawn(counter.clone(), bus.clone(), 8);
    tokio::time::sleep(Duration::from_millis(10)).await; // Let actor subscribe

    let router = HookRouter::new(vec![handle], bus.clone());

    let start_ctx = ToolExecutionStartCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };
    router.on_tool_execution_start(&start_ctx).await;

    let end_ctx = ToolExecutionEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        success: true,
    };
    router.on_tool_execution_end(&end_ctx).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(counter.start_count.load(Ordering::SeqCst), 1);
    assert_eq!(counter.end_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_compact_end_broadcast() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let counter = Arc::new(CompactCounterExt {
        compact_end_count: AtomicUsize::new(0),
    });

    let (handle, _) = ExtensionActor::spawn(counter.clone(), bus.clone(), 8);
    tokio::time::sleep(Duration::from_millis(10)).await;

    let router = HookRouter::new(vec![handle], bus.clone());

    let ctx = CompactEndCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        compacted_messages: vec![],
        token_savings: 100,
    };
    router.on_compact_end(&ctx).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(counter.compact_end_count.load(Ordering::SeqCst), 1);
}
