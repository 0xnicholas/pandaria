use std::sync::Arc;
use async_trait::async_trait;

use agent_core::context::ToolCallCtx;
use agent_core::mutations::{HookDecision, ToolCallMutation};
use agent_core::HookDispatcher;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

/// Extension that mutates input by adding a key
struct InputMutatorExt {
    key: String,
    value: serde_json::Value,
}

#[async_trait]
impl Extension for InputMutatorExt {
    fn name(&self) -> &str { "input_mutator" }

    async fn on_tool_call(&self, ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        let mut input = ctx.input.clone();
        if let Some(obj) = input.as_object_mut() {
            obj.insert(self.key.clone(), self.value.clone());
        }
        (HookDecision::Continue, ToolCallMutation { input: Some(input) })
    }
}

/// Extension that blocks
struct BlockerExt;

#[async_trait]
impl Extension for BlockerExt {
    fn name(&self) -> &str { "blocker" }

    async fn on_tool_call(&self, _ctx: &ToolCallCtx) -> (HookDecision, ToolCallMutation) {
        (HookDecision::Block { reason: "blocked".to_string() }, ToolCallMutation::default())
    }
}

#[tokio::test]
async fn test_tool_call_mutation_chain() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(InputMutatorExt { key: "ext1".to_string(), value: serde_json::json!(1) });
    let ext2 = Arc::new(InputMutatorExt { key: "ext2".to_string(), value: serde_json::json!(2) });

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({"original": true}),
    };

    let (decision, mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Continue));
    
    let input = mutation.input.expect("should have accumulated input");
    let obj = input.as_object().unwrap();
    assert!(obj.contains_key("original"));
    assert!(obj.contains_key("ext1"));
    assert!(obj.contains_key("ext2"));
}

#[tokio::test]
async fn test_tool_call_mutation_block_retained() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));

    let ext1 = Arc::new(InputMutatorExt { key: "sanitized".to_string(), value: serde_json::json!(true) });
    let ext2 = Arc::new(BlockerExt);

    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);

    let router = HookRouter::new(vec![h1, h2], bus);

    let ctx = ToolCallCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "t".to_string(),
        tool_call_id: "c1".to_string(),
        input: serde_json::json!({}),
    };

    let (decision, mutation) = router.on_tool_call(&ctx).await;
    assert!(matches!(decision, HookDecision::Block { .. }));
    
    // ext1's mutation should be preserved even though ext2 blocked
    let input = mutation.input.expect("should retain accumulated mutation");
    assert!(input.get("sanitized").is_some());
}
