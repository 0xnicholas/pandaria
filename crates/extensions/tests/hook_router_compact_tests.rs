use std::sync::Arc;
use async_trait::async_trait;

use agent_core::context::CompactCtx;
use agent_core::mutations::{CompactDecision, HookDecision, ToolCallMutation};
use agent_core::HookDispatcher;
use agent_core::compaction::{CompactionPreparation, CompactionResult};
use agent_core::SessionEntry;
use agent_core::context::CompactReason;
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

struct CompactBlockerExt;

#[async_trait]
impl Extension for CompactBlockerExt {
    fn name(&self) -> &str { "compact_blocker" }
    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Block { reason: "too early".to_string() }
    }
}

struct CompactContinueExt;

#[async_trait]
impl Extension for CompactContinueExt {
    fn name(&self) -> &str { "compact_continue" }
}

fn dummy_compact_ctx() -> CompactCtx {
    CompactCtx {
        tenant_id: "t1".to_string(),
        session_id: "s1".to_string(),
        preparation: CompactionPreparation {
            first_kept_entry_id: uuid::Uuid::nil(),
            messages_to_summarize: vec![],
            turn_prefix_messages: vec![],
            is_split_turn: false,
            tokens_before: 0,
            previous_summary: None,
            file_ops: agent_core::file_ops::FileOperations::default(),
        },
        entries: vec![],
        reason: CompactReason::Manual,
    }
}

#[tokio::test]
async fn test_on_before_compact_first_block_wins() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext1 = Arc::new(CompactContinueExt);
    let ext2 = Arc::new(CompactBlockerExt);
    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);
    let router = HookRouter::new(vec![h1, h2], bus);

    let result = router.on_before_compact(&dummy_compact_ctx()).await;
    assert!(matches!(result, CompactDecision::Block { .. }));
}

#[tokio::test]
async fn test_on_before_compact_all_continue() {
    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext1 = Arc::new(CompactContinueExt);
    let ext2 = Arc::new(CompactContinueExt);
    let (h1, _) = ExtensionActor::spawn(ext1, bus.clone(), 8);
    let (h2, _) = ExtensionActor::spawn(ext2, bus.clone(), 8);
    let router = HookRouter::new(vec![h1, h2], bus);

    let result = router.on_before_compact(&dummy_compact_ctx()).await;
    assert!(matches!(result, CompactDecision::Continue));
}
