use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use async_trait::async_trait;

use agent_core::context::{ToolExecutionStartCtx, ToolExecutionEndCtx};
use agent_core::HookDispatcher;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

struct ToolExecutionRecorderExt {
    start_count: AtomicUsize,
    end_count: AtomicUsize,
    last_success: std::sync::Mutex<Option<bool>>,
}

#[async_trait]
impl Extension for ToolExecutionRecorderExt {
    fn name(&self) -> &str { "execution_recorder" }

    async fn on_tool_execution_start(&self, _ctx: &ToolExecutionStartCtx) {
        self.start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_tool_execution_end(&self, ctx: &ToolExecutionEndCtx) {
        self.end_count.fetch_add(1, Ordering::SeqCst);
        *self.last_success.lock().unwrap() = Some(ctx.success);
    }
}

#[tokio::test]
async fn test_tool_execution_hooks_fire() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let recorder = Arc::new(ToolExecutionRecorderExt {
        start_count: AtomicUsize::new(0),
        end_count: AtomicUsize::new(0),
        last_success: std::sync::Mutex::new(None),
    });

    let (handle, _) = ExtensionActor::spawn(recorder.clone(), bus.clone(), 8);
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

    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(recorder.start_count.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.end_count.load(Ordering::SeqCst), 1);
    assert_eq!(*recorder.last_success.lock().unwrap(), Some(true));
}

#[tokio::test]
async fn test_tool_execution_hooks_on_error() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let recorder = Arc::new(ToolExecutionRecorderExt {
        start_count: AtomicUsize::new(0),
        end_count: AtomicUsize::new(0),
        last_success: std::sync::Mutex::new(None),
    });

    let (handle, _) = ExtensionActor::spawn(recorder.clone(), bus.clone(), 8);
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
        success: false,
    };
    router.on_tool_execution_end(&end_ctx).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(recorder.start_count.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.end_count.load(Ordering::SeqCst), 1);
    assert_eq!(*recorder.last_success.lock().unwrap(), Some(false));
}
