use std::sync::Arc;

use async_trait::async_trait;

use agent_core::compaction::{CompactionActor, CompactionConfig};
use agent_core::context::CompactCtx;
use agent_core::file_ops::DefaultFileOperationExtractor;
use agent_core::mutations::CompactDecision;
use agent_core::SessionActor;
use agent_core::SessionEntry;
use agent_core::test_utils::{TestProvider, TestResponse};
use extensions::host::event_bus::EventBus;
use extensions::host::extension::Extension;
use extensions::host::extension_actor::{ExtensionActor, ObsEvent};
use extensions::host::hook_router::HookRouter;

fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> Arc<CompactionActor> {
    Arc::new(CompactionActor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

// ============================================================================
// Mock Extensions
// ============================================================================

struct CompactContinueExt;

#[async_trait]
impl Extension for CompactContinueExt {
    fn name(&self) -> &str {
        "compact_continue"
    }

    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Continue
    }
}

struct CompactBlockerExt;

#[async_trait]
impl Extension for CompactBlockerExt {
    fn name(&self) -> &str {
        "compact_blocker"
    }

    async fn on_before_compact(&self, _ctx: &CompactCtx) -> CompactDecision {
        CompactDecision::Block {
            reason: "compaction blocked by extension".to_string(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_overflow_triggers_compaction_with_extension_hook() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(CompactContinueExt);
    let (handle, _) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let provider = TestProvider::sequence(vec![
        TestResponse::Overflow,
        TestResponse::Text("summary".into()),
        TestResponse::Text("compacted".into()),
    ]);
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "test".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    vec![],
    );

    let result = session.prompt("trigger overflow".to_string()).await;
    assert!(result.is_ok(), "expected Ok, got {:?}", result);

    // Session entries should contain a Compaction entry
    let entries = session.entries();
    assert!(
        entries.iter().any(|e| matches!(e, SessionEntry::Compaction { .. })),
        "expected a Compaction entry in session history"
    );

    // The results should contain the "compacted" text
    let results = result.unwrap();
    let has_compacted_text = results.iter().any(|m| {
        if let agent_core::types::AgentMessage::Assistant(a) = m {
            a.content.iter().any(|c| {
                if let ai_provider::Content::Text { text, .. } = c {
                    text == "compacted"
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(
        has_compacted_text,
        "expected results to contain 'compacted' text, got {:?}",
        results
    );
}

#[tokio::test]
async fn test_extension_blocks_compaction() {
    let _ = tracing_subscriber::fmt().try_init();

    let bus = Arc::new(EventBus::<ObsEvent>::new(16));
    let ext = Arc::new(CompactBlockerExt);
    let (handle, _) = ExtensionActor::spawn(ext, bus.clone(), 8);
    let router = HookRouter::new(vec![handle], bus);

    let provider = TestProvider::overflow();
    let compaction_actor = make_compaction_actor(provider.clone());
    let mut session = SessionActor::new(
        "t1".to_string(),
        "s1".to_string(),
        "You are helpful.".to_string(),
        "test".to_string(),
        provider,
        Arc::new(router),
        compaction_actor,
        vec![],
        None,
    vec![],
    );

    let result = session.prompt("trigger overflow".to_string()).await;

    // Result should be Err OR results should be empty (compaction was blocked, recovery failed)
    let is_err_or_empty = match &result {
        Err(_) => true,
        Ok(msgs) => msgs.is_empty(),
    };
    assert!(
        is_err_or_empty,
        "expected Err or empty results when compaction is blocked, got {:?}",
        result
    );

    // Session entries should NOT contain a Compaction entry
    let entries = session.entries();
    assert!(
        !entries.iter().any(|e| matches!(e, SessionEntry::Compaction { .. })),
        "expected NO Compaction entry when extension blocks compaction"
    );
}
